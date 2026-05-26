//! `find_filler_words` — scan whisper transcripts for verbal fillers
//! ("um", "uh", "ah", "er", etc) that the agent could suggest cutting.
//! Ported from `crates/core/src/tools/find_filler_words.rs` to the
//! in-process MCP server.
//!
//! Reads `index/whisper/<asset>.json` per asset (word-level alignment
//! when available; falls back to segment-level if no word array),
//! filters words whose lowercase form matches a small configurable
//! filler list, intersects each word's span with the timeline's clip
//! ranges, and returns the surviving fillers as editorial findings.

use std::path::{Path, PathBuf};

use awidat_proto::otio::{MediaReference, StackChild, Timeline, TrackChild};
use awidat_proto::project::Project;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::awidat_mcp::context::McpToolCtx;
use crate::transcript_cleanup::{default_filler_tokens, normalize_transcript_token};

/// Default cap on returned findings. Same magnitude as
/// `find_dead_air`; podcasts can have hundreds of "um"s and the
/// agent shouldn't drown in tool output.
const DEFAULT_MAX_RESULTS: usize = 30;
const HARD_MAX_RESULTS: usize = 200;

/// Arguments to `find_filler_words`.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct FindFillerWordsArgs {
    /// Override the filler list. Lowercase-matched.
    #[serde(default)]
    pub fillers: Option<Vec<String>>,
    /// Include aggressive discourse markers ("like", "you know",
    /// "i mean", "basically") in the match list. Default false.
    #[serde(default)]
    pub aggressive: bool,
    /// Max findings to return. Default 30, hard cap 200.
    #[serde(default)]
    pub max_results: Option<usize>,
}

/// Run `find_filler_words` against the project resolved from
/// [`McpToolCtx`]. Returns the JSON body as `Ok(String)`;
/// project-read errors return `Err(String)`.
pub fn run(args: FindFillerWordsArgs, ctx: McpToolCtx) -> Result<String, String> {
    let fillers = build_filler_set(args.fillers, args.aggressive);
    let max_results = args
        .max_results
        .unwrap_or(DEFAULT_MAX_RESULTS)
        .min(HARD_MAX_RESULTS);

    let project = Project::read(&ctx.project_root)
        .map_err(|e| format!("find_filler_words: failed to read project: {e}"))?;

    // Honor dismissal memory at the bucket level: dismissing
    // "filler basic" silences all default-list fillers;
    // dismissing "filler aggressive" silences just the
    // discourse-marker findings (the basic ones still surface).
    let dismissals = crate::dismissal::load_dismissals(&ctx.project_root);
    let bucket = crate::dismissal::DismissalBucket::for_filler(args.aggressive);
    if dismissals.is_dismissed(bucket) {
        let body = serde_json::json!({
            "fillers": fillers,
            "findings": Vec::<FillerFinding>::new(),
            "more_available": false,
            "dismissed": true,
        });
        return Ok(body.to_string());
    }

    let findings = scan_filler_words(&ctx.project_root, &project.timeline, &fillers, max_results);
    let body = serde_json::json!({
        "fillers": fillers,
        "findings": findings,
        "more_available": findings.len() == max_results,
    });
    Ok(body.to_string())
}

fn build_filler_set(override_list: Option<Vec<String>>, aggressive: bool) -> Vec<String> {
    if let Some(list) = override_list {
        return list
            .into_iter()
            .map(|token| normalize_transcript_token(&token))
            .filter(|token| !token.is_empty())
            .collect();
    }
    default_filler_tokens(aggressive)
}

/// Walk the timeline + each clip's whisper sidecar; emit a finding
/// for every filler word that lands on the timeline. Pure function.
pub fn scan_filler_words(
    project_root: &Path,
    timeline: &Timeline,
    fillers: &[String],
    max_results: usize,
) -> Vec<FillerFinding> {
    let mut out: Vec<FillerFinding> = Vec::new();
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
                        for w in &words {
                            // Filler match (case-insensitive).
                            let normalized = normalize_transcript_token(&w.text);
                            if !fillers.iter().any(|f| f == &normalized) {
                                continue;
                            }
                            // Intersect with clip's source range.
                            if w.end_s <= clip_source_start || w.start_s >= clip_source_end {
                                continue;
                            }
                            let visible_start = w.start_s.max(clip_source_start);
                            let visible_end = w.end_s.min(clip_source_end);
                            let timeline_start = timeline_cursor_s
                                + clip_track_start
                                + (visible_start - clip_source_start);
                            let timeline_end = timeline_start + (visible_end - visible_start);

                            out.push(FillerFinding {
                                asset_id: asset_id.clone(),
                                text: w.text.trim().to_string(),
                                source_start_s: visible_start,
                                source_end_s: visible_end,
                                timeline_start_s: timeline_start,
                                timeline_end_s: timeline_end,
                            });

                            if out.len() >= max_results {
                                return out;
                            }
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

/// One filler word that survived timeline intersection, ready to
/// become an `EditorialNote` of kind `filler_word`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct FillerFinding {
    /// Source asset.
    pub asset_id: String,
    /// The actual text matched (preserves case, e.g. "Um", "UH").
    pub text: String,
    /// Source-media seconds where the filler begins.
    pub source_start_s: f64,
    /// Source-media seconds where the filler ends.
    pub source_end_s: f64,
    /// Timeline-time start.
    pub timeline_start_s: f64,
    /// Timeline-time end.
    pub timeline_end_s: f64,
}

/// One whisper word read off the sidecar. Internal; the tool's
/// `FillerFinding` is what the agent sees.
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
    // Prefer /data/words for word-level alignment; fall back to
    // /data/segments if the indexer didn't produce words.
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

pub const DESCRIPTION: &str = "\
Scan the project's whisper transcripts for filler words (\"um\", \
\"uh\", etc) that the agent could suggest cutting. Each finding is \
a single word's span: { asset_id, text, source_start_s, \
source_end_s, timeline_start_s, timeline_end_s }, ready to become \
an EditorialNote of kind filler_word or to drive a `*** Trim Clip` \
envelope.\
\n\nDefault filler list: um/uh/uhh/umm/ah/ahh/er/err. Pass \
`aggressive: true` to also include discourse markers (like / so / \
just / but / yeah / basically / you know / i mean) — these are \
taste-dependent so they're opt-in. Pass `fillers: [...]` to \
override the list entirely.\
\n\nFindings are intersected with the timeline's clip ranges, so \
fillers in trimmed-away regions don't get re-surfaced. Default \
max_results=30, hard cap 200; `more_available: true` when the cap \
was hit.\
\n\nReturns an empty findings list when whisper sidecars haven't \
landed yet — surface that as \"transcripts still indexing?\" \
rather than \"no fillers in this episode.\"\
";
