from __future__ import annotations

import logging
import threading
from collections.abc import Callable, Mapping
from functools import lru_cache
from typing import Final, Protocol, cast

from litellm.rust_bridge.bindings import NativeBinding
from litellm.rust_bridge.configuration import Decision

NATIVE_ORIGIN: Final = object()
_ORIGIN_ATTRIBUTE: Final = "_litellm_native_origin"


class _ForwardingState(threading.local):
    def __init__(self) -> None:
        self.active: bool = False


_STATE: Final = _ForwardingState()


class DiagnosticEmitter(Protocol):
    def emit(self, severity: int, message: str, fields: str) -> None: ...


class NativeDiagnosticLogger(DiagnosticEmitter, Protocol):
    def configure_otlp(
        self,
        name: str,
        endpoint: str,
        headers: Mapping[str, str],
        service_name: str,
        policy: tuple[int, tuple[str, ...], float],
    ) -> None: ...
    def force_flush(self) -> None: ...
    def shutdown(self) -> None: ...
    def configure_posthog(
        self,
        name: str,
        api_key: str,
        host: str,
        service_name: str,
        policy: tuple[int, tuple[str, ...], float],
    ) -> None: ...


class NativeDiagnosticFactory(Protocol):
    def __call__(self) -> NativeDiagnosticLogger: ...


def _as_factory(value: object) -> NativeDiagnosticFactory | None:
    if not isinstance(value, type):
        return None
    return cast(NativeDiagnosticFactory, value)  # cast-ok: PyO3 factory must be a type


LOGGER: Final = NativeBinding("NativeDiagnosticLogger", validate=_as_factory)


@lru_cache(maxsize=4)
def _construct(factory: NativeDiagnosticFactory) -> NativeDiagnosticLogger:
    return factory()


def _load() -> NativeDiagnosticLogger | None:
    factory: Final = LOGGER.load()
    return None if factory is None else _construct(factory)


def _selected() -> Decision:
    from litellm.rust_bridge.catalog import LoggerContext, decision

    return decision(LoggerContext())


class ForwardingHandler(logging.Handler):
    def __init__(
        self,
        native: Callable[[], DiagnosticEmitter | None] = _load,
        selected: Callable[[], Decision] = _selected,
    ) -> None:
        super().__init__()
        self._native: Final = native
        self._selected: Final = selected
        self.failures: int = 0

    def emit(self, record: logging.LogRecord) -> None:
        if _STATE.active or record.__dict__.get(_ORIGIN_ATTRIBUTE) is NATIVE_ORIGIN:
            return
        _STATE.active = True
        try:
            if self._selected() is Decision.PYTHON:
                return
            native: Final = self._native()
            if native is None:
                return
            from litellm._logging import diagnostic_snapshot

            message, fields = diagnostic_snapshot(record)
            native.emit(record.levelno, message, fields)
        except Exception:
            self.failures += 1
        finally:
            _STATE.active = False


def _covered_by_ancestor(logger: logging.Logger, selected: tuple[logging.Logger, ...]) -> bool:
    if not logger.propagate or logger.parent is None:
        return False
    parent: Final = logger.parent
    if parent in selected or any(isinstance(handler, ForwardingHandler) for handler in parent.handlers):
        return True
    return _covered_by_ancestor(parent, selected)


def install(loggers: tuple[logging.Logger, ...]) -> None:
    for logger in loggers:
        if _covered_by_ancestor(logger, loggers):
            for handler in tuple(logger.handlers):
                if isinstance(handler, ForwardingHandler):
                    logger.removeHandler(handler)
            continue
        if not any(isinstance(handler, ForwardingHandler) for handler in logger.handlers):
            logger.addHandler(ForwardingHandler())


def configure_otlp(
    endpoint: str,
    *,
    name: str = "default",
    headers: Mapping[str, str] | None = None,
    service_name: str = "litellm",
    minimum_level: int = logging.INFO,
    target_prefixes: tuple[str, ...] = (),
    sample_rate: float = 1.0,
    loggers: tuple[logging.Logger, ...] | None = None,
) -> bool:
    if _selected() is Decision.PYTHON:
        return False
    native: Final = _load()
    if native is None:
        return False
    native.configure_otlp(
        name, endpoint, {} if headers is None else headers, service_name, (minimum_level, target_prefixes, sample_rate)
    )
    from litellm._logging import verbose_logger, verbose_proxy_logger, verbose_router_logger

    install((verbose_logger, verbose_proxy_logger, verbose_router_logger) if loggers is None else loggers)
    return True


def force_flush() -> bool:
    native: Final = _load()
    if native is None:
        return False
    native.force_flush()
    return True


def configure_posthog(
    api_key: str,
    *,
    host: str = "https://us.i.posthog.com",
    name: str = "posthog",
    service_name: str = "litellm",
    minimum_level: int = logging.INFO,
    target_prefixes: tuple[str, ...] = (),
    sample_rate: float = 1.0,
    loggers: tuple[logging.Logger, ...] | None = None,
) -> bool:
    if _selected() is Decision.PYTHON:
        return False
    native: Final = _load()
    if native is None or not callable(getattr(native, "configure_posthog", None)):
        return False
    native.configure_posthog(name, api_key, host, service_name, (minimum_level, target_prefixes, sample_rate))
    from litellm._logging import verbose_logger, verbose_proxy_logger, verbose_router_logger

    install((verbose_logger, verbose_proxy_logger, verbose_router_logger) if loggers is None else loggers)
    return True


def shutdown() -> bool:
    native: Final = _load()
    if native is None:
        return False
    native.shutdown()
    return True
