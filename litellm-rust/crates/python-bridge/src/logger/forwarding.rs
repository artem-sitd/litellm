use std::{
    collections::{BTreeMap, HashMap},
    sync::{
        Arc, OnceLock, RwLock,
        atomic::{AtomicU32, Ordering},
    },
};

#[cfg(feature = "posthog")]
use litellm_tracing::PostHogSink;
use litellm_tracing::{ExportPolicy, ExportSink, Level, Logger, Metadata, OtlpSink, Record, Sink};
use pyo3::{
    exceptions::{PyRuntimeError, PyValueError},
    prelude::*,
};

use super::PythonSink;

#[derive(Default)]
struct Destinations {
    pid: AtomicU32,
    sinks: RwLock<BTreeMap<String, Arc<dyn ExportSink>>>,
}

impl Destinations {
    fn check_process(&self) -> PyResult<()> {
        let owner = self.pid.load(Ordering::Acquire);
        if owner != 0 && owner != std::process::id() {
            return Err(PyRuntimeError::new_err(
                "configure diagnostic exporters after worker processes fork",
            ));
        }
        Ok(())
    }

    fn snapshot(&self) -> Vec<Arc<dyn ExportSink>> {
        if self.check_process().is_err() {
            return vec![];
        }
        self.sinks
            .read()
            .unwrap_or_else(|error| error.into_inner())
            .values()
            .cloned()
            .collect()
    }

    fn install(&self, name: String, sink: impl ExportSink) -> PyResult<()> {
        let previous = self
            .sinks
            .write()
            .unwrap_or_else(|error| error.into_inner())
            .insert(name, Arc::new(sink));
        if let Some(previous) = previous {
            previous
                .shutdown()
                .map_err(|_| PyRuntimeError::new_err("diagnostic exporter shutdown failed"))?;
        }
        Ok(())
    }
}

struct SharedSink(Arc<Destinations>);

impl Sink for SharedSink {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        if metadata.target() != "litellm.python"
            && !metadata.target().starts_with("litellm_")
            && !metadata.target().starts_with("_native::")
        {
            return false;
        }
        self.0.snapshot().iter().any(|sink| sink.enabled(metadata)) || PythonSink.enabled(metadata)
    }

    fn emit(&self, record: &Record) {
        for sink in self.0.snapshot() {
            sink.emit(record);
        }
        PythonSink.emit(record);
    }
}

fn runtime() -> &'static (Logger, Arc<Destinations>) {
    static RUNTIME: OnceLock<(Logger, Arc<Destinations>)> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        let destinations = Arc::new(Destinations::default());
        (Logger::new(SharedSink(destinations.clone())), destinations)
    })
}

pub(super) fn logger() -> Logger {
    runtime().0.clone()
}

fn level(value: i32) -> Level {
    match value {
        40.. => Level::ERROR,
        30..40 => Level::WARN,
        20..30 => Level::INFO,
        10..20 => Level::DEBUG,
        _ => Level::TRACE,
    }
}

#[pyclass]
pub(crate) struct NativeDiagnosticLogger;

#[pymethods]
impl NativeDiagnosticLogger {
    #[new]
    fn new() -> Self {
        Self
    }

    fn emit(&self, py: Python<'_>, severity: i32, message: String, fields: &str) -> PyResult<()> {
        runtime().1.check_process()?;
        let fields = serde_json::from_str(fields)
            .map_err(|_| PyValueError::new_err("diagnostic fields must be a JSON object"))?;
        py.detach(|| logger().emit(level(severity), &message, fields));
        Ok(())
    }

    fn configure_otlp(
        &self,
        py: Python<'_>,
        name: String,
        endpoint: String,
        headers: HashMap<String, String>,
        service_name: String,
        policy: (i32, Vec<String>, f64),
    ) -> PyResult<()> {
        let destinations = &runtime().1;
        destinations.check_process()?;
        let policy = ExportPolicy::new(level(policy.0), policy.1, policy.2).map_err(|_| {
            PyValueError::new_err("sample rate must be finite and between zero and one")
        })?;
        litellm_host_python::enter_native()?;
        destinations
            .pid
            .store(std::process::id(), Ordering::Release);
        py.detach(|| {
            let sink = OtlpSink::new(endpoint, headers, service_name, policy)
                .map_err(|_| PyRuntimeError::new_err("could not configure diagnostic export"))?;
            destinations.install(name, sink)
        })
    }

    #[cfg(feature = "posthog")]
    fn configure_posthog(
        &self,
        py: Python<'_>,
        name: String,
        api_key: String,
        host: String,
        service_name: String,
        policy: (i32, Vec<String>, f64),
    ) -> PyResult<()> {
        if api_key.trim().is_empty() {
            return Err(PyValueError::new_err(
                "a diagnostic export project key is required",
            ));
        }
        let destinations = &runtime().1;
        destinations.check_process()?;
        let policy = ExportPolicy::new(level(policy.0), policy.1, policy.2).map_err(|_| {
            PyValueError::new_err("sample rate must be finite and between zero and one")
        })?;
        litellm_host_python::enter_native()?;
        destinations
            .pid
            .store(std::process::id(), Ordering::Release);
        py.detach(|| {
            let sink = PostHogSink::new(api_key, host, service_name, policy)
                .map_err(|_| PyRuntimeError::new_err("could not configure diagnostic export"))?;
            destinations.install(name, sink)
        })
    }

    fn force_flush(&self, py: Python<'_>) -> PyResult<()> {
        runtime().1.check_process()?;
        let sinks = runtime().1.snapshot();
        py.detach(|| {
            let failed = sinks
                .iter()
                .map(|sink| sink.force_flush())
                .filter(Result::is_err)
                .count();
            if failed == 0 {
                Ok(())
            } else {
                Err(PyRuntimeError::new_err("diagnostic exporter flush failed"))
            }
        })
    }

    fn shutdown(&self, py: Python<'_>) -> PyResult<()> {
        runtime().1.check_process()?;
        let sinks = std::mem::take(
            &mut *runtime()
                .1
                .sinks
                .write()
                .unwrap_or_else(|error| error.into_inner()),
        );
        py.detach(|| {
            let failed = sinks
                .values()
                .map(|sink| sink.shutdown())
                .filter(Result::is_err)
                .count();
            if failed == 0 {
                Ok(())
            } else {
                Err(PyRuntimeError::new_err(
                    "diagnostic exporter shutdown failed",
                ))
            }
        })
    }
}
