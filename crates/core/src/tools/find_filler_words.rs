//! `find_filler_words` tool — scan whisper transcripts for verbal
//! fillers ("um", "uh", "ah", "er", etc) that the agent could
//! suggest cutting.
//!
//! Reads `index/whisper/<asset>.json` per asset (word-level
//! alignment when available; falls back to segment-level if no
//! word array), filters words whose lowercase form matches a small
//! configurable filler list, intersects each word's span with the
//! timeline's clip ranges, and returns the surviving fillers as
//! editorial findings.
//!
//! Coupling with continuity (Phase 2): a filler that lands mid-
//! sentence is a "clean cut" candidate; a filler that's standing
//! on its own with silence before/after is even cleaner. v1
//! returns the raw filler ranges; the agent (or Phase 2 rules) can
//! decide which to surface as Notes.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use montage_proto::otio::{MediaReference, StackChild, Timeline, TrackChild};
use montage_proto::project::Project;
use serde::Deserialize;

use crate::FunctionCallError;
use crate::tool::{ToolContext, ToolHandler, ToolInvocation, ToolOutput};
use crate::tool_schema::Tool as ToolSchema;
use crate::transcript_cleanup::{default_filler_tokens, normalize_transcript_token};

/// Default cap on returned findings. Same magnitude as
/// `find_dead_air`; podcasts can have hundreds of "um"s and the
/// agent shouldn't drown in tool output.
const DEFAULT_MAX_RESULTS: usize = 30;
const HARD_MAX_RESULTS: usize = 200;

/// Tool that finds filler words in transcript regions visible on the timeline.
pub struct FindFillerWordsTool;

#[derive(Debug, Deserialize)]
struct FindFillerWordsArgs {
    /// Override the filler list. Lowercase-matched.
    #[serde(default)]
    fillers: Option<Vec<String>>,
    /// Include aggressive discourse markers ("like", "you know",
    /// "i mean", "basically") in the match list. Default false.
    #[serde(default)]
    aggressive: bool,
    /// Max findings to return. Default 30, hard cap 200.
    #[serde(default)]
    max_results: Option<usize>,
}

#[async_trait]
impl ToolHandler for FindFillerWordsTool {
    fn name(&self) -> &'static str {
        "find_filler_words"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "find_filler_words".into(),
            description: DESCRIPTION.into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "fillers": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Override the default filler list (case-insensitive)."
                    },
                    "aggressive": {
                        "type": "boolean",
                        "description": "Include discourse markers like / you know / i mean / basically. Default false."
                    },
                    "max_results": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 200,
                        "description": "Max findings to return. Default 30, hard cap 200."
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
        let args: FindFillerWordsArgs = serde_json::from_value(invocation.args).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "find_filler_words: invalid args ({e}). All fields optional."
            ))
        })?;
        let fillers = build_filler_set(args.fillers, args.aggressive);
        let max_results = args
            .max_results
            .unwrap_or(DEFAULT_MAX_RESULTS)
            .min(HARD_MAX_RESULTS);

        let project = Project::read(&ctx.project_root).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "find_filler_words: failed to read project: {e}"
            ))
        })?;

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
            return Ok(ToolOutput::text(body.to_string()));
        }

        let findings =
            scan_filler_words(&ctx.project_root, &project.timeline, &fillers, max_results);
        let body = serde_json::json!({
            "fillers": fillers,
            "findings": findings,
            "more_available": findings.len() == max_results,
        });
        Ok(ToolOutput::text(body.to_string()))
    }
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

const DESCRIPTION: &str = "\
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

#[cfg(test)]
mod tests {
    use super::*;
    use montage_proto::otio::{
        Clip, ExternalReference, MediaReference, RationalTime, Stack, StackChild, TimeRange, Track,
        TrackChild, TrackKind,
    };

    fn write_whisper_words(project_root: &Path, asset_id: &str, words: Vec<(&str, f64, f64)>) {
        let path = whisper_sidecar_path(project_root, asset_id);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let payload = serde_json::json!({
            "indexer": "whisper",
            "asset_id": asset_id,
            "data": {
                "words": words
                    .into_iter()
                    .map(|(t, s, e)| {
                        serde_json::json!({"text": t, "start_s": s, "end_s": e})
                    })
                    .collect::<Vec<_>>()
            }
        });
        std::fs::write(&path, serde_json::to_vec_pretty(&payload).unwrap()).unwrap();
    }

    fn timeline_with_clip(asset_id: &str, source_start_s: f64, duration_s: f64) -> Timeline {
        let mut clip = Clip::empty("c1");
        clip.media_reference = MediaReference::External(ExternalReference::new(asset_id));
        clip.source_range = Some(TimeRange::new(
            RationalTime::new(source_start_s * 24.0, 24.0),
            RationalTime::new(duration_s * 24.0, 24.0),
        ));
        let mut track = Track::empty("V1", TrackKind::Video);
        track.children.push(TrackChild::Clip(clip));
        let mut tl = Timeline::empty("p");
        let mut stack = Stack::empty("root");
        stack.children.push(StackChild::Track(track));
        tl.tracks = stack;
        tl
    }

    #[test]
    fn finds_default_fillers_inside_clip_range() {
        let dir = tempfile::tempdir().unwrap();
        let asset = "raw/ep.mp4";
        // "so um yeah uh okay" — two fillers.
        write_whisper_words(
            dir.path(),
            asset,
            vec![
                ("so", 0.0, 0.3),
                ("um", 0.3, 0.6),
                ("yeah", 0.6, 0.9),
                ("uh", 0.9, 1.1),
                ("okay", 1.1, 1.4),
            ],
        );
        // Asset file must exist for project_root.join(asset_id) to
        // succeed in the production tool — but scan_filler_words
        // doesn't check, so the test doesn't need to write it.
        let tl = timeline_with_clip(asset, 0.0, 2.0);
        let fillers = build_filler_set(None, false);
        let findings = scan_filler_words(dir.path(), &tl, &fillers, 100);
        assert_eq!(findings.len(), 2);
        assert_eq!(findings[0].text, "um");
        assert_eq!(findings[1].text, "uh");
    }

    #[test]
    fn case_insensitive_match() {
        let dir = tempfile::tempdir().unwrap();
        let asset = "raw/ep.mp4";
        write_whisper_words(dir.path(), asset, vec![("Um,", 0.0, 0.3), ("UH", 0.3, 0.6)]);
        let tl = timeline_with_clip(asset, 0.0, 1.0);
        let fillers = build_filler_set(None, false);
        let findings = scan_filler_words(dir.path(), &tl, &fillers, 100);
        assert_eq!(findings.len(), 2);
    }

    #[test]
    fn aggressive_includes_discourse_markers() {
        let dir = tempfile::tempdir().unwrap();
        let asset = "raw/ep.mp4";
        write_whisper_words(
            dir.path(),
            asset,
            vec![("like", 0.0, 0.3), ("um", 0.3, 0.6)],
        );
        let tl = timeline_with_clip(asset, 0.0, 1.0);
        // Default mode: only "um" matches.
        let basic = build_filler_set(None, false);
        let basic_findings = scan_filler_words(dir.path(), &tl, &basic, 100);
        assert_eq!(basic_findings.len(), 1);
        assert_eq!(basic_findings[0].text, "um");
        // Aggressive mode: both match.
        let aggro = build_filler_set(None, true);
        let aggro_findings = scan_filler_words(dir.path(), &tl, &aggro, 100);
        assert_eq!(aggro_findings.len(), 2);
    }

    #[test]
    fn drops_fillers_outside_clip_source_range() {
        let dir = tempfile::tempdir().unwrap();
        let asset = "raw/ep.mp4";
        // "um" at source-time 30..30.3 — outside clip's 0..10 range.
        write_whisper_words(dir.path(), asset, vec![("um", 30.0, 30.3)]);
        let tl = timeline_with_clip(asset, 0.0, 10.0);
        let fillers = build_filler_set(None, false);
        let findings = scan_filler_words(dir.path(), &tl, &fillers, 100);
        assert!(findings.is_empty());
    }

    #[test]
    fn empty_when_no_whisper_sidecar() {
        let dir = tempfile::tempdir().unwrap();
        let asset = "raw/no-sidecar.mp4";
        std::fs::create_dir_all(dir.path().join("raw")).unwrap();
        std::fs::write(dir.path().join(asset), b"stub").unwrap();
        let tl = timeline_with_clip(asset, 0.0, 10.0);
        let fillers = build_filler_set(None, false);
        let findings = scan_filler_words(dir.path(), &tl, &fillers, 100);
        assert!(findings.is_empty());
    }

    #[test]
    fn override_filler_list_replaces_default() {
        let dir = tempfile::tempdir().unwrap();
        let asset = "raw/ep.mp4";
        write_whisper_words(
            dir.path(),
            asset,
            vec![("um", 0.0, 0.3), ("foobar", 0.3, 0.6)],
        );
        let tl = timeline_with_clip(asset, 0.0, 1.0);
        // Custom list: only "foobar" matches; "um" is excluded.
        let custom = build_filler_set(Some(vec!["foobar".into()]), false);
        let findings = scan_filler_words(dir.path(), &tl, &custom, 100);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].text, "foobar");
    }

    #[test]
    fn respects_max_results_cap() {
        let dir = tempfile::tempdir().unwrap();
        let asset = "raw/ep.mp4";
        let words: Vec<_> = (0..10)
            .map(|i| ("um", i as f64 * 0.5, i as f64 * 0.5 + 0.3))
            .collect();
        write_whisper_words(dir.path(), asset, words);
        let tl = timeline_with_clip(asset, 0.0, 10.0);
        let fillers = build_filler_set(None, false);
        let findings = scan_filler_words(dir.path(), &tl, &fillers, 4);
        assert_eq!(findings.len(), 4);
    }
}
