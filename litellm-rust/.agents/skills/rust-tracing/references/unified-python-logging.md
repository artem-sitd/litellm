# Shared Python and Rust diagnostic pipeline

This is a design sketch for a future implementation. It changes no application behavior or dependencies

## Goal and current behavior

Keep existing Python `logger.debug`, `info`, `warning`, `error`, and `exception` calls working. Forward eligible records into the same Rust subscriber that receives native tracing events, then apply destination-specific filtering, gating, sampling, and export

Today, `litellm/rust_bridge/diagnostics.py` evaluates `LoggerContext` and selects native diagnostic processing when admitted. `LoggerRule(Rollout.RUST_OPT_IN)` therefore gates diagnostic processing, not Python log forwarding. Native route capture in `crates/python-bridge/src/logger/mod.rs` creates a `Logger` with a `PythonSink`, and `litellm/rust_bridge/logger.py` delivers its records to the existing `verbose_logger`. `crates/tracing` already supplies composable sink layers, span field inheritance, and diagnostic redaction

The proposal extends those boundaries with Python ingress and shared subscriber ownership. It must preserve the existing logging contract during rollout

## Proposed flow

```text
Existing Python logger calls
  -> existing logger admission and filters
  -> explicitly attached forwarding handler
  -> owned, redacted record with captured request context
  -> selected shared Rust dispatch

Native Rust tracing events and spans
  -> selected shared Rust dispatch

Shared Rust subscriber
  -> destination filters and sampling
  -> bounded export queues
  -> OTLP / collector / configured backends

Existing Python handlers continue their configured output
```

One subscriber per host runtime means one place to configure Rust telemetry policy. It does not grant ownership of arbitrary handlers installed by an embedding application. Initially, forwarding is additive: existing Python outputs remain in place, while remote telemetry uses the shared subscriber. Moving console or file delivery under that subscriber later requires a separate compatibility decision, so the same record is not written twice

Initialize the runtime once when the host opts in, attach the forwarding integration idempotently, and select the same dispatch for Python ingress and native work. Keep request data on records or spans rather than constructing a separate subscriber for each request. An embedding host may select the shared dispatch explicitly without replacing another application's global subscriber

## Python compatibility boundary

Use Python's standard `logging.Handler` interface. Keep logger identities, effective levels, attached filters, propagation, existing handlers, and formatters intact. A root handler only sees records whose propagation reaches it, so define forwarding coverage for the LiteLLM loggers explicitly and respect `propagate=False`. Attach only one forwarding path for each covered logger hierarchy

Do not change Python levels to make downstream Rust filters see messages Python already rejects. Python admission remains authoritative for existing logging behavior. Central policy can further restrict the forwarded copy. Preserve existing redaction and capture correlation before asynchronous handoff; a background worker cannot recover the caller's context variables

Copy the record before normalization. Preserve its original severity number and name, logger name, creation timestamp, filename, line, function, rendered message, exception text, stack information, and extra properties. Do not retain traceback frames, live Python objects, or the GIL in queued export work. Keep exception rendering and structured conversion bounded, and apply existing secret processing before an export can bypass Python output filters. Do not modify positional access-log arguments on the original record, since existing access formatters depend on their shape

Mark ingress origin and records emitted by a Python compatibility sink. A Rust-origin record delivered to Python must not enter Rust again through the forwarding handler. Exporter failures must not recurse through the same pipeline or raise through application logging calls

When rollout is disabled or the native binding is unavailable, leave current Python logging working. Initial rollout should retain original handler output even if the forwarding path cannot initialize

## Bridge choice

`pyo3-pylogger` forwards Python logs into Rust and is worth evaluating with `default-features = false` and `tracing-kv`. However, the published 0.5.2 source is not a complete compatibility adapter for this contract

Its setup replaces `logging.basicConfig` and defines a `HostHandler`; it does not automatically attach forwarding to existing LiteLLM handlers. Its tracing path puts the Python target, path, and line in event fields rather than native tracing metadata. Extras become a `python_fields` JSON string. Exception and stack information and the original creation timestamp are not forwarded. The README mentions `register_tracing`, but the released source exposes `register` and `setup_logging`

Check a future release against these gaps before choosing it. Prefer a small explicit handler adapter at our existing PyO3 boundary if upstream cannot satisfy them without changing Python logging setup. Keep arbitrary extras in the owned normalized record, with selected stable properties exposed as native tracing fields. Do not generate unbounded tracing callsites for every possible combination of Python extra keys

The source audit used the [0.5.2 package](https://docs.rs/crate/pyo3-pylogger/0.5.2), whose Cargo VCS metadata identifies [commit 68a5ae9](https://github.com/dylanbstorey/pyo3-pylogger/blob/68a5ae90b2b839bc4109551ab6ca10659d09c92b/src/lib.rs). The [PyO3 logging guide](https://pyo3.rs/main/ecosystem/logging.html) distinguishes Python-to-Rust forwarding from `pyo3-log`, which sends Rust logs into Python

## Filtering, gating, and sampling

Treat rollout admission, verbosity filtering, and sampling as separate decisions. Resolve opt-in through the existing rollout policy; enabling Rust inference must not become a prerequisite for observing Python-only routes

Use metadata filters for stable Rust targets and levels, and per-layer filters for independent destinations. Python logger names carried as event fields need `Filter::event_enabled` or policy over the normalized record. A JSON extras string is not automatically a set of fields available to `EnvFilter`. Context-dependent filters must allow dynamic evaluation rather than caching a random decision once per callsite

Sample related spans consistently by trace identity. Head sampling limits application export volume but cannot guarantee retention of later failures. Use collector tail sampling when the requirement is retaining error or slow traces, and send enough upstream data for that decision. Keep all spans for a trace on the same tail-sampling collector instance

Log-event sampling is a separate policy from trace sampling. Remote policies can preserve errors and sample routine summaries without affecting existing Python output. Track dropped records by destination and cause so sampling, queue overflow, and export failure are distinguishable. Do not apply diagnostic sampling to product conversion events, audit records, or lifecycle callback delivery without an explicit policy for those data

## Export boundaries

Reuse `tracing-opentelemetry` for traces and `opentelemetry-appender-tracing` for OTLP logs, backed by the appropriate SDK batch processors. A collector can forward traces to Tempo and logs to Loki or PostHog Logs. For a small deployment, direct OTLP export can avoid operating a collector; collector tail sampling is an additional deployment choice

PostHog product events use a separate explicit event schema and capture integration. A Python `INFO` message does not by itself become a product analytics event. Preserve request correlation across exported diagnostics, and distinguish existing LiteLLM correlation IDs from valid OpenTelemetry trace and span IDs

Keep sink callbacks short. Queue owned records, bound queue capacity, batch network delivery, and define drops and shutdown flushing. The host owns worker lifetime and initialization after process forks; library imports must not start exporters or take over global logging

## Implementation sequence and acceptance

First implement Python record admission and conversion into one shared dispatch behind opt-in, with a recording sink. Establish compatibility before adding a remote backend. Then add one existing OTLP exporter and configure destination policy. Add collector-based sampling only when volume and retention requirements justify it

Compatibility tests should compare existing Python output before and after opt-in, including formatted arguments, exceptions, custom extras and levels, redaction, access-log formatters, child loggers, and `propagate=False`. Cover native binding absence, initialization failure, repeated setup, and records outside Rust inference calls

Concurrency tests should interleave requests with different context variables, span parents, spawned tasks, and returned streams. Verify captured context after queue handoff, one terminal stream outcome, and no forwarding loop when a Rust record goes through Python output. Export tests should verify independent sink policies, sampling consistency, overload reporting, and bounded shutdown with unavailable backends

This sketch leaves two decisions for implementation: whether an upstream bridge release can meet the conversion contract, and which configured destinations need direct export versus a collector. It does not choose sampling percentages or add new rollout flags

## Further references

Use the [Python logging contract](https://docs.python.org/3/library/logging.html), [subscriber per-layer filtering](https://docs.rs/tracing-subscriber/latest/tracing_subscriber/layer/index.html#per-layer-filtering), and [Filter API](https://docs.rs/tracing-subscriber/latest/tracing_subscriber/layer/trait.Filter.html) for admission and routing

Use the [trace bridge](https://docs.rs/tracing-opentelemetry/latest/tracing_opentelemetry/), [log bridge](https://docs.rs/opentelemetry-appender-tracing/latest/opentelemetry_appender_tracing/), [OpenTelemetry sampling guide](https://opentelemetry.io/docs/concepts/sampling/), and [Alloy tail sampling reference](https://grafana.com/docs/alloy/latest/reference/components/otelcol/otelcol.processor.tail_sampling/) for exports and retention

Use the [Alloy trace pipeline](https://grafana.com/docs/tempo/latest/set-up-for-tracing/instrument-send/set-up-collector/grafana-alloy/), [OTLP logs to Loki](https://grafana.com/docs/loki/latest/send-data/alloy/examples/alloy-otel-logs/), and [PostHog Logs documentation source](https://github.com/PostHog/posthog.com/blob/master/contents/docs/logs/index.mdx) for destination support
