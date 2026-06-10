//! OpenRouter video-generation adapter.

use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::generated_media::registry::{
    GeneratedArtifactKind, GeneratedMediaRecord, GeneratedMediaState, GeneratedMode,
    GeneratedWorkflowPurpose, RegistryError, prompt_hash, validate_generated_output_path,
};

/// Default OpenRouter video model used when the caller does not provide one.
pub const DEFAULT_VIDEO_MODEL: &str = "bytedance/seedance-2.0";
const DEFAULT_BASE_URL: &str = "https://openrouter.ai/api/v1";

/// OpenRouter video-generation request settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenRouterVideoConfig {
    /// Bearer token from the configured OpenRouter provider key.
    pub api_key: String,
    /// Model id sent to OpenRouter.
    pub model: String,
    /// Optional base URL override for tests.
    pub base_url: String,
}

impl OpenRouterVideoConfig {
    /// Build config from environment variables.
    pub fn from_env(model: Option<String>) -> Result<Self, OpenRouterError> {
        Self::from_secret_store(model, montage_secrets::get)
    }

    fn from_secret_store(
        model: Option<String>,
        get: impl Fn(&str, &str) -> Result<Option<String>, montage_secrets::SecretError>,
    ) -> Result<Self, OpenRouterError> {
        let api_key = get(
            montage_secrets::env_vars::OPENROUTER_API_KEY,
            montage_secrets::accounts::OPENROUTER_API_KEY,
        )?
        .ok_or(OpenRouterError::MissingApiKey)?;
        let model = model
            .or_else(|| std::env::var("OPENROUTER_VIDEO_MODEL").ok())
            .unwrap_or_else(|| DEFAULT_VIDEO_MODEL.into());
        Ok(Self {
            api_key,
            model,
            base_url: DEFAULT_BASE_URL.into(),
        })
    }

    #[cfg(test)]
    fn from_env_with(
        model: Option<String>,
        get: impl Fn(&str) -> Option<String>,
    ) -> Result<Self, OpenRouterError> {
        let api_key = get("OPENROUTER_API_KEY").ok_or(OpenRouterError::MissingApiKey)?;
        let model = model
            .or_else(|| get("OPENROUTER_VIDEO_MODEL"))
            .unwrap_or_else(|| DEFAULT_VIDEO_MODEL.into());
        Ok(Self {
            api_key,
            model,
            base_url: DEFAULT_BASE_URL.into(),
        })
    }
}

/// Optional video generation controls accepted by OpenRouter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenRouterVideoOptions {
    /// Duration in seconds.
    pub duration: Option<u32>,
    /// Output resolution, for example `720p`.
    pub resolution: Option<String>,
    /// Aspect ratio, for example `16:9`.
    pub aspect_ratio: Option<String>,
    /// Whether the provider should generate audio.
    pub generate_audio: Option<bool>,
}

/// Submit a text-to-video job to OpenRouter and convert the response to a registry record.
pub async fn submit_video_job(
    config: &OpenRouterVideoConfig,
    job_id: &str,
    prompt: &str,
    options: &OpenRouterVideoOptions,
) -> Result<GeneratedMediaRecord, OpenRouterError> {
    let mut body = serde_json::json!({
        "model": config.model,
        "prompt": prompt,
    });
    if let Some(duration) = options.duration {
        body["duration"] = serde_json::json!(duration);
    }
    if let Some(resolution) = options.resolution.as_deref() {
        body["resolution"] = serde_json::json!(resolution);
    }
    if let Some(aspect_ratio) = options.aspect_ratio.as_deref() {
        body["aspect_ratio"] = serde_json::json!(aspect_ratio);
    }
    if let Some(generate_audio) = options.generate_audio {
        body["generate_audio"] = serde_json::json!(generate_audio);
    }

    let url = format!("{}/videos", config.base_url.trim_end_matches('/'));
    let response = reqwest::Client::new()
        .post(url)
        .bearer_auth(&config.api_key)
        .header("Content-Type", "application/json")
        .header("X-OpenRouter-Title", "Montage")
        .json(&body)
        .send()
        .await?;
    if !response.status().is_success() {
        return Err(OpenRouterError::Api(response.text().await?));
    }
    let submitted: OpenRouterSubmitResponse = response.json().await?;
    Ok(record_from_submit(job_id, prompt, &config.model, submitted))
}

/// Poll an OpenRouter video job status URL.
pub async fn poll_video_job(
    config: &OpenRouterVideoConfig,
    status_url: &str,
) -> Result<OpenRouterStatusResponse, OpenRouterError> {
    let response = reqwest::Client::new()
        .get(status_url)
        .bearer_auth(&config.api_key)
        .send()
        .await?;
    if !response.status().is_success() {
        return Err(OpenRouterError::Api(response.text().await?));
    }
    Ok(response.json().await?)
}

/// Download the first completed video output with OpenRouter auth.
pub async fn download_completed_video(
    config: &OpenRouterVideoConfig,
    status: &OpenRouterStatusResponse,
    output_path: &std::path::Path,
) -> Result<(), OpenRouterError> {
    let url = status
        .unsigned_urls
        .first()
        .ok_or(OpenRouterError::MissingOutputUrl)?;
    let response = reqwest::Client::new()
        .get(url)
        .bearer_auth(&config.api_key)
        .send()
        .await?;
    if !response.status().is_success() {
        return Err(OpenRouterError::Api(response.text().await?));
    }
    let bytes = response.bytes().await?;
    if let Some(parent) = output_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::write(output_path, bytes).await?;
    Ok(())
}

/// Apply OpenRouter status metadata to a local registry record.
pub fn apply_status_to_record(
    record: &mut GeneratedMediaRecord,
    status: &OpenRouterStatusResponse,
    output_video_path: Option<String>,
) -> Result<(), OpenRouterError> {
    let now = Utc::now();
    record.updated_at = now;
    record.provider_job_id = Some(status.id.clone());
    record.provider_status_url = status
        .polling_url
        .clone()
        .or_else(|| record.provider_status_url.clone());
    record.failure_message = status.error.clone();
    record.state = match status.status.as_str() {
        "completed" | "succeeded" => GeneratedMediaState::Succeeded,
        "processing" | "running" => GeneratedMediaState::Running,
        "failed" => GeneratedMediaState::Failed,
        "cancelled" => GeneratedMediaState::Cancelled,
        _ => GeneratedMediaState::Queued,
    };
    if matches!(record.state, GeneratedMediaState::Succeeded) {
        record.completed_at = Some(now);
        if let Some(output_video_path) = output_video_path {
            validate_generated_output_path(&output_video_path)?;
            record
                .output_paths
                .insert("video".into(), output_video_path);
        }
    }
    Ok(())
}

/// Project-relative output path for a completed OpenRouter video.
pub fn local_video_output_path(job_id: &str) -> String {
    format!("raw/generated/openrouter/{job_id}.mp4")
}

fn record_from_submit(
    job_id: &str,
    prompt: &str,
    model: &str,
    submitted: OpenRouterSubmitResponse,
) -> GeneratedMediaRecord {
    let now = Utc::now();
    let state = match submitted.status.as_deref() {
        Some("completed" | "succeeded") => GeneratedMediaState::Succeeded,
        Some("processing" | "running") => GeneratedMediaState::Running,
        Some("failed") => GeneratedMediaState::Failed,
        _ => GeneratedMediaState::Queued,
    };
    let failure_message = submitted.error;
    GeneratedMediaRecord {
        job_id: job_id.into(),
        artifact_kind: GeneratedArtifactKind::Video,
        workflow_purpose: GeneratedWorkflowPurpose::Broll,
        provider: "openrouter".into(),
        model: model.into(),
        provider_job_id: Some(submitted.id),
        provider_status_url: submitted.polling_url,
        generation_mode: GeneratedMode::TextToVideo,
        prompt: prompt.into(),
        prompt_hash: prompt_hash(prompt),
        reference_paths: Vec::new(),
        state,
        created_at: now,
        updated_at: now,
        completed_at: None,
        output_paths: Default::default(),
        cost_estimate_usd: None,
        cost_actual_usd: None,
        uses_likeness: false,
        requires_disclosure: true,
        failure_message,
    }
}

#[derive(Debug, Deserialize, Serialize)]
struct OpenRouterSubmitResponse {
    id: String,
    #[serde(default)]
    polling_url: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

/// OpenRouter asynchronous video status response.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OpenRouterStatusResponse {
    /// Provider-side job id.
    pub id: String,
    /// Provider status token.
    pub status: String,
    /// URL for future status polls.
    #[serde(default)]
    pub polling_url: Option<String>,
    /// Authenticated content URLs returned after completion.
    #[serde(default)]
    pub unsigned_urls: Vec<String>,
    /// Provider failure message when present.
    #[serde(default)]
    pub error: Option<String>,
}

/// OpenRouter provider errors.
#[derive(Debug, thiserror::Error)]
pub enum OpenRouterError {
    /// API key is not available in the provider key store.
    #[error(
        "OpenRouter is needed for generated media. Add your OpenRouter key in Settings -> Advanced -> Provider Keys."
    )]
    MissingApiKey,
    /// OpenRouter returned a non-success response.
    #[error("OpenRouter video request failed: {0}")]
    Api(String),
    /// OpenRouter completed but did not return a content URL.
    #[error("OpenRouter video response did not include an output URL")]
    MissingOutputUrl,
    /// OS keychain lookup failed.
    #[error(transparent)]
    Secret(#[from] montage_secrets::SecretError),
    /// Local filesystem write failed.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// Registry path validation failed.
    #[error(transparent)]
    Registry(#[from] RegistryError),
    /// HTTP transport or JSON decoding failed.
    #[error(transparent)]
    Http(#[from] reqwest::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_video_model_is_seedance_2() {
        assert_eq!(DEFAULT_VIDEO_MODEL, "bytedance/seedance-2.0");
    }

    #[test]
    fn submit_response_builds_queued_record_without_output() {
        let record = record_from_submit(
            "local-job",
            "office b-roll",
            DEFAULT_VIDEO_MODEL,
            OpenRouterSubmitResponse {
                id: "provider-job".into(),
                polling_url: Some("https://openrouter.ai/api/v1/videos/provider-job".into()),
                status: Some("pending".into()),
                error: None,
            },
        );

        assert_eq!(record.provider, "openrouter");
        assert_eq!(record.model, DEFAULT_VIDEO_MODEL);
        assert_eq!(record.provider_job_id.as_deref(), Some("provider-job"));
        assert_eq!(record.state, GeneratedMediaState::Queued);
        assert!(record.output_paths.is_empty());
        assert!(record.requires_disclosure);
    }

    #[test]
    fn config_requires_api_key() {
        let err = OpenRouterVideoConfig::from_env_with(None, |_| None).unwrap_err();
        assert!(matches!(err, OpenRouterError::MissingApiKey));
    }

    #[test]
    fn config_reads_api_key_from_secret_store() {
        let config = OpenRouterVideoConfig::from_secret_store(None, |env, account| {
            assert_eq!(env, montage_secrets::env_vars::OPENROUTER_API_KEY);
            assert_eq!(account, montage_secrets::accounts::OPENROUTER_API_KEY);
            Ok(Some("secret".into()))
        })
        .unwrap();

        assert_eq!(config.api_key, "secret");
        assert_eq!(config.model, DEFAULT_VIDEO_MODEL);
    }

    #[test]
    fn completed_status_updates_record_with_local_video_output() {
        let mut record = record_from_submit(
            "local-job",
            "office b-roll",
            DEFAULT_VIDEO_MODEL,
            OpenRouterSubmitResponse {
                id: "provider-job".into(),
                polling_url: Some("https://openrouter.ai/api/v1/videos/provider-job".into()),
                status: Some("pending".into()),
                error: None,
            },
        );
        let status = OpenRouterStatusResponse {
            id: "provider-job".into(),
            status: "completed".into(),
            polling_url: Some("https://openrouter.ai/api/v1/videos/provider-job".into()),
            unsigned_urls: vec![
                "https://openrouter.ai/api/v1/videos/provider-job/content?index=0".into(),
            ],
            error: None,
        };

        apply_status_to_record(
            &mut record,
            &status,
            Some("raw/generated/openrouter/local-job.mp4".into()),
        )
        .unwrap();

        assert_eq!(record.state, GeneratedMediaState::Succeeded);
        assert_eq!(
            record.output_video_path(),
            Some("raw/generated/openrouter/local-job.mp4")
        );
        assert!(record.completed_at.is_some());
    }
}
