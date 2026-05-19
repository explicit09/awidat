//! Fixture manifest reporting for local real-video acceptance corpora.

use std::path::Path;

use anyhow::{Context, Result};
use chrono::Utc;
use serde::Serialize;

use super::case::load_case_from_path;
use super::external_fixture_paths_from_path;

/// Summary of runnable acceptance fixtures at a file or directory path.
#[derive(Debug, Serialize)]
pub struct FixtureManifest {
    /// File or directory inspected.
    pub fixture_root: String,
    /// RFC3339 timestamp for the manifest.
    pub generated_at: String,
    /// Number of runnable fixture files.
    pub fixture_count: usize,
    /// Number of fixtures that obtain EDL through a generator command.
    pub generated_edl_count: usize,
    /// Number of fixtures that embed a literal final EDL.
    pub literal_edl_count: usize,
    /// Number of fixtures with transcript phrase evidence.
    pub transcript_check_count: usize,
    /// Number of fixtures with removed or kept source-range expectations.
    pub source_range_check_count: usize,
    /// Per-fixture summaries.
    pub fixtures: Vec<FixtureManifestEntry>,
}

/// One runnable fixture summary.
#[derive(Debug, Serialize)]
pub struct FixtureManifestEntry {
    /// Fixture id.
    pub id: String,
    /// Fixture file path.
    pub fixture_path: String,
    /// Objective text from the fixture.
    pub objective: String,
    /// Fixture-authored project source asset id.
    pub source_asset: String,
    /// Absolute real source path when the fixture mounts local media.
    pub source_path: Option<String>,
    /// Excerpt start in source seconds.
    pub source_start_s: Option<f64>,
    /// Excerpt duration in seconds.
    pub source_duration_s: f64,
    /// `generator` or `literal`.
    pub edl_source: String,
    /// Minimum expected output duration.
    pub duration_min_s: f64,
    /// Maximum expected output duration.
    pub duration_max_s: f64,
    /// Count of expected removed source ranges.
    pub removed_source_range_count: usize,
    /// Count of expected kept source ranges.
    pub kept_source_range_count: usize,
    /// Count of required transcript phrases.
    pub must_preserve_count: usize,
    /// Count of forbidden transcript phrases.
    pub must_remove_count: usize,
}

/// Build a manifest for one fixture file or a directory of runnable fixtures.
pub fn build_fixture_manifest(path: &Path) -> Result<FixtureManifest> {
    let fixture_paths = external_fixture_paths_from_path(path)?;
    let mut fixtures = Vec::new();
    for fixture_path in fixture_paths {
        let case = load_case_from_path(&fixture_path)
            .with_context(|| format!("summarize fixture {}", fixture_path.display()))?;
        let (must_preserve_count, must_remove_count) = case
            .expect
            .transcript
            .as_ref()
            .map(|transcript| (transcript.must_preserve.len(), transcript.must_remove.len()))
            .unwrap_or((0, 0));
        let edl_source = if case.edl_generator.is_some() {
            "generator"
        } else {
            "literal"
        };
        fixtures.push(FixtureManifestEntry {
            id: case.id,
            fixture_path: fixture_path.to_string_lossy().into_owned(),
            objective: case.objective,
            source_asset: case.project.source_asset,
            source_path: case
                .project
                .source_path
                .map(|path| path.to_string_lossy().into_owned()),
            source_start_s: case.project.source_start_s,
            source_duration_s: case.project.source_duration_s,
            edl_source: edl_source.into(),
            duration_min_s: case.expect.duration_min_s,
            duration_max_s: case.expect.duration_max_s,
            removed_source_range_count: case.expect.removed_source_ranges.len(),
            kept_source_range_count: case.expect.kept_source_ranges.len(),
            must_preserve_count,
            must_remove_count,
        });
    }
    let generated_edl_count = fixtures
        .iter()
        .filter(|fixture| fixture.edl_source == "generator")
        .count();
    let literal_edl_count = fixtures
        .iter()
        .filter(|fixture| fixture.edl_source == "literal")
        .count();
    let transcript_check_count = fixtures
        .iter()
        .filter(|fixture| fixture.must_preserve_count > 0 || fixture.must_remove_count > 0)
        .count();
    let source_range_check_count = fixtures
        .iter()
        .filter(|fixture| {
            fixture.removed_source_range_count > 0 || fixture.kept_source_range_count > 0
        })
        .count();
    Ok(FixtureManifest {
        fixture_root: path.to_string_lossy().into_owned(),
        generated_at: Utc::now().to_rfc3339(),
        fixture_count: fixtures.len(),
        generated_edl_count,
        literal_edl_count,
        transcript_check_count,
        source_range_check_count,
        fixtures,
    })
}
