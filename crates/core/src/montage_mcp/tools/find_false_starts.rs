//! `find_false_starts` — surface places where the speaker began a
//! thought, abandoned it, and restarted. Ported from
//! `crates/core/src/tools/find_false_starts.rs` to the in-process
//! MCP server.
//!
//! Heuristic in v1 (gated by Phase 1 user reaction; the hard version
//! using a continuity model is Phase 4):
//!
//! 1. **Abrupt cut**: a word whose end is followed by a silence
//!    range ≥ 0.4s starting within 1s, AND the next word is not a
//!    continuation of the same thought.
//! 2. **Restart marker**: the speaker says one of {"wait", "let
//!    me", "actually"} mid-utterance. We surface the *prior*
//!    fragment as the false start.

use std::path::{Path, PathBuf};

use montage_proto::otio::{MediaReference, StackChild, Timeline, TrackChild};
use montage_proto::project::Project;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::montage_mcp::context::McpToolCtx;
use crate::transcript_cleanup::{
    TranscriptSegment, TranscriptWord, false_start_ranges, production_aside_ranges,
};

/// Default cap on findings.
const DEFAULT_MAX_RESULTS: usize = 20;
const HARD_MAX_RESULTS: usize = 100;

/// Arguments to `find_false_starts`.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct FindFalseStartsArgs {
    /// Max findings to return. Default 20, hard cap 100.
    #[serde(default)]
    pub max_results: Option<usize>,
}

/// Run `find_false_starts` against the project resolved from
/// [`McpToolCtx`]. Returns the JSON body as `Ok(String)`;
/// project-read errors return `Err(String)`.
pub fn run(args: FindFalseStartsArgs, ctx: McpToolCtx) -> Result<String, String> {
    let max_results = args
        .max_results
        .unwrap_or(DEFAULT_MAX_RESULTS)
        .min(HARD_MAX_RESULTS);

    let project = Project::read(&ctx.project_root)
        .map_err(|e| format!("find_false_starts: failed to read project: {e}"))?;

    // Honor per-project dismissal: if the user dismissed
    // false-start findings in a prior session, return empty.
    let dismissals = crate::dismissal::load_dismissals(&ctx.project_root);
    if dismissals.is_dismissed(crate::dismissal::DismissalBucket::FalseStart) {
        let body = serde_json::json!({
            "findings": Vec::<FalseStartFinding>::new(),
            "more_available": false,
            "dismissed": true,
        });
        return Ok(body.to_string());
    }

    let findings = scan_false_starts(&ctx.project_root, &project.timeline, max_results);
    let more_available = findings.len() == max_results;
    let body = serde_json::json!({
        "findings": findings,
        "more_available": more_available,
    });
    Ok(body.to_string())
}

/// Walk the timeline + each clip's whisper sidecar; emit a finding
/// for every restart-marker hit. Pure function.
pub fn scan_false_starts(
    project_root: &Path,
    timeline: &Timeline,
    max_results: usize,
) -> Vec<FalseStartFinding> {
    let mut out: Vec<FalseStartFinding> = Vec::new();
    let timeline_cursor_s = 0.0_f64;

    for stack_child in &timeline.tracks.children {
        let StackChild::Track(track) = stack_child else {
            continue;
        };
        let mut track_cursor_s = 0.0_f64;
        for tc in &track.children {
            match tc {
                TrackChild::Clip(clip) => {
                    let MediaReference::External(ext) = &clip.media_reference else {
                        if let Some(r) = clip.source_range.as_ref() {
                            track_cursor_s += r.duration.to_seconds();
                        }
                        continue;
                    };
                    let Some(range) = clip.source_range.as_ref() else {
                        continue;
                    };
                    let asset_id = ext.target_url.clone();
                    let clip_source_start = range.start_time.to_seconds();
                    let clip_source_end = clip_source_start + range.duration.to_seconds();
                    let clip_track_start = track_cursor_s;

                    if let Some(words) = load_whisper_words(project_root, &asset_id) {
                        scan_words_for_restarts(
                            &words,
                            &asset_id,
                            clip_source_start,
                            clip_source_end,
                            timeline_cursor_s + clip_track_start,
                            &mut out,
                        );
                        if out.len() >= max_results {
                            out.truncate(max_results);
                            return out;
                        }
                    }
                    if let Some(segments) = load_whisper_segments(project_root, &asset_id) {
                        scan_segments_for_production_asides(
                            &segments,
                            &asset_id,
                            clip_source_start,
                            clip_source_end,
                            timeline_cursor_s + clip_track_start,
                            &mut out,
                        );
                        if out.len() >= max_results {
                            out.truncate(max_results);
                            return out;
                        }
                    }
                    track_cursor_s += range.duration.to_seconds();
                }
                TrackChild::Gap(gap) => {
                    track_cursor_s += gap.source_range.duration.to_seconds();
                }
                TrackChild::Transition(_) | TrackChild::Stack(_) => {}
            }
        }
    }
    out
}

/// Per-clip restart detection. Consumes the whisper words list and
/// pushes findings into `out`. Separated so unit tests can drive
/// the heuristic without spinning up a full Timeline.
fn scan_words_for_restarts(
    words: &[TranscriptWord],
    asset_id: &str,
    clip_source_start: f64,
    clip_source_end: f64,
    timeline_offset: f64,
    out: &mut Vec<FalseStartFinding>,
) {
    for range in false_start_ranges(words, clip_source_start, clip_source_end) {
        out.push(FalseStartFinding {
            asset_id: asset_id.to_string(),
            marker: range.marker,
            source_start_s: range.start_s,
            source_end_s: range.end_s,
            timeline_start_s: timeline_offset + (range.start_s - clip_source_start),
            timeline_end_s: timeline_offset + (range.end_s - clip_source_start),
            snippet: range.snippet,
        });
    }
}

fn scan_segments_for_production_asides(
    segments: &[TranscriptSegment],
    asset_id: &str,
    clip_source_start: f64,
    clip_source_end: f64,
    timeline_offset: f64,
    out: &mut Vec<FalseStartFinding>,
) {
    for range in production_aside_ranges(segments, clip_source_start, clip_source_end) {
        out.push(FalseStartFinding {
            asset_id: asset_id.to_string(),
            marker: range.marker,
            source_start_s: range.start_s,
            source_end_s: range.end_s,
            timeline_start_s: timeline_offset + (range.start_s - clip_source_start),
            timeline_end_s: timeline_offset + (range.end_s - clip_source_start),
            snippet: range.snippet,
        });
    }
}

/// One restart-marker hit. The fragment from `source_start_s` to
/// `source_end_s` is the false-start the user might trim; `marker`
/// is the word that triggered detection.
#[derive(Debug, Clone, Serialize)]
pub struct FalseStartFinding {
    /// Source asset.
    pub asset_id: String,
    /// The restart-marker word that triggered this finding.
    pub marker: String,
    /// Source-time start of the false-start range.
    pub source_start_s: f64,
    /// Source-time end (right before the restart marker word).
    pub source_end_s: f64,
    /// Timeline-time start of the false-start range.
    pub timeline_start_s: f64,
    /// Timeline-time end of the false-start range.
    pub timeline_end_s: f64,
    /// Short human-readable preview of the false-start text.
    pub snippet: String,
}

fn load_whisper_segments(project_root: &Path, asset_id: &str) -> Option<Vec<TranscriptSegment>> {
    let path = whisper_sidecar_path(project_root, asset_id);
    let bytes = std::fs::read(&path).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let arr = value.pointer("/data/segments").and_then(|v| v.as_array())?;
    let mut out = Vec::with_capacity(arr.len());
    for item in arr {
        let text = item
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        if text.is_empty() {
            continue;
        }
        let start_s = item
            .get("start_s")
            .or_else(|| item.get("start"))
            .and_then(|v| v.as_f64());
        let end_s = item
            .get("end_s")
            .or_else(|| item.get("end"))
            .and_then(|v| v.as_f64());
        if let (Some(s), Some(e)) = (start_s, end_s)
            && e > s
        {
            out.push(TranscriptSegment {
                text: text.to_string(),
                start_s: s,
                end_s: e,
            });
        }
    }
    Some(out)
}

fn load_whisper_words(project_root: &Path, asset_id: &str) -> Option<Vec<TranscriptWord>> {
    let path = whisper_sidecar_path(project_root, asset_id);
    let bytes = std::fs::read(&path).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let arr = value
        .pointer("/data/words")
        .or_else(|| value.pointer("/data/segments"))
        .and_then(|v| v.as_array())?;
    let mut out = Vec::with_capacity(arr.len());
    for item in arr {
        let text = item
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        if text.is_empty() {
            continue;
        }
        let start_s = item
            .get("start_s")
            .or_else(|| item.get("start"))
            .and_then(|v| v.as_f64());
        let end_s = item
            .get("end_s")
            .or_else(|| item.get("end"))
            .and_then(|v| v.as_f64());
        if let (Some(s), Some(e)) = (start_s, end_s)
            && e > s
        {
            out.push(TranscriptWord {
                text: text.to_string(),
                start_s: s,
                end_s: e,
            });
        }
    }
    Some(out)
}

fn whisper_sidecar_path(project_root: &Path, asset_id: &str) -> PathBuf {
    project_root
        .join("index")
        .join("whisper")
        .join(format!("{asset_id}.json"))
}

pub const DESCRIPTION: &str = "\
Detect places where the speaker began a thought, abandoned it, and \
restarted (\"actually, what I meant was…\", \"wait, let me back up\", \
etc), plus production/coaching asides such as \"cut\", \"one more time\", \
or \"you can just say\". v1 heuristic: scans the whisper transcript \
for restart markers and production-aside language, then surfaces the \
visible source fragment as the candidate false-start.\
\n\nEach finding: { asset_id, marker, source_start_s, source_end_s, \
timeline_start_s, timeline_end_s, snippet }. The marker tells the \
agent what triggered the detection so it can describe the Note. The \
source range covers the fragment that precedes the restart — what \
the user might trim.\
\n\nLimitations: this is a heuristic, not a continuity model. False \
positives are likely (\"actually\" used as an emphatic, not a \
restart). The agent should treat findings as suggestions and present \
them as Notes the user reviews — never auto-trim without approval.\
\n\nDefault max_results=20, hard cap 100. Returns empty when no \
whisper sidecars exist.\
";
