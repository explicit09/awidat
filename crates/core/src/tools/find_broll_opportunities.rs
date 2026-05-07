//! `find_broll_opportunities` tool — surface moments where stock B-roll
//! would land well, derived from transcript heuristics.
//!
//! Distinct from [`crate::tools::broll_candidates`], which finds *in-
//! footage* cutaways (shots already in the project that could cover
//! another speaker). This tool looks for moments where the speaker
//! references a concrete visual subject ("imagine a busy office",
//! "look at this graph") and proposes a Pexels query for a stock
//! cutaway. The agent then drives [`crate::tools::search_broll`] to
//! preview and [`crate::tools::use_broll`] to place.
//!
//! Heuristic: slide over whisper words; emit a finding when one of the
//! known trigger phrases fires (demonstrative — "look at", "imagine
//! a", "picture", etc.) AND the words immediately following contain
//! at least one concrete noun (e.g. "skyline", "office", "graph"). The
//! `pexels_query` is the trigger's tail words minus stop-words.
//!
//! v1 ships with a small hand-tuned list. Phase 4 (per the plan)
//! escalates to a more sophisticated entity-extraction pass; the
//! shape of the tool output is stable across that escalation.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use awidat_proto::otio::{MediaReference, StackChild, Timeline, TrackChild};
use awidat_proto::project::Project;
use serde::{Deserialize, Serialize};

use crate::FunctionCallError;
use crate::anthropic::Tool as ToolSchema;
use crate::tool::{ToolContext, ToolHandler, ToolInvocation, ToolOutput};

/// Trigger phrases that, when followed by a concrete noun, suggest
/// stock b-roll would land. Lowercase-matched against the whisper
/// word stream as contiguous sequences.
const TRIGGER_PHRASES: &[&[&str]] = &[
    &["look", "at"],
    &["check", "out"],
    &["picture", "this"],
    &["imagine", "a"],
    &["imagine", "the"],
    &["imagine"],
    &["think", "about"],
    &["consider"],
    &["the", "way"],
    &["see", "this"],
    &["see", "the"],
    &["like", "a"],
    &["just", "like"],
];

/// Concrete nouns we'll search Pexels for. Curated for general
/// "scene-setting" b-roll — abstract nouns ("idea", "feeling") are
/// excluded because the resulting Pexels searches return junk.
const CONCRETE_NOUNS: &[&str] = &[
    // Architecture / cityscape
    "skyline", "city", "street", "building", "office", "kitchen", "bedroom",
    "bathroom", "window", "door", "stairs", "garage", "warehouse", "factory",
    // Nature / landscape
    "ocean", "sea", "mountain", "forest", "river", "lake", "beach", "sunset",
    "sunrise", "tree", "flower", "garden", "field", "desert", "snow", "rain",
    "storm", "cloud", "sky",
    // Tech / objects
    "phone", "laptop", "screen", "computer", "keyboard", "monitor", "code",
    "graph", "chart", "map", "diagram", "whiteboard", "notebook", "book",
    "camera", "microphone", "robot", "car", "bike", "plane", "train",
    // People-doing-things (Pexels indexes these as scene tags)
    "crowd", "audience", "meeting", "handshake", "running", "cooking", "writing",
    "typing", "walking", "driving",
    // Food / lifestyle
    "coffee", "kitchen", "food", "restaurant", "cafe",
    // Workplace
    "boardroom", "conference", "presentation", "interview",
];

/// Stop-words pruned out of the generated Pexels query — fillers and
/// articles that don't help search relevance.
const QUERY_STOP_WORDS: &[&str] = &[
    "a", "an", "the", "this", "that", "these", "those", "is", "are", "was",
    "were", "of", "to", "in", "on", "at", "for", "with", "by", "and", "or",
    "but", "so", "um", "uh", "you", "i", "we", "they", "it",
];

/// How many words after the trigger to scan for a concrete noun + use
/// as the query basis. 6 is enough to capture "imagine a busy office
/// in midtown" without spilling into the next sentence.
const QUERY_WINDOW_WORDS: usize = 6;

/// Default cap on findings. Editorial Notes are coarse; the agent
/// shouldn't be drowning in 50 b-roll Notes per session.
const DEFAULT_MAX_RESULTS: usize = 12;
const HARD_MAX_RESULTS: usize = 40;

/// Default b-roll duration in seconds. Stock cutaways feel right at
/// 2–4s; longer than that and the audience starts wondering when
/// they'll see the speaker again. The agent can override per finding.
const DEFAULT_BROLL_DURATION_S: f64 = 3.0;

/// The `find_broll_opportunities` tool.
pub struct FindBrollOpportunitiesTool;

#[derive(Debug, Deserialize)]
struct Args {
    /// Restrict to one asset id; otherwise scan all whisper sidecars
    /// reachable from the timeline.
    #[serde(default)]
    asset_id: Option<String>,
    /// Override cutaway duration in seconds (1.0–8.0).
    #[serde(default)]
    duration_s: Option<f64>,
    /// Max findings. Default 12, hard cap 40.
    #[serde(default)]
    max_results: Option<usize>,
}

#[async_trait]
impl ToolHandler for FindBrollOpportunitiesTool {
    fn name(&self) -> &'static str {
        "find_broll_opportunities"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "find_broll_opportunities".into(),
            description: DESCRIPTION.into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "asset_id": {
                        "type": "string",
                        "description": "Restrict to one asset's whisper sidecar."
                    },
                    "duration_s": {
                        "type": "number",
                        "minimum": 1.0,
                        "maximum": 8.0,
                        "description": "Default cutaway length in seconds. Default 3.0."
                    },
                    "max_results": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 40,
                        "description": "Default 12, hard cap 40."
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
        let args: Args = serde_json::from_value(invocation.args).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "find_broll_opportunities: invalid args ({e}). All fields optional."
            ))
        })?;
        let max_results = args
            .max_results
            .unwrap_or(DEFAULT_MAX_RESULTS)
            .min(HARD_MAX_RESULTS);
        let duration_s = args
            .duration_s
            .unwrap_or(DEFAULT_BROLL_DURATION_S)
            .clamp(1.0, 8.0);

        let project = Project::read(&ctx.project_root).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "find_broll_opportunities: failed to read project: {e}"
            ))
        })?;

        // Honor dismissal at the bucket level: if the user has
        // dismissed b-roll suggestions for this project, surface a
        // tombstone rather than silently re-flooding them.
        let dismissals = crate::dismissal::load_dismissals(&ctx.project_root);
        if dismissals.is_dismissed(crate::dismissal::DismissalBucket::BrollOpportunity) {
            let body = serde_json::json!({
                "findings": Vec::<BrollFinding>::new(),
                "more_available": false,
                "dismissed": true,
            });
            return Ok(ToolOutput::text(body.to_string()));
        }

        let findings = scan_broll_opportunities(
            &ctx.project_root,
            &project.timeline,
            args.asset_id.as_deref(),
            duration_s,
            max_results,
        );
        let body = serde_json::json!({
            "findings": findings,
            "more_available": findings.len() == max_results,
        });
        Ok(ToolOutput::text(body.to_string()))
    }
}

/// Walk timeline → whisper sidecar → emit findings. Pure function for
/// test ergonomics.
pub fn scan_broll_opportunities(
    project_root: &Path,
    timeline: &Timeline,
    only_asset: Option<&str>,
    duration_s: f64,
    max_results: usize,
) -> Vec<BrollFinding> {
    let mut out: Vec<BrollFinding> = Vec::new();
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
                    if let Some(filter) = only_asset
                        && asset_id != filter
                    {
                        track_cursor_s += range.duration.to_seconds();
                        continue;
                    }
                    let clip_source_start = range.start_time.to_seconds();
                    let clip_source_end = clip_source_start + range.duration.to_seconds();
                    let clip_track_start = track_cursor_s;

                    if let Some(words) = load_whisper_words(project_root, &asset_id) {
                        for finding in scan_words(
                            &asset_id,
                            &words,
                            clip_source_start,
                            clip_source_end,
                            clip_track_start,
                            timeline_cursor_s,
                            duration_s,
                        ) {
                            out.push(finding);
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

/// Run the trigger-phrase scan over one asset's word list, intersected
/// with one clip's source range.
#[allow(clippy::too_many_arguments)]
fn scan_words(
    asset_id: &str,
    words: &[WhisperWord],
    clip_source_start: f64,
    clip_source_end: f64,
    clip_track_start: f64,
    timeline_cursor_s: f64,
    duration_s: f64,
) -> Vec<BrollFinding> {
    let mut out = Vec::new();
    let lowered: Vec<String> = words
        .iter()
        .map(|w| normalize_word(&w.text))
        .collect();

    // Track the last-emitted source-time so we don't dump 5 findings
    // within 4 seconds of each other when the speaker rambles. Min
    // gap = duration_s (the new cutaway shouldn't overlap the prior).
    let mut last_emit_end: Option<f64> = None;

    let mut i = 0;
    while i < lowered.len() {
        let Some(phrase_len) = match_trigger_at(&lowered, i) else {
            i += 1;
            continue;
        };
        // Trigger fires; look ahead in the next QUERY_WINDOW_WORDS for
        // a concrete noun.
        let look_start = i + phrase_len;
        let look_end = (look_start + QUERY_WINDOW_WORDS).min(lowered.len());
        let Some(noun_idx) = (look_start..look_end).find(|&j| is_concrete_noun(&lowered[j]))
        else {
            // Trigger without a concrete noun — common in podcasts
            // ("imagine that…"). Skip; we'd rather miss than send the
            // agent on a junk Pexels query.
            i += phrase_len;
            continue;
        };

        // Build the query: trigger-tail + noun + 1 word of trailing
        // context (e.g. "busy office in midtown" → "busy office
        // midtown"), pruned of stop-words.
        let query_end = (noun_idx + 2).min(lowered.len());
        let query_words: Vec<&str> = lowered[look_start..query_end]
            .iter()
            .map(String::as_str)
            .filter(|w| !QUERY_STOP_WORDS.iter().any(|s| s == w) && !w.is_empty())
            .collect();
        // Always include the matched noun even if it slipped through
        // a (defensive) stop-word check.
        let mut query_terms = query_words.clone();
        if !query_terms.iter().any(|w| *w == lowered[noun_idx].as_str()) {
            query_terms.push(lowered[noun_idx].as_str());
        }
        let pexels_query = query_terms.join(" ").trim().to_string();
        if pexels_query.is_empty() {
            i += phrase_len;
            continue;
        }

        // Anchor the cutaway start at the trigger word's source time.
        let trigger_source_start = words[i].start_s;
        // Skip if outside this clip's source range.
        if trigger_source_start < clip_source_start || trigger_source_start >= clip_source_end {
            i += phrase_len;
            continue;
        }
        // Skip if too close to the previous emission.
        if let Some(prev_end) = last_emit_end
            && trigger_source_start < prev_end
        {
            i += phrase_len;
            continue;
        }

        // Clamp the cutaway end inside the clip's source range — it's
        // OK if a Pexels download is longer than `duration_s`; the
        // cutaway in the timeline is what we're sizing here.
        let raw_end_source = trigger_source_start + duration_s;
        let cutaway_source_end = raw_end_source.min(clip_source_end);
        let actual_duration = (cutaway_source_end - trigger_source_start).max(0.5);
        let timeline_start =
            timeline_cursor_s + clip_track_start + (trigger_source_start - clip_source_start);
        let timeline_end = timeline_start + actual_duration;

        // Reason string — short, references the trigger phrase and the
        // matched noun, so the agent can quote it in the Note.
        let trigger_text = lowered[i..i + phrase_len].join(" ");
        let reason = format!(
            "speaker says \"{}\" — visual reference to \"{}\"",
            trigger_text, lowered[noun_idx]
        );

        out.push(BrollFinding {
            asset_id: asset_id.to_string(),
            source_start_s: trigger_source_start,
            source_end_s: cutaway_source_end,
            timeline_start_s: timeline_start,
            timeline_end_s: timeline_end,
            reason,
            pexels_query,
            transcript_excerpt: excerpt_words(words, i, look_end),
        });
        last_emit_end = Some(raw_end_source);
        i += phrase_len;
    }
    out
}

/// Returns the trigger-phrase length if one matches at position `i`.
fn match_trigger_at(lowered: &[String], i: usize) -> Option<usize> {
    for phrase in TRIGGER_PHRASES {
        if i + phrase.len() > lowered.len() {
            continue;
        }
        let matches_phrase = phrase
            .iter()
            .enumerate()
            .all(|(off, w)| lowered[i + off].as_str() == *w);
        if matches_phrase {
            return Some(phrase.len());
        }
    }
    None
}

fn is_concrete_noun(word: &str) -> bool {
    CONCRETE_NOUNS.iter().any(|n| *n == word)
}

fn normalize_word(s: &str) -> String {
    s.trim()
        .trim_matches(|c: char| {
            c == '.' || c == ',' || c == '?' || c == '!' || c == ';' || c == ':' || c == '"'
        })
        .to_lowercase()
}

fn excerpt_words(words: &[WhisperWord], start: usize, end: usize) -> String {
    let end = end.min(words.len());
    words[start..end]
        .iter()
        .map(|w| w.text.trim().to_string())
        .collect::<Vec<_>>()
        .join(" ")
}

/// One b-roll opportunity ready to become an `EditorialNote` of kind
/// `broll_opportunity`.
#[derive(Debug, Clone, Serialize)]
pub struct BrollFinding {
    /// Source asset where the trigger fired.
    pub asset_id: String,
    /// Source-media seconds where the cutaway should start.
    pub source_start_s: f64,
    /// Source-media seconds where it should end.
    pub source_end_s: f64,
    /// Timeline-time start of the cutaway.
    pub timeline_start_s: f64,
    /// Timeline-time end.
    pub timeline_end_s: f64,
    /// Short human-readable reason: `speaker says "look at" — visual reference to "skyline"`.
    pub reason: String,
    /// Suggested Pexels search query.
    pub pexels_query: String,
    /// Transcript excerpt around the trigger (helps the agent +
    /// reviewer evaluate the suggestion in context).
    pub transcript_excerpt: String,
}

/// One whisper word read off the sidecar.
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
Surface moments where stock B-roll (from Pexels) would land well, by \
scanning whisper transcripts for trigger phrases (\"look at\", \
\"imagine a\", \"picture this\", etc.) followed by a concrete visual \
noun (\"skyline\", \"office\", \"graph\", etc.).\
\n\nReturns findings: { asset_id, source_start_s, source_end_s, \
timeline_start_s, timeline_end_s, reason, pexels_query, \
transcript_excerpt }. Each finding is meant to become an \
EditorialNote of kind `broll_opportunity` — the agent (or user) then \
calls `search_broll(query)` to preview Pexels results and \
`use_broll(...)` to download + place a chosen clip.\
\n\nThis tool finds *opportunities for stock cutaways*. For \
*in-footage* cutaways (shots in the project that could cover another \
speaker), use `broll_candidates`.\
\n\nDistinct from a search: this surveys the transcript for moments. \
Empty findings means whisper hasn't landed yet OR the transcript \
genuinely has no demonstrative phrases — both common, neither an \
error. Findings are de-duplicated to one per `duration_s` window so \
the agent isn't flooded.\
";

#[cfg(test)]
mod tests {
    use super::*;
    use awidat_proto::otio::{
        Clip, ExternalReference, MediaReference, RationalTime, Stack, StackChild, TimeRange,
        Track, TrackChild, TrackKind,
    };

    fn write_whisper_words(
        project_root: &Path,
        asset_id: &str,
        words: Vec<(&str, f64, f64)>,
    ) {
        let path = whisper_sidecar_path(project_root, asset_id);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let payload = serde_json::json!({
            "indexer": "whisper",
            "asset_id": asset_id,
            "data": {
                "words": words
                    .into_iter()
                    .map(|(t, s, e)| serde_json::json!({"text": t, "start_s": s, "end_s": e}))
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
    fn finds_imagine_a_office_pattern() {
        let dir = tempfile::tempdir().unwrap();
        let asset = "raw/ep.mp4";
        // "imagine a busy office at 9am"
        write_whisper_words(
            dir.path(),
            asset,
            vec![
                ("imagine", 5.0, 5.4),
                ("a", 5.4, 5.5),
                ("busy", 5.5, 5.9),
                ("office", 5.9, 6.4),
                ("at", 6.4, 6.5),
                ("9am", 6.5, 7.0),
            ],
        );
        let tl = timeline_with_clip(asset, 0.0, 60.0);
        let findings = scan_broll_opportunities(dir.path(), &tl, None, 3.0, 10);
        assert_eq!(findings.len(), 1, "{findings:#?}");
        let f = &findings[0];
        assert!(f.pexels_query.contains("office"), "{}", f.pexels_query);
        assert!(f.reason.contains("office"));
        assert_eq!(f.source_start_s, 5.0);
    }

    #[test]
    fn skips_trigger_without_concrete_noun() {
        let dir = tempfile::tempdir().unwrap();
        let asset = "raw/ep.mp4";
        // "imagine that we did this differently" — no concrete noun
        write_whisper_words(
            dir.path(),
            asset,
            vec![
                ("imagine", 0.0, 0.4),
                ("that", 0.4, 0.6),
                ("we", 0.6, 0.7),
                ("did", 0.7, 0.9),
                ("this", 0.9, 1.0),
                ("differently", 1.0, 1.5),
            ],
        );
        let tl = timeline_with_clip(asset, 0.0, 30.0);
        let findings = scan_broll_opportunities(dir.path(), &tl, None, 3.0, 10);
        assert!(findings.is_empty(), "{findings:#?}");
    }

    #[test]
    fn drops_findings_outside_clip_source_range() {
        let dir = tempfile::tempdir().unwrap();
        let asset = "raw/ep.mp4";
        // Trigger at source 35s — outside the clip's [0, 10) range.
        write_whisper_words(
            dir.path(),
            asset,
            vec![
                ("look", 35.0, 35.3),
                ("at", 35.3, 35.4),
                ("the", 35.4, 35.5),
                ("skyline", 35.5, 36.0),
            ],
        );
        let tl = timeline_with_clip(asset, 0.0, 10.0);
        let findings = scan_broll_opportunities(dir.path(), &tl, None, 3.0, 10);
        assert!(findings.is_empty(), "{findings:#?}");
    }

    #[test]
    fn dedupes_close_findings() {
        let dir = tempfile::tempdir().unwrap();
        let asset = "raw/ep.mp4";
        // Two triggers within 1 second — second should be suppressed
        // because duration_s = 3.0.
        write_whisper_words(
            dir.path(),
            asset,
            vec![
                ("look", 1.0, 1.3),
                ("at", 1.3, 1.4),
                ("the", 1.4, 1.5),
                ("skyline", 1.5, 2.0),
                ("imagine", 2.2, 2.6),
                ("a", 2.6, 2.7),
                ("city", 2.7, 3.0),
            ],
        );
        let tl = timeline_with_clip(asset, 0.0, 30.0);
        let findings = scan_broll_opportunities(dir.path(), &tl, None, 3.0, 10);
        assert_eq!(findings.len(), 1, "{findings:#?}");
        // The first one wins.
        assert_eq!(findings[0].source_start_s, 1.0);
    }

    #[test]
    fn second_trigger_emits_when_far_enough_apart() {
        let dir = tempfile::tempdir().unwrap();
        let asset = "raw/ep.mp4";
        write_whisper_words(
            dir.path(),
            asset,
            vec![
                ("look", 1.0, 1.3),
                ("at", 1.3, 1.4),
                ("the", 1.4, 1.5),
                ("skyline", 1.5, 2.0),
                // 10 seconds later
                ("picture", 12.0, 12.4),
                ("this", 12.4, 12.7),
                ("graph", 12.7, 13.2),
            ],
        );
        let tl = timeline_with_clip(asset, 0.0, 30.0);
        let findings = scan_broll_opportunities(dir.path(), &tl, None, 3.0, 10);
        assert_eq!(findings.len(), 2, "{findings:#?}");
        assert!(findings[1].pexels_query.contains("graph"));
    }

    #[test]
    fn pexels_query_excludes_stop_words() {
        let dir = tempfile::tempdir().unwrap();
        let asset = "raw/ep.mp4";
        write_whisper_words(
            dir.path(),
            asset,
            vec![
                ("look", 0.0, 0.3),
                ("at", 0.3, 0.4),
                ("the", 0.4, 0.5),
                ("skyline", 0.5, 1.0),
                ("of", 1.0, 1.1),
                ("the", 1.1, 1.2),
                ("city", 1.2, 1.6),
            ],
        );
        let tl = timeline_with_clip(asset, 0.0, 30.0);
        let findings = scan_broll_opportunities(dir.path(), &tl, None, 3.0, 10);
        assert_eq!(findings.len(), 1);
        let q = &findings[0].pexels_query;
        assert!(q.contains("skyline"), "{q}");
        assert!(!q.split_whitespace().any(|w| w == "the" || w == "of"), "{q}");
    }

    #[test]
    fn empty_when_no_whisper_sidecar() {
        let dir = tempfile::tempdir().unwrap();
        let asset = "raw/no-sidecar.mp4";
        let tl = timeline_with_clip(asset, 0.0, 30.0);
        let findings = scan_broll_opportunities(dir.path(), &tl, None, 3.0, 10);
        assert!(findings.is_empty());
    }

    #[test]
    fn cap_respected() {
        let dir = tempfile::tempdir().unwrap();
        let asset = "raw/ep.mp4";
        // 5 valid triggers, 10s apart so dedup won't suppress them.
        let mut words: Vec<(&str, f64, f64)> = Vec::new();
        for i in 0..5 {
            let t = (i as f64) * 10.0;
            words.push(("look", t, t + 0.3));
            words.push(("at", t + 0.3, t + 0.4));
            words.push(("the", t + 0.4, t + 0.5));
            words.push(("skyline", t + 0.5, t + 1.0));
        }
        write_whisper_words(dir.path(), asset, words);
        let tl = timeline_with_clip(asset, 0.0, 100.0);
        let findings = scan_broll_opportunities(dir.path(), &tl, None, 3.0, 3);
        assert_eq!(findings.len(), 3);
    }

    #[test]
    fn case_insensitive_trigger_match() {
        let dir = tempfile::tempdir().unwrap();
        let asset = "raw/ep.mp4";
        write_whisper_words(
            dir.path(),
            asset,
            vec![
                ("LOOK", 0.0, 0.3),
                ("At", 0.3, 0.4),
                ("the", 0.4, 0.5),
                ("Skyline", 0.5, 1.0),
            ],
        );
        let tl = timeline_with_clip(asset, 0.0, 30.0);
        let findings = scan_broll_opportunities(dir.path(), &tl, None, 3.0, 10);
        assert_eq!(findings.len(), 1);
    }
}
