use awidat_render::MediaProbe;
use serde::Serialize;
use serde_json::json;

use super::case::{
    AcceptanceCase, AcceptanceTranscriptExpect, DEFAULT_SOURCE_RANGE_TOLERANCE_S,
    SourceRangeExpectation, TranscriptSegment,
};
use super::scorecard::{AcceptanceGateResult, push_gate};
use super::timeline::{AcceptanceEditManifest, FinalKeptSourceRange};

pub(crate) fn add_media_probe_gates(
    gates: &mut Vec<AcceptanceGateResult>,
    case: &AcceptanceCase,
    probe: &MediaProbe,
) {
    push_gate(
        gates,
        "has_video_stream",
        probe.has_video,
        "rendered output contains at least one video stream",
        json!({"stream_types": probe.stream_types}),
    );
    push_gate(
        gates,
        "has_audio_stream",
        probe.has_audio,
        "rendered output contains at least one audio stream",
        json!({"stream_types": probe.stream_types}),
    );

    let duration_ok = probe
        .duration_s
        .map(|duration| {
            duration >= case.expect.duration_min_s && duration <= case.expect.duration_max_s
        })
        .unwrap_or(false);
    push_gate(
        gates,
        "duration_bounds",
        duration_ok,
        "rendered duration is inside scenario bounds",
        json!({
            "duration_s": probe.duration_s,
            "min_s": case.expect.duration_min_s,
            "max_s": case.expect.duration_max_s,
        }),
    );
}

pub(crate) fn add_source_range_gates(
    gates: &mut Vec<AcceptanceGateResult>,
    case: &AcceptanceCase,
    edit: &AcceptanceEditManifest,
) {
    if case.expect.removed_source_ranges.is_empty() && case.expect.kept_source_ranges.is_empty() {
        return;
    }

    let report = evaluate_source_range_expectations(
        &edit.final_kept_source_ranges,
        &case.expect.removed_source_ranges,
        &case.expect.kept_source_ranges,
    );
    if !case.expect.removed_source_ranges.is_empty() {
        push_gate(
            gates,
            "removed_source_ranges_absent",
            report.removed_passed,
            "expected removed source ranges do not appear in final kept timeline ranges",
            json!({
                "removed": report.removed,
                "final_kept_source_ranges": edit.final_kept_source_ranges,
            }),
        );
    }
    if !case.expect.kept_source_ranges.is_empty() {
        push_gate(
            gates,
            "kept_source_ranges_present",
            report.preserved_passed,
            "expected preserved source ranges are still covered by the final timeline",
            json!({
                "preserved": report.preserved,
                "final_kept_source_ranges": edit.final_kept_source_ranges,
            }),
        );
    }
}

pub(crate) fn evaluate_source_range_expectations(
    kept_ranges: &[FinalKeptSourceRange],
    removed: &[SourceRangeExpectation],
    preserved: &[SourceRangeExpectation],
) -> SourceRangeExpectationReport {
    let removed = removed
        .iter()
        .map(|expected| {
            let overlap_s = source_range_coverage_s(expected, kept_ranges);
            let tolerance_s = expected
                .tolerance_s
                .unwrap_or(DEFAULT_SOURCE_RANGE_TOLERANCE_S);
            SourceRangeCheck {
                start_s: expected.start_s,
                end_s: expected.end_s,
                reason: expected.reason.clone(),
                tolerance_s,
                overlap_s,
                coverage_s: 0.0,
                missing_s: 0.0,
                status: if overlap_s <= tolerance_s {
                    "pass".into()
                } else {
                    "fail".into()
                },
            }
        })
        .collect::<Vec<_>>();
    let preserved = preserved
        .iter()
        .map(|expected| {
            let coverage_s = source_range_coverage_s(expected, kept_ranges);
            let duration_s = (expected.end_s - expected.start_s).max(0.0);
            let missing_s = (duration_s - coverage_s).max(0.0);
            let tolerance_s = expected
                .tolerance_s
                .unwrap_or(DEFAULT_SOURCE_RANGE_TOLERANCE_S);
            SourceRangeCheck {
                start_s: expected.start_s,
                end_s: expected.end_s,
                reason: expected.reason.clone(),
                tolerance_s,
                overlap_s: 0.0,
                coverage_s,
                missing_s,
                status: if missing_s <= tolerance_s {
                    "pass".into()
                } else {
                    "fail".into()
                },
            }
        })
        .collect::<Vec<_>>();
    SourceRangeExpectationReport {
        removed_passed: removed.iter().all(|check| check.status == "pass"),
        preserved_passed: preserved.iter().all(|check| check.status == "pass"),
        removed,
        preserved,
    }
}

fn source_range_coverage_s(
    expected: &SourceRangeExpectation,
    kept_ranges: &[FinalKeptSourceRange],
) -> f64 {
    let expected_duration_s = (expected.end_s - expected.start_s).max(0.0);
    kept_ranges
        .iter()
        .map(|kept| {
            overlap_s(
                expected.start_s,
                expected.end_s,
                kept.source_start_s,
                kept.source_end_s,
            )
        })
        .sum::<f64>()
        .min(expected_duration_s)
}

fn overlap_s(left_start_s: f64, left_end_s: f64, right_start_s: f64, right_end_s: f64) -> f64 {
    (left_end_s.min(right_end_s) - left_start_s.max(right_start_s)).max(0.0)
}

pub(crate) fn build_transcript_evidence(
    transcript: &AcceptanceTranscriptExpect,
    kept_ranges: &[FinalKeptSourceRange],
) -> TranscriptEvidence {
    let final_kept_segments = transcript
        .segments
        .iter()
        .filter(|segment| {
            kept_ranges.iter().any(|kept| {
                overlap_s(
                    segment.start_s,
                    segment.end_s,
                    kept.source_start_s,
                    kept.source_end_s,
                ) > 0.0
            })
        })
        .cloned()
        .collect::<Vec<_>>();
    let final_kept_text = final_kept_segments
        .iter()
        .map(|segment| segment.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    let normalized_final = normalize_transcript_text(&final_kept_text);
    let preserve_checks = transcript
        .must_preserve
        .iter()
        .map(|phrase| {
            let passed = normalized_final.contains(&normalize_transcript_text(phrase));
            TranscriptPhraseCheck {
                phrase: phrase.clone(),
                status: if passed { "pass".into() } else { "fail".into() },
            }
        })
        .collect::<Vec<_>>();
    let remove_checks = transcript
        .must_remove
        .iter()
        .map(|phrase| {
            let passed = !normalized_final.contains(&normalize_transcript_text(phrase));
            TranscriptPhraseCheck {
                phrase: phrase.clone(),
                status: if passed { "pass".into() } else { "fail".into() },
            }
        })
        .collect::<Vec<_>>();
    TranscriptEvidence {
        final_kept_segments,
        final_kept_text,
        preserve_passed: preserve_checks.iter().all(|check| check.status == "pass"),
        remove_passed: remove_checks.iter().all(|check| check.status == "pass"),
        preserve_checks,
        remove_checks,
    }
}

fn normalize_transcript_text(text: &str) -> String {
    let mut normalized = String::with_capacity(text.len());
    let mut previous_was_space = true;
    for ch in text.chars().flat_map(char::to_lowercase) {
        if ch.is_alphanumeric() {
            normalized.push(ch);
            previous_was_space = false;
        } else if !previous_was_space {
            normalized.push(' ');
            previous_was_space = true;
        }
    }
    normalized.trim().to_string()
}
#[derive(Debug, Serialize)]
pub(crate) struct SourceRangeExpectationReport {
    pub(crate) removed_passed: bool,
    pub(crate) preserved_passed: bool,
    pub(crate) removed: Vec<SourceRangeCheck>,
    pub(crate) preserved: Vec<SourceRangeCheck>,
}

#[derive(Debug, Serialize)]
pub(crate) struct SourceRangeCheck {
    pub(crate) start_s: f64,
    pub(crate) end_s: f64,
    pub(crate) reason: Option<String>,
    pub(crate) tolerance_s: f64,
    pub(crate) overlap_s: f64,
    pub(crate) coverage_s: f64,
    pub(crate) missing_s: f64,
    pub(crate) status: String,
}
#[derive(Debug, Serialize)]
pub(crate) struct TranscriptEvidence {
    pub(crate) final_kept_segments: Vec<TranscriptSegment>,
    pub(crate) final_kept_text: String,
    pub(crate) preserve_passed: bool,
    pub(crate) remove_passed: bool,
    pub(crate) preserve_checks: Vec<TranscriptPhraseCheck>,
    pub(crate) remove_checks: Vec<TranscriptPhraseCheck>,
}

#[derive(Debug, Serialize)]
pub(crate) struct TranscriptPhraseCheck {
    pub(crate) phrase: String,
    pub(crate) status: String,
}
