//! Provider abstractions for generated-media backends.

use crate::generated_media::registry::{GeneratedMediaRecord, RegistryError};

/// Provider-neutral request for starting a generated-media job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartGeneratedMediaRequest {
    /// Provider id, for example `mock` or `seedance`.
    pub provider: String,
    /// Requested artifact kind, such as `video`.
    pub artifact_kind: String,
    /// Editorial purpose for the output, such as `broll`.
    pub workflow_purpose: String,
    /// Prompt submitted to the provider or mock generator.
    pub prompt: String,
    /// Optional provider model override.
    pub model: Option<String>,
}

/// Minimal provider interface for generated media backends.
pub trait GeneratedMediaProvider {
    /// Stable provider id used in registry records.
    fn provider_name(&self) -> &'static str;

    /// Start an offline job and return the registry record.
    fn start_offline(
        &self,
        request: &StartGeneratedMediaRequest,
        job_id: &str,
    ) -> Result<GeneratedMediaRecord, ProviderError>;
}

/// Errors returned by generated-media providers.
#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    /// Provider-specific user-facing failure.
    #[error("{0}")]
    Message(String),
    /// Registry validation failed while building the provider record.
    #[error(transparent)]
    Registry(#[from] RegistryError),
}

/// Offline provider used by tests and development smoke paths.
pub struct MockProvider;

impl GeneratedMediaProvider for MockProvider {
    fn provider_name(&self) -> &'static str {
        "mock"
    }

    fn start_offline(
        &self,
        request: &StartGeneratedMediaRequest,
        job_id: &str,
    ) -> Result<GeneratedMediaRecord, ProviderError> {
        let output = format!("raw/generated/mock/{job_id}.mp4");
        GeneratedMediaRecord::new_mock_succeeded(job_id, output, request.prompt.clone())
            .map_err(ProviderError::from)
    }
}
