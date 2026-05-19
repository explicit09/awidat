//! Safe promotion of reviewed local fixture drafts.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use chrono::Utc;
use serde::Serialize;
use serde_json::{Value, json};

/// Review metadata supplied during fixture promotion.
pub struct PromotionOptions<'a> {
    /// Human review note.
    pub reviewer_note: &'a str,
    /// Reason this draft is worth keeping.
    pub promotion_reason: &'a str,
    /// Optional discovery manifest that produced the draft.
    pub discovery_manifest_path: Option<&'a Path>,
    /// Allow replacing an existing promoted fixture and metadata sidecar.
    pub force: bool,
}

/// Copy a reviewed draft fixture into a durable local fixture directory.
pub fn promote_fixture(
    draft_path: &Path,
    output_dir: &Path,
    options: PromotionOptions<'_>,
) -> Result<PromotionReport> {
    if !draft_path.is_file() {
        bail!("draft fixture does not exist: {}", draft_path.display());
    }
    if options.reviewer_note.trim().is_empty() {
        bail!("reviewer note must not be empty");
    }
    if options.promotion_reason.trim().is_empty() {
        bail!("promotion reason must not be empty");
    }
    let draft: Value = serde_json::from_slice(
        &fs::read(draft_path).with_context(|| format!("read {}", draft_path.display()))?,
    )
    .with_context(|| format!("parse {}", draft_path.display()))?;
    let scenario_type = draft
        .get("scenario_type")
        .and_then(Value::as_str)
        .unwrap_or("unspecified");
    let file_name = draft_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("promoted_fixture.json");
    fs::create_dir_all(output_dir)
        .with_context(|| format!("create promoted fixture directory {}", output_dir.display()))?;
    let promoted_fixture_path = output_dir.join(file_name);
    let review_metadata_path = output_dir.join(review_metadata_file_name(file_name));
    if !options.force && promoted_fixture_path.exists() {
        bail!(
            "promoted fixture already exists: {}",
            promoted_fixture_path.display()
        );
    }
    if !options.force && review_metadata_path.exists() {
        bail!(
            "promotion metadata already exists: {}",
            review_metadata_path.display()
        );
    }

    fs::copy(draft_path, &promoted_fixture_path).with_context(|| {
        format!(
            "copy {} to {}",
            draft_path.display(),
            promoted_fixture_path.display()
        )
    })?;
    let metadata = json!({
        "reviewed_at": Utc::now().to_rfc3339(),
        "source_draft_path": draft_path,
        "promoted_fixture_path": promoted_fixture_path,
        "scenario_type": scenario_type,
        "reviewer_note": options.reviewer_note,
        "promotion_reason": options.promotion_reason,
        "original_discovery_manifest_path": options.discovery_manifest_path,
        "force": options.force
    });
    fs::write(&review_metadata_path, serde_json::to_vec_pretty(&metadata)?)
        .with_context(|| format!("write {}", review_metadata_path.display()))?;

    Ok(PromotionReport {
        promoted_fixture_path,
        review_metadata_path,
        scenario_type: scenario_type.into(),
        reviewer_note: options.reviewer_note.into(),
        promotion_reason: options.promotion_reason.into(),
    })
}

/// Promotion result for CLI and JSON output.
#[derive(Debug, Serialize)]
pub struct PromotionReport {
    /// Copied fixture path.
    pub promoted_fixture_path: PathBuf,
    /// Review metadata sidecar path.
    pub review_metadata_path: PathBuf,
    /// Scenario type recorded from the draft fixture.
    pub scenario_type: String,
    /// Human review note.
    pub reviewer_note: String,
    /// Reason this draft was promoted.
    pub promotion_reason: String,
}

fn review_metadata_file_name(fixture_file_name: &str) -> String {
    fixture_file_name
        .strip_suffix(".json")
        .map(|stem| format!("{stem}.review.json"))
        .unwrap_or_else(|| format!("{fixture_file_name}.review.json"))
}
