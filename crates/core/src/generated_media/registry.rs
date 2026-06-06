//! Project-local registry and provenance types for generated media.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Component, Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Project-relative generated-media registry location.
pub const REGISTRY_RELATIVE_PATH: &str = ".awidat/generated-media/registry.json";
/// Required project-relative prefix for generated media outputs.
pub const GENERATED_OUTPUT_PREFIX: &str = "raw/generated";

/// High-level media type produced by a generated-media job.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GeneratedArtifactKind {
    /// Moving-image output.
    Video,
    /// Still image output.
    Image,
    /// Audio output.
    Audio,
    /// Unknown artifact kind from a future registry version.
    #[serde(other)]
    Unknown,
}

/// Editorial purpose for a generated output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GeneratedWorkflowPurpose {
    /// Cutaway or cover visual for spoken content.
    Broll,
    /// Reference material for a later generation.
    Reference,
    /// Purpose not yet modeled by a dedicated variant.
    #[serde(other)]
    Other,
}

/// Generation input mode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GeneratedMode {
    /// Generate video directly from text.
    TextToVideo,
    /// Generate video from text plus a still image.
    ImageToVideo,
    /// Generate video from text plus an input video.
    VideoToVideo,
    /// Offline deterministic mode used by tests.
    Mock,
    /// Unknown generation mode from a future registry version.
    #[serde(other)]
    Unknown,
}

/// Lifecycle state of a generated-media job.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GeneratedMediaState {
    /// Prompt/options exist but no provider job has been submitted.
    Draft,
    /// Provider accepted the job but has not started work.
    Queued,
    /// Provider is actively generating output.
    Running,
    /// Output is ready and recorded in `output_paths`.
    Succeeded,
    /// Provider or local processing failed.
    Failed,
    /// User or system cancelled the job.
    Cancelled,
}

/// Durable generated-media job and provenance record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GeneratedMediaRecord {
    /// Stable local job id.
    pub job_id: String,
    /// Kind of artifact the job produces.
    pub artifact_kind: GeneratedArtifactKind,
    /// Editorial purpose for the generated output.
    pub workflow_purpose: GeneratedWorkflowPurpose,
    /// Provider id that owns the job.
    pub provider: String,
    /// Provider model id.
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Provider-side job id when the backend returns one.
    pub provider_job_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Provider polling URL for asynchronous jobs.
    pub provider_status_url: Option<String>,
    /// Input mode used for generation.
    pub generation_mode: GeneratedMode,
    /// Full prompt text. Stored project-locally for auditability.
    pub prompt: String,
    /// SHA-256 hash of `prompt`, used in approval keys and logs.
    pub prompt_hash: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    /// Project-relative reference media paths, when any.
    pub reference_paths: Vec<String>,
    /// Current job lifecycle state.
    pub state: GeneratedMediaState,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Last update timestamp.
    pub updated_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Completion timestamp when the job reached a terminal success state.
    pub completed_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    /// Named project-relative output paths, such as `video`.
    pub output_paths: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Provider cost estimate in USD when known.
    pub cost_estimate_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Final provider cost in USD when known.
    pub cost_actual_usd: Option<f64>,
    /// Whether the prompt/reference uses an identifiable likeness.
    #[serde(default)]
    pub uses_likeness: bool,
    /// Whether downstream export metadata should disclose AI generation.
    #[serde(default)]
    pub requires_disclosure: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Provider/local error message when failed.
    pub failure_message: Option<String>,
}

impl GeneratedMediaRecord {
    /// Build a succeeded mock video record for offline tests.
    pub fn new_mock_succeeded(
        job_id: impl Into<String>,
        output_video_path: impl Into<String>,
        prompt: impl Into<String>,
    ) -> Result<Self, RegistryError> {
        let output_video_path = output_video_path.into();
        validate_generated_output_path(&output_video_path)?;
        let now = Utc::now();
        let prompt = prompt.into();
        let mut output_paths = BTreeMap::new();
        output_paths.insert("video".into(), output_video_path);
        Ok(Self {
            job_id: job_id.into(),
            artifact_kind: GeneratedArtifactKind::Video,
            workflow_purpose: GeneratedWorkflowPurpose::Broll,
            provider: "mock".into(),
            model: "offline-placeholder".into(),
            provider_job_id: None,
            provider_status_url: None,
            generation_mode: GeneratedMode::Mock,
            prompt_hash: prompt_hash(&prompt),
            prompt,
            reference_paths: Vec::new(),
            state: GeneratedMediaState::Succeeded,
            created_at: now,
            updated_at: now,
            completed_at: Some(now),
            output_paths,
            cost_estimate_usd: Some(0.0),
            cost_actual_usd: Some(0.0),
            uses_likeness: false,
            requires_disclosure: false,
            failure_message: None,
        })
    }

    #[cfg(test)]
    /// Convenience constructor for tests.
    pub fn mock_succeeded(job_id: &str, output_video_path: &str, prompt: &str) -> Self {
        Self::new_mock_succeeded(job_id, output_video_path, prompt).unwrap()
    }

    /// Return the named video output path when present.
    pub fn output_video_path(&self) -> Option<&str> {
        self.output_paths.get("video").map(String::as_str)
    }
}

/// In-file collection of generated-media records.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Registry {
    #[serde(default)]
    /// All generated-media records in insertion order.
    pub records: Vec<GeneratedMediaRecord>,
}

impl Registry {
    /// Load the project registry, returning an empty registry if missing.
    pub fn load_or_default(project_root: &Path) -> Result<Self, RegistryError> {
        let path = registry_path(project_root);
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = fs::read_to_string(&path)?;
        let registry: Self = serde_json::from_str(&raw)?;
        registry.validate()?;
        Ok(registry)
    }

    /// Persist the registry to the project registry path.
    pub fn save(&self, project_root: &Path) -> Result<(), RegistryError> {
        let path = registry_path(project_root);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let raw = serde_json::to_string_pretty(self)?;
        let temp_path = path.with_extension("json.tmp");
        fs::write(&temp_path, raw)?;
        fs::rename(temp_path, path)?;
        Ok(())
    }

    /// Insert or replace a record, then persist the registry.
    pub fn upsert(
        mut self,
        project_root: &Path,
        record: GeneratedMediaRecord,
    ) -> Result<Self, RegistryError> {
        for output in record.output_paths.values() {
            validate_generated_output_path(output)?;
        }
        if let Some(existing) = self.records.iter_mut().find(|r| r.job_id == record.job_id) {
            *existing = record;
        } else {
            self.records.push(record);
        }
        self.save(project_root)?;
        Ok(self)
    }

    /// Find a record by local job id.
    pub fn get(&self, job_id: &str) -> Option<&GeneratedMediaRecord> {
        self.records.iter().find(|record| record.job_id == job_id)
    }

    fn validate(&self) -> Result<(), RegistryError> {
        for record in &self.records {
            for output in record.output_paths.values() {
                validate_generated_output_path(output)?;
            }
        }
        Ok(())
    }
}

/// Errors from reading, writing, or validating the generated-media registry.
#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    /// Filesystem I/O failed.
    #[error("generated media registry I/O failed: {0}")]
    Io(#[from] std::io::Error),
    /// Registry JSON failed to parse or serialize.
    #[error("generated media registry JSON failed: {0}")]
    Json(#[from] serde_json::Error),
    /// Output path escaped the allowed generated-media output directory.
    #[error("generated output path must be project-relative under raw/generated: {0}")]
    InvalidOutputPath(String),
}

/// Absolute path to the generated-media registry for a project.
pub fn registry_path(project_root: &Path) -> PathBuf {
    project_root.join(REGISTRY_RELATIVE_PATH)
}

/// Write the lightweight semantic sidecar for a generated-media record.
pub fn write_generated_description_sidecar(
    project_root: &Path,
    record: &GeneratedMediaRecord,
) -> Result<(), RegistryError> {
    let Some(asset_id) = record.output_video_path() else {
        return Ok(());
    };
    let sidecar_path = project_root
        .join("index")
        .join("generated-description")
        .join(format!("{asset_id}.json"));
    if let Some(parent) = sidecar_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let produced_at = record
        .completed_at
        .unwrap_or(record.updated_at)
        .to_rfc3339();
    let body = serde_json::json!({
        "indexer": "generated-description",
        "indexer_version": "0.1.0",
        "schema_version": "1",
        "asset_id": asset_id,
        "asset_sha256": record.prompt_hash,
        "produced_at": produced_at,
        "data": {
            "job_id": record.job_id,
            "provider": record.provider,
            "model": record.model,
            "prompt": record.prompt,
            "prompt_hash": record.prompt_hash,
            "artifact_kind": record.artifact_kind,
            "workflow_purpose": record.workflow_purpose,
            "visual_summary": record.prompt,
            "intended_use": record.workflow_purpose,
            "created_at": record.created_at,
            "completed_at": record.completed_at,
            "requires_disclosure": record.requires_disclosure,
            "uses_likeness": record.uses_likeness,
            "provenance": "generated_media_registry"
        }
    });
    fs::write(sidecar_path, serde_json::to_vec_pretty(&body)?)?;
    Ok(())
}

/// Return the SHA-256 hash of a prompt as lowercase hex.
pub fn prompt_hash(prompt: &str) -> String {
    let digest = Sha256::digest(prompt.as_bytes());
    format!("{digest:x}")
}

/// Validate that an output path is project-relative and under `raw/generated`.
pub fn validate_generated_output_path(path: &str) -> Result<(), RegistryError> {
    let path = Path::new(path);
    if path.is_absolute() {
        return Err(RegistryError::InvalidOutputPath(path.display().to_string()));
    }
    let mut components = path.components();
    match (components.next(), components.next()) {
        (Some(Component::Normal(raw)), Some(Component::Normal(generated)))
            if raw == "raw" && generated == "generated" => {}
        _ => return Err(RegistryError::InvalidOutputPath(path.display().to_string())),
    }
    if path
        .components()
        .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(RegistryError::InvalidOutputPath(path.display().to_string()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_round_trips_record() {
        let dir = tempfile::tempdir().unwrap();
        let record = GeneratedMediaRecord::mock_succeeded(
            "job-1",
            "raw/generated/mock/job-1.mp4",
            "quiet street",
        );

        Registry::load_or_default(dir.path())
            .unwrap()
            .upsert(dir.path(), record.clone())
            .unwrap();

        let loaded = Registry::load_or_default(dir.path()).unwrap();
        assert_eq!(loaded.get("job-1"), Some(&record));
    }

    #[test]
    fn generated_description_sidecar_uses_registry_record() {
        let dir = tempfile::tempdir().unwrap();
        let record = GeneratedMediaRecord::new_mock_succeeded(
            "gen-1",
            "raw/generated/mock/gen-1.mp4",
            "slow orbit around a product on a clean desk",
        )
        .unwrap();

        write_generated_description_sidecar(dir.path(), &record).unwrap();

        let path = dir
            .path()
            .join("index/generated-description/raw/generated/mock/gen-1.mp4.json");
        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();

        assert_eq!(value["indexer"], "generated-description");
        assert_eq!(value["asset_id"], "raw/generated/mock/gen-1.mp4");
        assert_eq!(value["data"]["job_id"], "gen-1");
        assert_eq!(value["data"]["workflow_purpose"], "broll");
        assert!(
            value["data"]["visual_summary"]
                .as_str()
                .unwrap()
                .contains("slow orbit")
        );
    }

    #[test]
    fn loading_registry_rejects_tampered_output_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = registry_path(dir.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            serde_json::json!({
                "records": [{
                    "job_id": "bad",
                    "artifact_kind": "video",
                    "workflow_purpose": "broll",
                    "provider": "mock",
                    "model": "offline-placeholder",
                    "generation_mode": "mock",
                    "prompt": "bad",
                    "prompt_hash": prompt_hash("bad"),
                    "state": "succeeded",
                    "created_at": "2026-05-24T00:00:00Z",
                    "updated_at": "2026-05-24T00:00:00Z",
                    "output_paths": { "video": "raw/generated/../outside.mp4" }
                }]
            })
            .to_string(),
        )
        .unwrap();

        assert!(matches!(
            Registry::load_or_default(dir.path()),
            Err(RegistryError::InvalidOutputPath(_))
        ));
    }

    #[test]
    fn registry_loads_missing_optional_flags_and_unknown_generated_values() {
        let dir = tempfile::tempdir().unwrap();
        let path = registry_path(dir.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            serde_json::json!({
                "records": [{
                    "job_id": "future",
                    "artifact_kind": "future_kind",
                    "workflow_purpose": "broll",
                    "provider": "future",
                    "model": "future-model",
                    "generation_mode": "future_mode",
                    "prompt": "future",
                    "prompt_hash": prompt_hash("future"),
                    "state": "succeeded",
                    "created_at": "2026-05-24T00:00:00Z",
                    "updated_at": "2026-05-24T00:00:00Z",
                    "output_paths": { "video": "raw/generated/future/job.mp4" }
                }]
            })
            .to_string(),
        )
        .unwrap();

        let loaded = Registry::load_or_default(dir.path()).unwrap();
        let record = loaded.get("future").unwrap();
        assert_eq!(record.artifact_kind, GeneratedArtifactKind::Unknown);
        assert_eq!(record.generation_mode, GeneratedMode::Unknown);
        assert!(!record.uses_likeness);
        assert!(!record.requires_disclosure);
    }

    #[test]
    fn registry_loads_unknown_workflow_purpose_as_other() {
        let dir = tempfile::tempdir().unwrap();
        let path = registry_path(dir.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            serde_json::json!({
                "records": [{
                    "job_id": "future-purpose",
                    "artifact_kind": "video",
                    "workflow_purpose": "social_cutaway",
                    "provider": "future",
                    "model": "future-model",
                    "generation_mode": "mock",
                    "prompt": "future",
                    "prompt_hash": prompt_hash("future"),
                    "state": "succeeded",
                    "created_at": "2026-05-24T00:00:00Z",
                    "updated_at": "2026-05-24T00:00:00Z",
                    "output_paths": { "video": "raw/generated/future/job.mp4" }
                }]
            })
            .to_string(),
        )
        .unwrap();

        let loaded = Registry::load_or_default(dir.path()).unwrap();
        assert_eq!(
            loaded.get("future-purpose").unwrap().workflow_purpose,
            GeneratedWorkflowPurpose::Other
        );
    }
}
