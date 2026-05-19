//! Fixture draft generation for discovered real-video candidates.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::{Value, json};

use super::discover::{DiscoveryCandidate, DiscoveryOptions, DiscoveryReport};

/// Result of writing discovery fixture drafts to disk.
#[derive(Debug)]
pub struct FixtureDraftWriteReport {
    /// New fixture draft files written during this call.
    pub written: Vec<PathBuf>,
    /// Existing fixture draft files left untouched.
    pub skipped_existing: Vec<PathBuf>,
    /// Review manifest written beside the fixture drafts.
    pub manifest_path: PathBuf,
}

/// Build an authoring-ready acceptance fixture draft from discovery evidence.
pub(crate) fn build_fixture_draft(
    candidate: &DiscoveryCandidate,
    options: &DiscoveryOptions,
) -> Option<Value> {
    if candidate.suggested_behavior != "dead_air_cleanup" || candidate.long_silences.is_empty() {
        return None;
    }
    let source_duration_s = candidate
        .duration_s
        .unwrap_or(options.sample_duration_s)
        .min(options.sample_duration_s);
    let removed_duration_s = candidate
        .long_silences
        .iter()
        .map(|silence| silence.duration_s)
        .sum::<f64>();
    let duration_min_s = (source_duration_s - removed_duration_s - 1.0).max(0.1);
    let duration_max_s = source_duration_s + 0.5;
    let removed_source_ranges = candidate
        .long_silences
        .iter()
        .map(|silence| {
            json!({
                "start_s": round_millis(silence.start_s),
                "end_s": round_millis(silence.end_s),
                "reason": "detected dead air",
                "tolerance_s": 0.15
            })
        })
        .collect::<Vec<_>>();
    Some(json!({
        "id": candidate.fixture_id_hint,
        "objective": "Generate and apply a dead-air cleanup EDL for a discovered real video candidate.",
        "scenario_type": candidate.suggested_behavior,
        "project": {
            "name": fixture_project_name(&candidate.fixture_id_hint),
            "source_asset": "raw/source.mp4",
            "source_path": candidate.path,
            "source_start_s": 0.0,
            "source_duration_s": round_millis(source_duration_s)
        },
        "edl_generator": {
            "command": [
                "/bin/sh",
                "-c",
                format!(
                    "exec \"${{AWIDAT_ACCEPTANCE_CLI:-awidat}}\" plan-dead-air-edl \"$AWIDAT_ACCEPTANCE_PROJECT_ROOT\" --min-duration-s {} --silence-threshold-db {}",
                    options.min_silence_s,
                    options.silence_threshold_db
                )
            ]
        },
        "expect": {
            "duration_min_s": round_millis(duration_min_s),
            "duration_max_s": round_millis(duration_max_s),
            "max_long_silence_s": options.min_silence_s,
            "silence_threshold_db": options.silence_threshold_db,
            "max_black_segment_s": 0.2,
            "black_min_duration_s": 0.2,
            "min_clip_count": 1,
            "edl_must_contain": ["*** Trim Clip"],
            "removed_source_ranges": removed_source_ranges
        }
    }))
}

/// Write embedded fixture drafts from a discovery report to an output directory.
pub fn write_fixture_drafts(
    report: &DiscoveryReport,
    output_dir: &Path,
) -> Result<FixtureDraftWriteReport> {
    fs::create_dir_all(output_dir)
        .with_context(|| format!("create fixture draft directory {}", output_dir.display()))?;
    let mut written = Vec::new();
    let mut skipped_existing = Vec::new();
    let mut manifest_drafts = Vec::new();

    for (index, candidate) in report.candidates.iter().enumerate() {
        let Some(draft) = &candidate.fixture_draft else {
            continue;
        };
        let file_stem = draft
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or(candidate.fixture_id_hint.as_str());
        let path = output_dir.join(format!("{}.json", fixture_file_stem(file_stem)));
        let status = if path.exists() {
            skipped_existing.push(path.clone());
            "skipped_existing"
        } else {
            let mut bytes = serde_json::to_vec_pretty(draft)
                .with_context(|| format!("encode fixture draft {file_stem}"))?;
            bytes.push(b'\n');
            fs::write(&path, bytes)
                .with_context(|| format!("write fixture draft {}", path.display()))?;
            written.push(path.clone());
            "written"
        };
        let rank = index + 1;
        let ranking_signals = ranking_signals(candidate, report);
        manifest_drafts.push(json!({
            "id": file_stem,
            "rank": rank,
            "fixture_path": path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("fixture.json"),
            "source_path": candidate.path,
            "scenario_type": candidate.suggested_behavior,
            "suggested_behavior": candidate.suggested_behavior,
            "score": candidate.score,
            "duration_s": candidate.duration_s,
            "long_silence_count": candidate.long_silences.len(),
            "ranking_signals": ranking_signals,
            "ranking_reason": ranking_reason(candidate, report, rank),
            "status": status
        }));
    }
    let manifest_path = output_dir.join("fixture_drafts_manifest.json");
    let manifest = json!({
        "source_root": report.source_root,
        "generated_at": report.generated_at,
        "discovery": {
            "scanned_files": report.scanned_files,
            "media_candidates": report.candidates.len(),
            "transcript_candidates": report.transcript_candidates.len(),
            "errors": report.errors.len()
        },
        "draft_count": manifest_drafts.len(),
        "written_count": written.len(),
        "skipped_existing_count": skipped_existing.len(),
        "drafts": manifest_drafts
    });
    let mut bytes =
        serde_json::to_vec_pretty(&manifest).context("encode fixture draft manifest")?;
    bytes.push(b'\n');
    fs::write(&manifest_path, bytes)
        .with_context(|| format!("write fixture draft manifest {}", manifest_path.display()))?;

    Ok(FixtureDraftWriteReport {
        written,
        skipped_existing,
        manifest_path,
    })
}

fn fixture_project_name(fixture_id: &str) -> String {
    fixture_id
        .strip_prefix("acceptance::")
        .unwrap_or(fixture_id)
        .replace('_', "-")
}

fn fixture_file_stem(fixture_id: &str) -> String {
    let raw = fixture_id
        .strip_prefix("acceptance::")
        .unwrap_or(fixture_id);
    let sanitized = raw
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim_matches('_')
        .to_string();
    if sanitized.is_empty() {
        "fixture".into()
    } else {
        sanitized
    }
}

fn ranking_signals(candidate: &DiscoveryCandidate, report: &DiscoveryReport) -> Value {
    let long_silence_total_s = long_silence_total_s(candidate);
    let sample_window_s = sample_window_s(candidate, report);
    let silence_density = if sample_window_s > 0.0 {
        long_silence_total_s / sample_window_s
    } else {
        0.0
    };
    json!({
        "scenario_type": candidate.suggested_behavior,
        "score": candidate.score,
        "duration_s": candidate.duration_s,
        "sample_window_s": round_millis(sample_window_s),
        "long_silence_count": candidate.long_silences.len(),
        "long_silence_total_s": round_millis(long_silence_total_s),
        "silence_density": round_millis(silence_density),
        "source_path_confidence": source_path_confidence(&candidate.path),
        "expected_usefulness": expected_usefulness(candidate, silence_density),
        "transcript_cleanup_evidence": {
            "available": false,
            "indicator_count": 0,
            "reason": "media discovery draft has no transcript-sidecar cleanup signal attached"
        }
    })
}

fn ranking_reason(candidate: &DiscoveryCandidate, report: &DiscoveryReport, rank: usize) -> String {
    let long_silence_total_s = long_silence_total_s(candidate);
    let sample_window_s = sample_window_s(candidate, report);
    let silence_density = if sample_window_s > 0.0 {
        long_silence_total_s / sample_window_s
    } else {
        0.0
    };
    let silence_label = if candidate.long_silences.len() == 1 {
        "1 long silence".to_string()
    } else {
        format!("{} long silences", candidate.long_silences.len())
    };
    format!(
        "Rank {rank}: {} draft with {silence_label} totaling {:.3}s ({:.1}% of sampled duration); source path confidence {}; expected usefulness {}.",
        candidate.suggested_behavior,
        round_millis(long_silence_total_s),
        round_millis(silence_density * 100.0),
        source_path_confidence(&candidate.path),
        expected_usefulness(candidate, silence_density)
    )
}

fn long_silence_total_s(candidate: &DiscoveryCandidate) -> f64 {
    candidate
        .long_silences
        .iter()
        .map(|silence| silence.duration_s)
        .sum()
}

fn sample_window_s(candidate: &DiscoveryCandidate, report: &DiscoveryReport) -> f64 {
    candidate
        .duration_s
        .unwrap_or(report.options.sample_duration_s)
        .min(report.options.sample_duration_s)
        .max(0.0)
}

fn source_path_confidence(source_path: &str) -> &'static str {
    let path = Path::new(source_path);
    match (path.is_absolute(), path.exists()) {
        (true, true) => "absolute_existing",
        (true, false) => "absolute_missing",
        (false, true) => "relative_existing",
        (false, false) => "relative_unverified",
    }
}

fn expected_usefulness(candidate: &DiscoveryCandidate, silence_density: f64) -> &'static str {
    if candidate.score >= 30.0 && !candidate.long_silences.is_empty() && silence_density >= 0.03 {
        "high"
    } else if candidate.score >= 10.0 && !candidate.long_silences.is_empty() {
        "medium"
    } else {
        "low"
    }
}

fn round_millis(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}
