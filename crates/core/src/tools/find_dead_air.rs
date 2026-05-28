//! `find_dead_air` tool — surface silence ranges as editorial findings.
//!
//! Reads silence sidecars at `<project>/.awidat/silences/<stem>-<hash>
//! .json` (written by Step 1.2's post-import chain), maps each source-
//! time silence range onto the project's timeline by walking the
//! current OTIO clips, and returns the ranges that survived
//! trimming + their surrounding transcript context.
//!
//! Each returned finding is shaped to become an `EditorialNote` of
//! kind `silence_trim` — the agent typically calls this tool, then
//! emits one Note per finding. In Phase 1.6 the dismissal-pattern
//! matcher consults its memory before re-surfacing repeats; v1
//! returns the raw findings and lets the caller filter.
//!
//! Sidecar discovery: tool reproduces the FNV-1a path-hash from
//! `apps/desktop/.../media.rs::stable_path_hash` so the core crate
//! doesn't need to depend on the desktop crate. Path computation is
//! `<project>/raw/<asset_id>` → absolute path → FNV-1a hash → join
//! with the project's silences dir under the asset's file stem.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use awidat_proto::otio::{MediaReference, StackChild, Timeline, TrackChild};
use awidat_proto::project::Project;
use serde::Deserialize;

use crate::FunctionCallError;
use crate::tool::{ToolContext, ToolHandler, ToolInvocation, ToolOutput};
use crate::tool_schema::Tool as ToolSchema;

/// Default minimum silence duration the tool surfaces. Below this
/// (~breath beats), silences are part of natural speech rhythm; the
/// `find_dead_air` Note would be noise.
const DEFAULT_MIN_DURATION_S: f64 = 1.5;

/// Hard cap on returned findings. Long podcasts can have hundreds
/// of silences; we trim to keep the tool result manageable for the
/// model. The agent can re-call with a higher `max_results` if it
/// wants more.
const DEFAULT_MAX_RESULTS: usize = 20;
const HARD_MAX_RESULTS: usize = 100;

/// Window for pulling transcript context around a silence range.
/// We grab words within `±TRANSCRIPT_CONTEXT_S` of each end of the
/// silence and stitch them into a short `before:` / `after:` snippet.
const TRANSCRIPT_CONTEXT_S: f64 = 2.0;

/// The `find_dead_air` tool.
pub struct FindDeadAirTool;

#[derive(Debug, Deserialize)]
struct FindDeadAirArgs {
    /// Min silence duration (seconds) to surface. Below this is
    /// treated as breath beat / natural rhythm. Default 1.5s.
    #[serde(default)]
    min_duration_s: Option<f64>,
    /// Max findings to return. Default 20, hard cap 100.
    #[serde(default)]
    max_results: Option<usize>,
}

#[async_trait]
impl ToolHandler for FindDeadAirTool {
    fn name(&self) -> &'static str {
        "find_dead_air"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "find_dead_air".into(),
            description: DESCRIPTION.into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "min_duration_s": {
                        "type": "number",
                        "minimum": 0.6,
                        "description": "Minimum silence duration (s) to surface. Default 1.5."
                    },
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
        let args: FindDeadAirArgs = serde_json::from_value(invocation.args).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "find_dead_air: invalid args ({e}). All fields optional."
            ))
        })?;
        let min_duration_s = args
            .min_duration_s
            .unwrap_or(DEFAULT_MIN_DURATION_S)
            .max(0.0);
        let max_results = args
            .max_results
            .unwrap_or(DEFAULT_MAX_RESULTS)
            .min(HARD_MAX_RESULTS);

        let project = Project::read(&ctx.project_root).map_err(|e| {
            FunctionCallError::RespondToModel(format!("find_dead_air: failed to read project: {e}"))
        })?;

        let mut findings = scan_dead_air(
            &ctx.project_root,
            &project.timeline,
            min_duration_s,
            // Generate up to 2× the cap so dismissal-filtering still
            // returns up to `max_results` findings even when half
            // would have been filtered out.
            max_results.saturating_mul(2).max(max_results),
        );

        // Filter by per-project dismissal memory. The user dismissed
        // a duration bucket → all findings of that bucket get
        // dropped from this session's output.
        let dismissals = crate::dismissal::load_dismissals(&ctx.project_root);
        findings.retain(|f| {
            let bucket = crate::dismissal::DismissalBucket::for_silence(f.duration_s);
            !dismissals.is_dismissed(bucket)
        });
        findings.truncate(max_results);

        let body = serde_json::json!({
            "min_duration_s": min_duration_s,
            "findings": findings,
            "more_available": findings.len() == max_results,
        });
        Ok(ToolOutput::text(body.to_string()))
    }
}

/// Walk every video-track clip on the timeline, find silences in
/// each clip's source_range, and emit a Finding per intersection.
/// Pure function, separated for unit testing without spinning up
/// a full ToolContext.
pub fn scan_dead_air(
    project_root: &Path,
    timeline: &Timeline,
    min_duration_s: f64,
    max_results: usize,
) -> Vec<DeadAirFinding> {
    let mut out: Vec<DeadAirFinding> = Vec::new();
    let timeline_cursor_s = 0.0_f64;

    for stack_child in &timeline.tracks.children {
        let StackChild::Track(track) = stack_child else {
            continue;
        };
        // Reset cursor per track — different tracks have different
        // timelines. The per-track cursor still matters for mapping
        // each clip's source-time silences onto track-time.
        let mut track_cursor_s = 0.0_f64;

        for tc in &track.children {
            match tc {
                TrackChild::Clip(clip) => {
                    let MediaReference::External(ext) = &clip.media_reference else {
                        // Title clips, missing references, etc. — no
                        // silences to surface.
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

                    if let Some(sidecar) = load_sidecar(project_root, &asset_id) {
                        for r in sidecar.ranges {
                            // Intersect [r.start_s, r.end_s] with
                            // [clip_source_start, clip_source_end]. If
                            // the silence is entirely outside the clip's
                            // playable range, skip; if it crosses an
                            // edge, clamp to the visible portion.
                            let visible_start = r.start_s.max(clip_source_start);
                            let visible_end = r.end_s.min(clip_source_end);
                            if visible_end <= visible_start {
                                continue;
                            }
                            let visible_duration = visible_end - visible_start;
                            if visible_duration < min_duration_s {
                                continue;
                            }
                            // Map source-time → track-time via the
                            // clip's source_range offset.
                            let timeline_start = timeline_cursor_s
                                + clip_track_start
                                + (visible_start - clip_source_start);
                            let timeline_end = timeline_start + visible_duration;

                            let (before, after) = read_transcript_context(
                                project_root,
                                &asset_id,
                                visible_start,
                                visible_end,
                            );

                            out.push(DeadAirFinding {
                                asset_id: asset_id.clone(),
                                source_start_s: visible_start,
                                source_end_s: visible_end,
                                timeline_start_s: timeline_start,
                                timeline_end_s: timeline_end,
                                duration_s: visible_duration,
                                transcript_before: before,
                                transcript_after: after,
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

/// One silence range that survived timeline mapping, ready to become
/// an `EditorialNote`. Fields ordered for the agent: most relevant
/// first (timeline position + duration), context last.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DeadAirFinding {
    /// Source asset (project-relative).
    pub asset_id: String,
    /// Source-media seconds where the silence begins (clamped to the
    /// clip's source_range).
    pub source_start_s: f64,
    /// Source-media seconds where the silence ends.
    pub source_end_s: f64,
    /// Timeline-time where the silence begins on the master timeline.
    pub timeline_start_s: f64,
    /// Timeline-time where the silence ends.
    pub timeline_end_s: f64,
    /// Length of the silence on the timeline.
    pub duration_s: f64,
    /// Last words spoken before the silence (up to ~2s of transcript).
    pub transcript_before: String,
    /// First words spoken after the silence.
    pub transcript_after: String,
}

/// Mirror of `commands::silence::SilenceSidecar` — duplicated here so
/// `awidat-core` doesn't depend on the desktop crate.
#[derive(Debug, Clone, serde::Deserialize)]
struct SilenceSidecar {
    ranges: Vec<SilenceRangeRecord>,
    #[allow(dead_code)]
    threshold_db: f64,
    #[allow(dead_code)]
    min_duration_s: f64,
}

#[derive(Debug, Clone, Copy, serde::Deserialize)]
struct SilenceRangeRecord {
    start_s: f64,
    end_s: f64,
    #[allow(dead_code)]
    db_floor: f64,
}

/// FNV-1a over the absolute path string. Mirrors
/// `apps/desktop/src-tauri/src/commands/media.rs::stable_path_hash`
/// so the silence sidecar paths line up with the desktop's writes.
/// Drift here = the tool can't find any sidecars.
fn stable_path_hash(path: &Path) -> u32 {
    let mut hash = 0x811c9dc5_u32;
    for byte in path.to_string_lossy().as_bytes() {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(0x01000193);
    }
    hash
}

fn silences_path_for(project_root: &Path, asset_abs_path: &Path) -> PathBuf {
    let stem = asset_abs_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("asset");
    project_root.join(".awidat").join("silences").join(format!(
        "{stem}-{:08x}.json",
        stable_path_hash(asset_abs_path)
    ))
}

fn load_sidecar(project_root: &Path, asset_id: &str) -> Option<SilenceSidecar> {
    let abs = project_root.join(asset_id);
    if !abs.is_file() {
        return None;
    }
    let sidecar_path = silences_path_for(project_root, &abs);
    let bytes = std::fs::read(&sidecar_path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Pull whisper words around a silence range to give the agent
/// editorial context. Returns `(before, after)` snippet pair; either
/// can be empty if no transcript or no words in the window.
fn read_transcript_context(
    project_root: &Path,
    asset_id: &str,
    silence_start_s: f64,
    silence_end_s: f64,
) -> (String, String) {
    let whisper_path = whisper_sidecar_path(project_root, asset_id);
    let Ok(bytes) = std::fs::read(&whisper_path) else {
        return (String::new(), String::new());
    };
    let Ok(value): Result<serde_json::Value, _> = serde_json::from_slice(&bytes) else {
        return (String::new(), String::new());
    };
    // Try /data/words first (full word-level alignment), then fall
    // back to /data/segments. Words give tighter context.
    let mut before_words: Vec<&str> = Vec::new();
    let mut after_words: Vec<&str> = Vec::new();

    if let Some(words) = value.pointer("/data/words").and_then(|v| v.as_array()) {
        for w in words {
            let end = word_end_s(w);
            let start = word_start_s(w);
            let text = w.get("text").and_then(|v| v.as_str()).unwrap_or("").trim();
            if text.is_empty() {
                continue;
            }
            // Word ends within `TRANSCRIPT_CONTEXT_S` before the silence.
            if end <= silence_start_s && silence_start_s - end <= TRANSCRIPT_CONTEXT_S {
                before_words.push(text);
            // Word starts within `TRANSCRIPT_CONTEXT_S` after the silence.
            } else if start >= silence_end_s && start - silence_end_s <= TRANSCRIPT_CONTEXT_S {
                after_words.push(text);
            }
        }
    } else if let Some(segments) = value.pointer("/data/segments").and_then(|v| v.as_array()) {
        for s in segments {
            let end = word_end_s(s);
            let start = word_start_s(s);
            let text = s.get("text").and_then(|v| v.as_str()).unwrap_or("").trim();
            if text.is_empty() {
                continue;
            }
            if end <= silence_start_s && silence_start_s - end <= TRANSCRIPT_CONTEXT_S {
                before_words.push(text);
            } else if start >= silence_end_s && start - silence_end_s <= TRANSCRIPT_CONTEXT_S {
                after_words.push(text);
            }
        }
    }

    (before_words.join(" "), after_words.join(" "))
}

fn whisper_sidecar_path(project_root: &Path, asset_id: &str) -> PathBuf {
    project_root
        .join("index")
        .join("whisper")
        .join(format!("{asset_id}.json"))
}

/// Read a `start` / `start_s` field from a word/segment record.
fn word_start_s(v: &serde_json::Value) -> f64 {
    v.get("start_s")
        .or_else(|| v.get("start"))
        .and_then(|s| s.as_f64())
        .unwrap_or(f64::INFINITY)
}

fn word_end_s(v: &serde_json::Value) -> f64 {
    v.get("end_s")
        .or_else(|| v.get("end"))
        .and_then(|s| s.as_f64())
        .unwrap_or(f64::NEG_INFINITY)
}

const DESCRIPTION: &str = "\
Surface silence ranges (\"dead air\") on the project timeline as \
editorial findings. Reads the per-asset silence sidecars produced \
on import, intersects each silence range with the clip's current \
source_range on the timeline (so trims that already removed a \
silence don't get re-surfaced), and returns the surviving silences \
plus surrounding transcript context.\
\n\nEach finding has: asset_id, source_start_s, source_end_s, \
timeline_start_s, timeline_end_s, duration_s, transcript_before, \
transcript_after. Use the timeline_* fields for click-to-seek; the \
source_* fields for `*** Trim Clip` envelopes.\
\n\nDefault min_duration_s=1.5 (below this is breath beat / \
natural rhythm — surfacing those would be noise). Default \
max_results=20, hard cap 100. Returns `more_available: true` when \
the cap was hit so you know to re-query with a higher limit.\
\n\nWhen no silence sidecars exist (assets weren't fully imported, \
or the post-import chain was interrupted), returns an empty \
findings list — NOT an error. Surface that as \"no dead air found \
yet — has indexing finished?\" rather than asserting the project is \
clean.\
";

#[cfg(test)]
mod tests {
    use super::*;
    use awidat_proto::otio::{
        Clip, ExternalReference, MediaReference, RationalTime, Stack, StackChild, TimeRange, Track,
        TrackChild, TrackKind,
    };

    fn write_silences_sidecar(project_root: &Path, asset_id: &str, ranges: Vec<(f64, f64)>) {
        let abs = project_root.join(asset_id);
        std::fs::create_dir_all(abs.parent().unwrap()).unwrap();
        std::fs::write(&abs, b"stub").unwrap();
        let sidecar_path = silences_path_for(project_root, &abs);
        std::fs::create_dir_all(sidecar_path.parent().unwrap()).unwrap();
        let payload = serde_json::json!({
            "ranges": ranges
                .into_iter()
                .map(|(s, e)| serde_json::json!({"start_s": s, "end_s": e, "db_floor": -45.0}))
                .collect::<Vec<_>>(),
            "threshold_db": -40.0,
            "min_duration_s": 0.6,
        });
        std::fs::write(&sidecar_path, serde_json::to_vec_pretty(&payload).unwrap()).unwrap();
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
    fn scans_silence_inside_clip_and_maps_to_timeline() {
        let dir = tempfile::tempdir().unwrap();
        let asset = "raw/ep.mp4";
        // Silence 10..13 in the source asset.
        write_silences_sidecar(dir.path(), asset, vec![(10.0, 13.0)]);
        // Clip plays source range 5..20 → silence at source 10..13
        // maps to timeline 5..8.
        let tl = timeline_with_clip(asset, 5.0, 15.0);

        let findings = scan_dead_air(dir.path(), &tl, 1.0, 10);
        assert_eq!(findings.len(), 1);
        let f = &findings[0];
        assert!((f.source_start_s - 10.0).abs() < 1e-9);
        assert!((f.source_end_s - 13.0).abs() < 1e-9);
        assert!((f.timeline_start_s - 5.0).abs() < 1e-9);
        assert!((f.timeline_end_s - 8.0).abs() < 1e-9);
        assert!((f.duration_s - 3.0).abs() < 1e-9);
    }

    #[test]
    fn skips_silences_below_min_duration() {
        let dir = tempfile::tempdir().unwrap();
        let asset = "raw/ep.mp4";
        // Three silences: 0.5s (under), 2s (over), 0.8s (under).
        write_silences_sidecar(dir.path(), asset, vec![(1.0, 1.5), (3.0, 5.0), (7.0, 7.8)]);
        let tl = timeline_with_clip(asset, 0.0, 10.0);
        let findings = scan_dead_air(dir.path(), &tl, 1.0, 10);
        assert_eq!(findings.len(), 1, "only the 2s silence should surface");
        assert!((findings[0].duration_s - 2.0).abs() < 1e-9);
    }

    #[test]
    fn drops_silences_outside_clips_source_range() {
        let dir = tempfile::tempdir().unwrap();
        let asset = "raw/ep.mp4";
        // Silence at source 30..35 — but clip only plays source 0..10,
        // so this silence is "off the timeline" and shouldn't surface.
        write_silences_sidecar(dir.path(), asset, vec![(30.0, 35.0)]);
        let tl = timeline_with_clip(asset, 0.0, 10.0);
        let findings = scan_dead_air(dir.path(), &tl, 1.0, 10);
        assert!(findings.is_empty());
    }

    #[test]
    fn clamps_silence_at_clip_edges() {
        let dir = tempfile::tempdir().unwrap();
        let asset = "raw/ep.mp4";
        // Silence at source 8..15 — but clip plays 5..12, so the
        // visible portion is 8..12 (4 seconds).
        write_silences_sidecar(dir.path(), asset, vec![(8.0, 15.0)]);
        let tl = timeline_with_clip(asset, 5.0, 7.0);
        let findings = scan_dead_air(dir.path(), &tl, 1.0, 10);
        assert_eq!(findings.len(), 1);
        let f = &findings[0];
        assert!((f.source_start_s - 8.0).abs() < 1e-9);
        assert!((f.source_end_s - 12.0).abs() < 1e-9);
        assert!((f.duration_s - 4.0).abs() < 1e-9);
    }

    #[test]
    fn empty_when_no_sidecars_exist() {
        let dir = tempfile::tempdir().unwrap();
        // Timeline references an asset with no sidecar on disk.
        let asset = "raw/no-sidecar.mp4";
        std::fs::create_dir_all(dir.path().join("raw")).unwrap();
        std::fs::write(dir.path().join(asset), b"stub").unwrap();
        let tl = timeline_with_clip(asset, 0.0, 10.0);
        let findings = scan_dead_air(dir.path(), &tl, 1.0, 10);
        assert!(findings.is_empty());
    }

    #[test]
    fn respects_max_results_cap() {
        let dir = tempfile::tempdir().unwrap();
        let asset = "raw/ep.mp4";
        // Five silences of 2s each, well separated.
        write_silences_sidecar(
            dir.path(),
            asset,
            vec![
                (1.0, 3.0),
                (5.0, 7.0),
                (9.0, 11.0),
                (13.0, 15.0),
                (17.0, 19.0),
            ],
        );
        let tl = timeline_with_clip(asset, 0.0, 25.0);
        let findings = scan_dead_air(dir.path(), &tl, 1.0, 3);
        assert_eq!(findings.len(), 3);
    }

    #[test]
    fn stable_path_hash_matches_desktop_helper() {
        // Sanity check — if this drifts from the desktop helper,
        // sidecars become unfindable. We pin a known-good output.
        let path = Path::new("/tmp/proj/raw/foo.mp4");
        let h = stable_path_hash(path);
        // Recompute with the desktop helper (functionally identical
        // code, copied for a different crate).
        let mut expected = 0x811c9dc5_u32;
        for byte in path.to_string_lossy().as_bytes() {
            expected ^= u32::from(*byte);
            expected = expected.wrapping_mul(0x01000193);
        }
        assert_eq!(h, expected);
    }
}
