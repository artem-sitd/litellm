# Native diagnostic tracing

`litellm-tracing` connects standard `tracing` spans and events to a host-provided `Sink`. It has no Python dependency. Hosts choose scoped dispatch or explicitly install a global subscriber

Use upstream `tracing` macros and `#[tracing::instrument(skip_all, fields(...))]` in native code. A host creates a `Logger` with its sink, uses `scope` for synchronous operations, and wraps futures with `instrument`. Propagate both spans and dispatch into spawned work and returned streams. `Logger::current()` captures the current dispatch. Existing event macro re-exports remain available

Bindings implement `litellm_tracing::Sink` to connect events to their host runtime:

```rust
pub trait Sink: Send + Sync + 'static {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool;
    fn emit(&self, record: &Record);
}
```

Pass the implementation to `litellm_tracing::Logger::new(sink)`, then call `logger.scope(|| litellm_tracing::info!(attempt = 1, "request started"))`. The sink owns host access, level mapping, correlation capture, and delivery failures. `enabled` runs before event fields are evaluated or formatted. `emit` borrows a record; an adapter that queues delivery must copy the data it needs into an owned value

Records retain event metadata, the message, and typed event fields. Sink filtering runs for each event so runtime level changes take effect. Logging from inside a sink is suppressed to prevent recursion

`sink_layer(sink)` exposes the same adapter as a composable `tracing_subscriber::Layer`, with an independent dynamic filter. It records span field updates and inherits fields from outer to inner spans, with event fields taking precedence. Closing a span emits `span closed` at the span's level with `span_name` and monotonic `duration_ms`. Delivery rechecks the sink filter. Core records route outcomes and retains route spans until a stream ends, fails, or is dropped

The Python bridge scopes native execution to a sink that uses LiteLLM's existing Python logger. It preserves request correlation, redacts before delivering to handlers, maps Rust trace events to Python debug, and reports handler failures through `sys.unraisablehook`. It accepts LiteLLM targets only, keeping dependency wire diagnostics out of the application logger

Python consumers continue using `litellm._logging` and its existing loggers, filters, formatters, and context setters. Catalog dispatch selects the processing backend for both Python and native diagnostics. The pure `Processor` takes explicit settings and never emits events

A future Node bridge can implement the same sink with runtime-specific delivery and expose the same processor through N-API. Node callback scheduling, queue limits, and shutdown belong in that bridge; this crate has no interpreter handles

This is diagnostic logging. Request lifecycle hooks and `CustomLogger` dispatch remain separate

## Configured diagnostic export

The Python host can forward eligible Python records and native events through one shared scoped dispatch while preserving existing Python output. The `posthog` Cargo feature is enabled by default; exporters start only through explicit configuration after worker processes fork

```python
import os
import litellm
from litellm.rust_bridge import forwarding

litellm.rust(True)
forwarding.configure_posthog(
    os.environ["POSTHOG_PROJECT_KEY"],
    sample_rate=0.1,
)
forwarding.configure_otlp(
    os.environ["OTLP_LOGS_ENDPOINT"],
    name="grafana",
    headers={"Authorization": os.environ["OTLP_AUTHORIZATION"]},
)
```

Call `forwarding.shutdown()` during host shutdown to drain and stop its exporters

Configuration returns `False` when the logger rollout or native binding is unavailable. Named destinations apply independent minimum severities, target prefixes, and sampling. Errors bypass sampling; Python output is unaffected. Default forwarding covers LiteLLM's package, proxy, and router loggers. Pass `loggers=(my_logger,)` to cover another host logger explicitly

OTLP exports log records over HTTP/protobuf to a full logs endpoint. The PostHog SDK captures personless `litellm diagnostic` events, not PostHog Logs or product conversions. Both use SDK batch queues and redact before enqueueing. Flush and shutdown are best effort; they do not guarantee every record was delivered

Read [the shared pipeline notes](../../.agents/skills/rust-tracing/references/unified-python-logging.md) for normalization, rollout, process ownership, and remaining scope
