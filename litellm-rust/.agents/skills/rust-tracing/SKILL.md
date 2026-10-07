---
name: rust-tracing
description: Add or change diagnostic tracing in litellm-rust, including route spans, Python logging compatibility, shared subscriber policy, and exporters
---

# Rust tracing

Use upstream `tracing` throughout Rust, including `#[tracing::instrument]`, events, and span propagation. Centralize collection, filtering, sampling, and delivery infrastructure in `crates/tracing`. Direct upstream imports still reach our configured subscriber; re-exporting macros does not control delivery. Keep Rust instrumentation on `tracing` rather than introducing a parallel `log` pipeline

`litellm-tracing` owns shared subscriber layers, span field collection, and diagnostic processing. Keep adapters composable as `tracing_subscriber::Layer`s, with the host owning initialization and dispatch. Keep Python dependencies and Python object conversion in the host bridge

The intended direction is existing Python logging calls and Rust tracing events converging on one configured Rust subscriber per host runtime, with independent destination policies. Read [the Python logging design sketch](references/unified-python-logging.md) before changing forwarding, subscriber ownership, or exporters. This is proposed architecture, not an implemented feature: the current bridge sends Rust records back to the Python SDK logger, and `LoggerRule(Rollout.RUST_OPT_IN)` selects native diagnostic processing

Preserve Python logging call sites, logger names, levels, filters, handlers, propagation, formatters, exception information, and redaction. Forward an owned copy through an explicit `logging.Handler` integration. Do not replace `logging.basicConfig`, globally change levels, or take over an embedding application's root logger. Rollout disabled or native initialization unavailable must retain existing Python behavior. Guard records delivered back to Python against forwarding loops and duplicate delivery

Evaluate `pyo3-pylogger` in tracing mode as a bridge candidate, not a guaranteed drop-in solution. Inspect the selected release's implementation and record mapping before adopting it. The design sketch records compatibility gaps found in 0.5.2. `pyo3-log` forwards in the opposite direction

Hosts configure subscribers once for their runtime. A library must not unconditionally install a process-wide subscriber. A shared dispatch may remain scoped in an embedding host; do not create a new registry for each request or log record as part of the proposed convergence. Capture request correlation before crossing threads or queues, and propagate both span context and dispatch across spawned work and returned streams. Python context variables and Rust's current span are not automatically shared

Apply metadata filtering early and destination filtering with `Layer::with_filter`. Use event-aware filtering or normalized records for Python logger names and dynamic fields that are not tracing metadata. Keep rollout gating separate from verbosity filtering and sampling. Make trace sampling decisions consistently for the whole trace, with collector tail sampling when retention depends on final errors or latency. Remote sampling must not silently suppress existing Python handler output

Prefer existing OpenTelemetry bridges and OTLP exporters for logs and traces. `tracing-opentelemetry` handles traces; `opentelemetry-appender-tracing` bridges log events. Keep PostHog product analytics explicit and separate from diagnostic logs and `CustomLogger` lifecycle callbacks. Network delivery belongs behind bounded queues with batching, observable drops, and host-owned shutdown

In core, instrument execution shared by native calls and hosted machines. Use consistent route, model, provider, streaming, and outcome fields. Put status recording at shared provider boundaries instead of scattering basic logging through handlers. Keep upstream HTTP status separate from route success

Use `skip_all` and explicitly selected fields. Basic tracing excludes bodies, credentials, headers, and raw error strings. Avoid automatic `ret` or `err` capture of sensitive values. Keep payload diagnostics separate and subject to existing redaction

A returned stream retains its route span until exhaustion, error, or drop, with exactly one terminal outcome. Builder construction does not start a trace. Never hold a span entry guard across an await. Diagnostic tracing remains separate from lifecycle callbacks and `CustomLogger` dispatch

Use `litellm_tracing::sink_layer` to compose a sink with other subscriber layers. It inherits span fields into events and emits span-close summaries with elapsed time. Test observable records, concurrent isolation, dynamic filtering, sensitive-field exclusion, and stream cancellation when changing this behavior. For Python forwarding changes, also verify existing handler output, traceback and extra fields, rollout fallback, repeated initialization, and loop prevention

Consult the [tracing API](https://docs.rs/tracing/latest/tracing/) and [subscriber layers](https://docs.rs/tracing-subscriber/latest/tracing_subscriber/layer/index.html) for implementation details
