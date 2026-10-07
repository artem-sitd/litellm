#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[cfg(feature = "posthog")]
    #[error("could not configure diagnostic export")]
    Configuration(#[from] posthog_rs::ClientOptionsBuilderError),
    #[error("sample rate must be finite and between zero and one")]
    InvalidSampleRate,
    #[error("could not configure diagnostic export")]
    ExportBuild(#[from] opentelemetry_otlp::ExporterBuildError),
    #[error("diagnostic exporter operation failed")]
    Export(#[from] opentelemetry_sdk::error::OTelSdkError),
}
