//! Tauri commands for project-local generated media records.

use montage_core::generated_media::registry::{
    GeneratedMediaRecord, Registry, validate_generated_output_path,
};
use serde::Serialize;
use tauri::State;

use crate::state::MontageState;

/// Generated-media record projected to the desktop media rail.
#[derive(Debug, Clone, Serialize)]
pub struct GeneratedMediaEntry {
    /// Local generated-media job id.
    pub job_id: String,
    /// Artifact kind, serialized as a compact display token.
    pub artifact_kind: String,
    /// Editorial workflow purpose for the generated asset.
    pub workflow_purpose: String,
    /// Provider id that produced or owns the job.
    pub provider: String,
    /// Provider model id.
    pub model: String,
    /// Current job state.
    pub state: String,
    /// Prompt excerpt suitable for a compact card.
    pub prompt_excerpt: String,
    /// Full prompt hash for audit/debug detail.
    pub prompt_hash: String,
    /// Project-relative video output path when available.
    pub video_path: Option<String>,
    /// Absolute video output path when available.
    pub absolute_video_path: Option<String>,
    /// Whether the generated asset used an identifiable likeness.
    pub uses_likeness: bool,
    /// Whether AI-generation disclosure should be preserved downstream.
    pub requires_disclosure: bool,
    /// Estimated cost in USD when known.
    pub cost_estimate_usd: Option<f64>,
    /// Final cost in USD when known.
    pub cost_actual_usd: Option<f64>,
    /// Provider/local failure message when the job failed.
    pub failure_message: Option<String>,
}

/// List generated-media registry records for the active project.
#[tauri::command]
pub async fn list_generated_media(
    state: State<'_, MontageState>,
) -> Result<Vec<GeneratedMediaEntry>, String> {
    let project_root = match state.project_root.lock().await.clone() {
        Some(root) => root,
        None => return Ok(Vec::new()),
    };
    tokio::task::spawn_blocking(move || {
        let registry = Registry::load_or_default(&project_root)
            .map_err(|e| format!("generated media: {e}"))?;
        let mut entries = Vec::with_capacity(registry.records.len());
        for record in registry.records {
            entries.push(entry_from_record(&project_root, record)?);
        }
        Ok(entries)
    })
    .await
    .map_err(|e| format!("list generated media join: {e}"))?
}

fn entry_from_record(
    project_root: &std::path::Path,
    record: GeneratedMediaRecord,
) -> Result<GeneratedMediaEntry, String> {
    let video_path = record.output_video_path().map(str::to_string);
    let absolute_video_path = match video_path.as_deref() {
        Some(path) => {
            validate_generated_output_path(path).map_err(|e| format!("generated media: {e}"))?;
            Some(project_root.join(path).display().to_string())
        }
        None => None,
    };
    Ok(GeneratedMediaEntry {
        job_id: record.job_id,
        artifact_kind: serde_variant(&record.artifact_kind),
        workflow_purpose: serde_variant(&record.workflow_purpose),
        provider: record.provider,
        model: record.model,
        state: serde_variant(&record.state),
        prompt_excerpt: prompt_excerpt(&record.prompt),
        prompt_hash: record.prompt_hash,
        video_path,
        absolute_video_path,
        uses_likeness: record.uses_likeness,
        requires_disclosure: record.requires_disclosure,
        cost_estimate_usd: record.cost_estimate_usd,
        cost_actual_usd: record.cost_actual_usd,
        failure_message: record.failure_message,
    })
}

fn serde_variant<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_else(|| "unknown".into())
}

fn prompt_excerpt(prompt: &str) -> String {
    let prompt = prompt.trim();
    const MAX_CHARS: usize = 140;
    if prompt.chars().count() <= MAX_CHARS {
        return prompt.to_string();
    }
    let mut excerpt: String = prompt.chars().take(MAX_CHARS - 1).collect();
    excerpt.push('…');
    excerpt
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_excerpt_truncates_by_chars() {
        let prompt = "x".repeat(180);
        let excerpt = prompt_excerpt(&prompt);
        assert_eq!(excerpt.chars().count(), 140);
        assert!(excerpt.ends_with('…'));
    }

    #[test]
    fn entry_projects_video_path() {
        let dir = tempfile::tempdir().unwrap();
        let record = GeneratedMediaRecord::new_mock_succeeded(
            "job-1",
            "raw/generated/mock/job-1.mp4",
            "quiet office",
        )
        .unwrap();
        let entry = entry_from_record(dir.path(), record).unwrap();
        assert_eq!(entry.job_id, "job-1");
        assert_eq!(entry.state, "succeeded");
        assert_eq!(
            entry.video_path.as_deref(),
            Some("raw/generated/mock/job-1.mp4")
        );
        assert!(
            entry
                .absolute_video_path
                .unwrap()
                .contains("raw/generated/mock/job-1.mp4")
        );
    }
}
