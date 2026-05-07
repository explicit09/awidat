//! `find_false_starts` tool — surface places where the speaker
//! began a thought, abandoned it, and restarted.
//!
//! Heuristic in v1 (gated by Phase 1 user reaction; the hard
//! version using a continuity model is Phase 4):
//!
//! 1. **Abrupt cut**: a word whose end is followed by a silence
//!    range ≥ 0.4s starting within 1s, AND the next word is not
//!    a continuation of the same thought (different sentence per
//!    whisper segment boundaries, or contains a restart marker).
//!
//! 2. **Restart marker**: the speaker says one of {"wait", "let
//!    me", "actually"} mid-utterance. We surface the *prior*
//!    fragment (from the segment start to the restart marker) as
//!    the false start; the trim removes everything from segment-
//!    start to restart-marker-start.
//!
//! Each finding shapes for an `EditorialNote` of kind `false_start`.
//! Trimmed-away ranges still surface if the visible portion contains
//! a restart marker — the user can decide whether to also delete
//! more upstream context.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use awidat_proto::otio::{MediaReference, StackChild, Timeline, TrackChild};
use awidat_proto::project::Project;
use serde::Deserialize;

use crate::FunctionCallError;
use crate::anthropic::Tool as ToolSchema;
use crate::tool::{ToolContext, ToolHandler, ToolInvocation, ToolOutput};

/// Restart-marker phrases. Lowercase-matched against word.text or
/// against a sliding 2-word window (so "let me" matches across word
/// boundaries). The single-word forms catch most restarts.
const RESTART_MARKERS: &[&str] = &["wait", "actually"];

/// Multi-word restart phrases. Matched across consecutive words.
const RESTART_PHRASES: &[&[&str]] = &[&["let", "me"]];

/// Default cap on findings.
const DEFAULT_MAX_RESULTS: usize = 20;
const HARD_MAX_RESULTS: usize = 100;

pub struct FindFalseStartsTool;

#[derive(Debug, Deserialize)]
struct FindFalseStartsArgs {
    /// Max findings to return. Default 20, hard cap 100.
    #[serde(default)]
    max_results: Option<usize>,
}

#[async_trait]
impl ToolHandler for FindFalseStartsTool {
    fn name(&self) -> &'static str {
        "find_false_starts"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "find_false_starts".into(),
            description: DESCRIPTION.into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "max_results": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 100,
                        "description": "Max findings to return. Default 20, hard cap 100."
                    }
                }
            }),
            ..Default::default()
        }
    }

    fn is_mutating(&self, _invocation: &ToolInvocation) -> bool {
        false
    }

    async fn handle(
        &self,
        invocation: ToolInvocation,
        ctx: ToolContext,
    ) -> Result<ToolOutput, FunctionCallError> {
        let args: FindFalseStartsArgs =
            serde_json::from_value(invocation.args).map_err(|e| {
                FunctionCallError::RespondToModel(format!(
                    "find_false_starts: invalid args ({e}). All fields optional."
                ))
            })?;
        let max_results = args
            .max_results
            .unwrap_or(DEFAULT_MAX_RESULTS)
            .min(HARD_MAX_RESULTS);

        let project = Project::read(&ctx.project_root).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "find_false_starts: failed to read project: {e}"
            ))
        })?;

        let findings =
            scan_false_starts(&ctx.project_root, &project.timeline, max_results);
        let body = serde_json::json!({
            "findings": findings,
            "more_available": findings.len() == max_results,
        });
        Ok(ToolOutput::text(body.to_string()))
    }
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
        let StackChild::Track(track) = stack_child else { continue };
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
                    let clip_source_end =
                        clip_source_start + range.duration.to_seconds();
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
    words: &[WhisperWord],
    asset_id: &str,
    clip_source_start: f64,
    clip_source_end: f64,
    timeline_offset: f64,
    out: &mut Vec<FalseStartFinding>,
) {
    let n = words.len();
    let mut i = 0;
    while i < n {
        let w = &words[i];
        // Skip words outside the clip's visible range.
        if w.end_s <= clip_source_start || w.start_s >= clip_source_end {
            i += 1;
            continue;
        }
        let normalized = normalize(&w.text);

        // Check single-word restart marker.
        let single_match = RESTART_MARKERS.iter().any(|m| *m == normalized);
        // Check multi-word phrase starting at i (e.g. "let me").
        let phrase_match = RESTART_PHRASES.iter().any(|phrase| {
            phrase.iter().enumerate().all(|(off, expected)| {
                words
                    .get(i + off)
                    .map(|w| normalize(&w.text) == *expected)
                    .unwrap_or(false)
            })
        });

        if single_match || phrase_match {
            // The "false start" is everything from the previous
            // restart-marker (or the beginning of this clip) up to
            // (but not including) this marker. Surface that range
            // so the user can decide whether to trim it.
            //
            // Lower bound: start of this segment / first word in
            // this clip. v1 simplification: the first word in the
            // clip whose start is ≥ clip_source_start. The tighter
            // "start of the current sentence" needs whisper
            // segments, which we'll add when the heuristic proves
            // out.
            let preceding_start_s = words
                .iter()
                .take(i)
                .rev()
                .find(|prev| {
                    prev.start_s >= clip_source_start
                        && (RESTART_MARKERS.iter().any(|m| *m == normalize(&prev.text))
                            || RESTART_PHRASES.iter().any(|phrase| {
                                phrase.iter().enumerate().all(|(off, expected)| {
                                    words
                                        .get(prev_index_of(words, prev) + off)
                                        .map(|x| normalize(&x.text) == *expected)
                                        .unwrap_or(false)
                                })
                            }))
                })
                .map(|prev| prev.end_s)
                .unwrap_or(clip_source_start);

            let visible_start = preceding_start_s.max(clip_source_start);
            let visible_end = w.start_s.min(clip_source_end);
            if visible_end <= visible_start {
                i += 1;
                continue;
            }
            // Build a short snippet of the false start so the agent
            // can describe the Note in plain English.
            let snippet: String = words
                .iter()
                .filter(|x| x.start_s >= visible_start && x.end_s <= visible_end)
                .map(|x| x.text.trim())
                .collect::<Vec<_>>()
                .join(" ");

            out.push(FalseStartFinding {
                asset_id: asset_id.to_string(),
                marker: w.text.trim().to_string(),
                source_start_s: visible_start,
                source_end_s: visible_end,
                timeline_start_s: timeline_offset + (visible_start - clip_source_start),
                timeline_end_s: timeline_offset + (visible_end - clip_source_start),
                snippet,
            });
        }
        i += 1;
    }
}

/// One restart-marker hit. The fragment from `source_start_s` to
/// `source_end_s` is the false-start the user might trim; `marker`
/// is the word that triggered detection (so the agent can describe
/// it: "speaker said 'wait' at 1:23 — false start before it?").
#[derive(Debug, Clone, serde::Serialize)]
pub struct FalseStartFinding {
    /// Source asset.
    pub asset_id: String,
    /// The restart-marker word that triggered this finding.
    pub marker: String,
    /// Source-time start of the false-start range (the ditch
    /// point — typically where a prior restart ended or where
    /// the clip began).
    pub source_start_s: f64,
    /// Source-time end (right before the restart marker word).
    pub source_end_s: f64,
    pub timeline_start_s: f64,
    pub timeline_end_s: f64,
    /// Short human-readable preview of the false-start text.
    pub snippet: String,
}

/// Lower-case + strip trailing punctuation. Words come out of
/// whisper with trailing `.` / `,` / `?` attached.
fn normalize(text: &str) -> String {
    text.trim()
        .trim_matches(|c: char| c == '.' || c == ',' || c == '?' || c == '!')
        .to_lowercase()
}

/// Look up the slice index of a word by reference equality. Used
/// inside the back-walk for previous-marker detection. O(n) but
/// only runs on backtracks (rare in well-formed transcripts).
fn prev_index_of(words: &[WhisperWord], target: &WhisperWord) -> usize {
    words
        .iter()
        .position(|w| std::ptr::eq(w, target))
        .unwrap_or(0)
}

#[derive(Debug, Clone)]
struct WhisperWord {
    text: String,
    start_s: f64,
    end_s: f64,
}

fn load_whisper_words(project_root: &Path, asset_id: &str) -> Option<Vec<WhisperWord>> {
    let path = whisper_sidecar_path(project_root, asset_id);
    let bytes = std::fs::read(&path).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let arr = value
        .pointer("/data/words")
        .or_else(|| value.pointer("/data/segments"))
        .and_then(|v| v.as_array())?;
    let mut out = Vec::with_capacity(arr.len());
    for item in arr {
        let text = item.get("text").and_then(|v| v.as_str()).unwrap_or("").trim();
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
            out.push(WhisperWord {
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

const DESCRIPTION: &str = "\
Detect places where the speaker began a thought, abandoned it, and \
restarted (\"actually, what I meant was…\", \"wait, let me back up\", \
etc). v1 heuristic: scans the whisper transcript for restart-marker \
words ({wait, actually, let me}) and surfaces the preceding text \
fragment as the candidate false-start.\
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

#[cfg(test)]
mod tests {
    use super::*;

    fn words(items: &[(&str, f64, f64)]) -> Vec<WhisperWord> {
        items
            .iter()
            .map(|(t, s, e)| WhisperWord {
                text: t.to_string(),
                start_s: *s,
                end_s: *e,
            })
            .collect()
    }

    #[test]
    fn detects_wait_marker_and_emits_preceding_range() {
        let ws = words(&[
            ("So", 0.0, 0.3),
            ("the", 0.3, 0.5),
            ("thing", 0.5, 0.9),
            ("is,", 0.9, 1.2),
            ("wait,", 1.2, 1.5),
            ("actually", 1.5, 2.0),
            ("the", 2.0, 2.2),
            ("real", 2.2, 2.5),
            ("issue", 2.5, 2.9),
        ]);
        let mut out = Vec::new();
        // Clip plays full source [0.0, 3.0]; timeline starts at 10.
        scan_words_for_restarts(&ws, "raw/ep.mp4", 0.0, 3.0, 10.0, &mut out);
        // One finding: the leading fragment ("So the thing is,") gets
        // surfaced at "wait,". The "actually" right after the wait
        // marker doesn't get its own finding because there's no
        // intervening text between them — the user would trim both
        // markers together with one *** Trim Clip envelope.
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].marker, "wait,");
        // False-start range covers the leading fragment up to but
        // not including "wait,".
        assert!((out[0].source_start_s - 0.0).abs() < 1e-9);
        assert!((out[0].source_end_s - 1.2).abs() < 1e-9);
        assert!(out[0].snippet.contains("So"));
        // Timeline is offset by 10.
        assert!((out[0].timeline_start_s - 10.0).abs() < 1e-9);
    }

    #[test]
    fn detects_two_separated_restart_markers() {
        // When restart markers are separated by real content, both
        // surface as findings. Second finding's range starts after
        // the first marker's end (the prior marker's end_s).
        let ws = words(&[
            ("So", 0.0, 0.3),
            ("the", 0.3, 0.5),
            ("wait,", 0.5, 0.8),
            ("anyway", 0.8, 1.1),
            ("we", 1.1, 1.3),
            ("went,", 1.3, 1.6),
            ("actually,", 1.6, 2.0),
            ("more", 2.0, 2.3),
        ]);
        let mut out = Vec::new();
        scan_words_for_restarts(&ws, "raw/ep.mp4", 0.0, 3.0, 0.0, &mut out);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].marker, "wait,");
        assert_eq!(out[1].marker, "actually,");
        // Second finding's range starts at the previous marker's end
        // (0.8) and runs up to the second marker's start (1.6).
        assert!((out[1].source_start_s - 0.8).abs() < 1e-9);
        assert!((out[1].source_end_s - 1.6).abs() < 1e-9);
    }

    #[test]
    fn detects_let_me_phrase() {
        let ws = words(&[
            ("So", 0.0, 0.3),
            ("anyway,", 0.3, 0.7),
            ("let", 0.7, 0.9),
            ("me", 0.9, 1.1),
            ("explain", 1.1, 1.5),
        ]);
        let mut out = Vec::new();
        scan_words_for_restarts(&ws, "raw/ep.mp4", 0.0, 2.0, 0.0, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].marker, "let");
        assert!((out[0].source_end_s - 0.7).abs() < 1e-9);
    }

    #[test]
    fn skips_words_outside_clip_source_range() {
        let ws = words(&[
            ("wait", 30.0, 30.3),
            ("actually", 30.3, 30.7),
        ]);
        let mut out = Vec::new();
        // Clip plays only [0..10]; both markers are well outside.
        scan_words_for_restarts(&ws, "raw/ep.mp4", 0.0, 10.0, 0.0, &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn no_marker_no_finding() {
        let ws = words(&[
            ("hello", 0.0, 0.3),
            ("world", 0.3, 0.6),
        ]);
        let mut out = Vec::new();
        scan_words_for_restarts(&ws, "raw/ep.mp4", 0.0, 1.0, 0.0, &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn case_insensitive_marker_match() {
        let ws = words(&[
            ("So,", 0.0, 0.3),
            ("Wait!", 0.3, 0.6),
            ("the", 0.6, 0.8),
            ("real", 0.8, 1.0),
        ]);
        let mut out = Vec::new();
        scan_words_for_restarts(&ws, "raw/ep.mp4", 0.0, 2.0, 0.0, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].marker, "Wait!");
    }
}
