//! Utilities for local review package authoring.
//!
//! A "review package" here is a lightweight JSON artifact that links a
//! rendered path back to the authoritative vedit commit metadata and
//! optional editor rationale. It is intentionally local-only and does not
//! depend on external review services or upstream sync.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::vc;

/// Relative directory under `.awidat/` that stores local review packages.
pub const REVIEW_PACKAGES_DIR: &str = "review-packages";
/// JSON filename pattern extension.
const REVIEW_PACKAGE_EXT: &str = "json";

/// Metadata persisted for a local review package.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalReviewPackage {
    /// Path to the review render that this package points to.
    pub render_path: String,
    /// Timestamp when this package was generated.
    pub generated_at: String,
    /// Commit hash this review package references.
    pub vedit_commit: String,
    /// Timeline hash for fast identity checks.
    pub timeline_hash: String,
    /// Optional package tags.
    pub tags: Vec<String>,
    /// Header from the referenced vedit commit.
    pub commit_header: String,
    /// Parsed commit body text (Agent reasoning) if present.
    pub reasoning_body: String,
    /// Absolute path of the persisted package file.
    pub package_path: String,
}

/// Build and persist a local review package entry for `render_path`.
pub fn build_local_review_package(
    project_root: &Path,
    render_path: &str,
    tags: Vec<String>,
) -> Result<LocalReviewPackage, String> {
    let render_path = resolve_render_path(project_root, render_path)?;
    let repo = vc::open_or_init(project_root).map_err(|e| format!("open vedit repo: {e}"))?;
    let entries = vc::log(&repo, 1).map_err(|e| format!("read vedit log: {e}"))?;
    let latest = entries.into_iter().next().ok_or_else(|| {
        "no vedit commits available; run vedit_commit before authoring a review package".to_string()
    })?;

    let package = LocalReviewPackage {
        render_path: render_path.to_string_lossy().into_owned(),
        generated_at: Utc::now().to_rfc3339(),
        vedit_commit: latest.commit_hash,
        timeline_hash: latest.timeline_hash,
        tags: normalize_tags(tags),
        commit_header: latest.header,
        reasoning_body: extract_reasoning_body(&latest.full_message),
        package_path: String::new(),
    };
    let package_path = write_local_review_package(project_root, &package)?;
    Ok(LocalReviewPackage {
        package_path: package_path.to_string_lossy().into_owned(),
        ..package
    })
}

fn resolve_render_path(project_root: &Path, raw: &str) -> Result<PathBuf, String> {
    let candidate = PathBuf::from(raw);
    let resolved = if candidate.is_absolute() {
        candidate
    } else {
        project_root.join(candidate)
    };
    if resolved.is_file() {
        Ok(resolved)
    } else {
        Err(format!(
            "review render path not found or not a file: {}",
            resolved.display()
        ))
    }
}

fn extract_reasoning_body(message: &str) -> String {
    let marker = "\nAgent reasoning:";
    if let Some(idx) = message.find(marker) {
        message[idx + marker.len()..]
            .trim_start()
            .trim()
            .to_string()
    } else {
        String::new()
    }
}

fn normalize_tags(raw_tags: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut tags = Vec::new();
    for tag in raw_tags {
        let trimmed = tag.trim().to_string();
        if trimmed.is_empty() || !seen.insert(trimmed.clone()) {
            continue;
        }
        tags.push(trimmed);
    }
    tags
}

fn write_local_review_package(
    project_root: &Path,
    package: &LocalReviewPackage,
) -> Result<PathBuf, String> {
    let package_dir = project_root.join(".awidat").join(REVIEW_PACKAGES_DIR);
    std::fs::create_dir_all(&package_dir)
        .map_err(|e| format!("create review package dir {}: {e}", package_dir.display()))?;
    let short_commit = package
        .vedit_commit
        .strip_prefix("sha256:")
        .unwrap_or(&package.vedit_commit);
    let short_commit = short_commit.chars().take(8).collect::<String>();
    let safe_ts = package.generated_at.replace([':', '/'], "-");
    let filename = format!("review-{safe_ts}-{short_commit}.{REVIEW_PACKAGE_EXT}");
    let path = package_dir.join(filename);
    let payload =
        serde_json::to_vec_pretty(package).map_err(|e| format!("serialize review package: {e}"))?;
    std::fs::write(&path, payload)
        .map_err(|e| format!("write review package {}: {e}", path.display()))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vc;
    use std::fs;
    use tempfile::tempdir;

    fn write_minimal_otio(project_root: &std::path::Path) {
        let v = serde_json::json!({
            "OTIO_SCHEMA": "Timeline.1",
            "name": "test",
            "tracks": {
                "OTIO_SCHEMA": "Stack.1",
                "name": "tracks",
                "children": []
            }
        });
        let path = project_root.join("project.otio.json");
        fs::write(&path, serde_json::to_vec_pretty(&v).unwrap()).unwrap();
    }

    fn commit_with_reasoning(project_root: &std::path::Path, header: &str, body: &str) {
        write_minimal_otio(project_root);
        let repo = vc::open_or_init(project_root).unwrap();
        vc::commit_current_timeline(&repo, header, Some(body)).unwrap();
    }

    #[test]
    fn builds_and_writes_a_local_review_package() {
        let project = tempdir().unwrap();
        commit_with_reasoning(
            project.path(),
            "Local review baseline",
            "Brand new render for review",
        );
        let render = project.path().join("renders").join("render.mp4");
        fs::create_dir_all(render.parent().unwrap()).unwrap();
        fs::write(&render, "binary").unwrap();

        let package = build_local_review_package(
            project.path(),
            "renders/render.mp4",
            vec![
                "review".into(),
                " review ".into(),
                "".into(),
                "review".into(),
            ],
        )
        .unwrap();

        assert_eq!(package.commit_header, "Local review baseline");
        assert_eq!(package.reasoning_body, "Brand new render for review");
        assert_eq!(package.tags, vec!["review"]);
        assert_eq!(package.render_path, render.to_string_lossy());
        assert!(package.package_path.ends_with(".json"));
        assert!(std::path::Path::new(&package.package_path).is_file());
    }

    #[test]
    fn errors_when_render_is_missing() {
        let project = tempdir().unwrap();
        commit_with_reasoning(project.path(), "Local review baseline", "Body");

        let err =
            build_local_review_package(project.path(), "renders/missing.mp4", vec![]).unwrap_err();
        assert!(err.contains("not found") || err.contains("not a file"));
    }

    #[test]
    fn errors_when_no_vedit_history() {
        let project = tempdir().unwrap();
        write_minimal_otio(project.path());

        let render = project.path().join("renders").join("render.mp4");
        fs::create_dir_all(render.parent().unwrap()).unwrap();
        fs::write(&render, "binary").unwrap();

        let err =
            build_local_review_package(project.path(), "renders/render.mp4", vec![]).unwrap_err();
        assert!(err.contains("no vedit commits"));
    }
}
