//! `run_sound_gates` — deterministic sound-pass gates over the current
//! program, scored against a house style profile: delivery loudness
//! (ffmpeg ebur128 on the newest render) and J/L-cut coverage at
//! whisper-derived speaker-turn boundaries.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use montage_eval::{HouseProfile, LoudnessStats, SplitEditStats, gates, parse_ebur128};
use montage_proto::otio::{MediaReference, Stack, StackChild, TrackChild, TrackKind};
use montage_proto::project::Project;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::montage_mcp::context::McpToolCtx;

/// Arguments to `run_sound_gates`.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct RunSoundGatesArgs {
    /// House profile name ("doac", "tbpn", "technologia"). Defaults to
    /// "technologia".
    #[serde(default)]
    pub profile: Option<String>,
}

#[derive(Debug, Clone)]
struct Word {
    start_s: f64,
    end_s: f64,
    speaker: String,
}

fn load_words(project_root: &Path, asset: &str) -> Option<Vec<Word>> {
    let path = project_root
        .join("index/whisper")
        .join(format!("{asset}.json"));
    let doc: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    let words = doc.pointer("/data/words")?.as_array()?;
    let mut out = Vec::with_capacity(words.len());
    for w in words {
        let start = w.get("start_s").or_else(|| w.get("start"))?.as_f64()?;
        let end = w.get("end_s").or_else(|| w.get("end"))?.as_f64()?;
        let speaker = w
            .get("speaker")
            .or_else(|| w.get("speaker_id"))
            .and_then(|s| s.as_str())
            .unwrap_or_default()
            .to_string();
        out.push(Word {
            start_s: start,
            end_s: end,
            speaker,
        });
    }
    Some(out)
}

fn first_video_track(stack: &Stack) -> Option<&montage_proto::otio::Track> {
    for child in &stack.children {
        match child {
            StackChild::Track(track) if track.kind == TrackKind::Video => return Some(track),
            StackChild::Stack(inner) => {
                if let Some(found) = first_video_track(inner) {
                    return Some(found);
                }
            }
            _ => {}
        }
    }
    None
}

/// `(boundary_time, has_j_or_l_cut)` for each speaker-turn boundary on the
/// first video track. A turn is a boundary between adjacent clips whose
/// whisper speaker labels differ across the cut; gaps break adjacency,
/// transitions are transparent.
fn speaker_turn_boundaries(project_root: &Path, stack: &Stack) -> Option<Vec<(f64, bool)>> {
    let track = first_video_track(stack)?;
    let mut cache: HashMap<String, Option<Vec<Word>>> = HashMap::new();
    let mut boundaries = Vec::new();
    let mut cursor = 0.0;
    let mut prev: Option<&montage_proto::otio::Clip> = None;
    for item in &track.children {
        match item {
            TrackChild::Clip(clip) => {
                if let (Some(out_clip), true) = (prev, cursor > 0.0) {
                    if let Some(entry) = judge_boundary(project_root, &mut cache, out_clip, clip) {
                        boundaries.push((cursor, entry));
                    }
                }
                cursor += clip
                    .source_range
                    .as_ref()
                    .map(|r| r.duration.to_seconds())
                    .unwrap_or(0.0);
                prev = Some(clip);
            }
            TrackChild::Gap(gap) => {
                cursor += gap.source_range.duration.to_seconds();
                prev = None; // no dialogue continuity across a gap
            }
            // Transitions decorate the boundary; the clips stay adjacent.
            _ => {}
        }
    }
    Some(boundaries)
}

/// `Some(covered)` when the pair is a speaker turn; `None` otherwise.
fn judge_boundary(
    project_root: &Path,
    cache: &mut HashMap<String, Option<Vec<Word>>>,
    out_clip: &montage_proto::otio::Clip,
    in_clip: &montage_proto::otio::Clip,
) -> Option<bool> {
    let mut speaker_at = |clip: &montage_proto::otio::Clip, last: bool| -> Option<String> {
        let MediaReference::External(ext) = &clip.media_reference else {
            return None;
        };
        let range = clip.source_range.as_ref()?;
        let (start, end) = (
            range.start_time.to_seconds(),
            range.start_time.to_seconds() + range.duration.to_seconds(),
        );
        let words = cache
            .entry(ext.target_url.clone())
            .or_insert_with(|| load_words(project_root, &ext.target_url))
            .as_ref()?;
        let overlapping = words
            .iter()
            .filter(|w| w.end_s > start && w.start_s < end && !w.speaker.is_empty());
        let word = if last {
            overlapping.last()
        } else {
            overlapping.into_iter().next()
        }?;
        Some(word.speaker.clone())
    };
    let out_speaker = speaker_at(out_clip, true)?;
    let in_speaker = speaker_at(in_clip, false)?;
    if out_speaker == in_speaker {
        return None;
    }
    let has_split =
        |spec: Option<&montage_proto::montage_meta::SplitEditSpec>, lead: bool| -> bool {
            spec.map(|s| {
                let v = if lead {
                    s.audio_lead_s
                } else {
                    s.audio_trail_s
                };
                v.is_some_and(|x| x > 0.0)
            })
            .unwrap_or(false)
        };
    let out_spec = out_clip
        .metadata
        .montage
        .as_ref()
        .and_then(|m| m.split_edit.as_ref());
    let in_spec = in_clip
        .metadata
        .montage
        .as_ref()
        .and_then(|m| m.split_edit.as_ref());
    Some(has_split(out_spec, false) || has_split(in_spec, true))
}

fn newest_render(project_root: &Path) -> Option<PathBuf> {
    let mut newest: Option<(std::time::SystemTime, PathBuf)> = None;
    for entry in std::fs::read_dir(project_root.join("renders"))
        .ok()?
        .flatten()
    {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "mp4") {
            let mtime = entry.metadata().ok()?.modified().ok()?;
            if newest.as_ref().is_none_or(|(t, _)| mtime > *t) {
                newest = Some((mtime, path));
            }
        }
    }
    newest.map(|(_, p)| p)
}

/// Run `run_sound_gates` against the project resolved from [`McpToolCtx`].
pub fn run(args: RunSoundGatesArgs, ctx: McpToolCtx) -> Result<String, String> {
    let project = Project::read(&ctx.project_root)
        .map_err(|e| format!("run_sound_gates: failed to read project: {e}"))?;
    let profile_name = args.profile.as_deref().unwrap_or("technologia");
    let profile = HouseProfile::builtin(profile_name).ok_or_else(|| {
        format!(
            "run_sound_gates: unknown profile {profile_name:?} \
             (builtins: doac, tbpn, technologia)"
        )
    })?;

    let boundaries = speaker_turn_boundaries(&ctx.project_root, &project.timeline.tracks)
        .ok_or_else(|| "run_sound_gates: no video track in timeline".to_string())?;
    let split_stats = SplitEditStats::from_boundaries(&boundaries);
    let covered = boundaries.iter().filter(|(_, c)| *c).count();

    let render = newest_render(&ctx.project_root);
    let (loudness_report, loudness_json, render_json) = match &render {
        None => (
            // A pre-publish gate must fail closed when the artifact it
            // judges does not exist (matching the eval gates'
            // unknown-archetype precedent).
            montage_eval::gates::GateReport {
                gate: "sound.loudness",
                passed: false,
                measured: f64::NAN,
                detail: "no rendered MP4 under renders/ — render the program \
                         before running sound gates"
                    .to_string(),
            },
            serde_json::Value::Null,
            serde_json::Value::Null,
        ),
        Some(path) => {
            let ffmpeg = montage_render::ffmpeg_path()
                .map_err(|e| format!("run_sound_gates: ffmpeg unavailable: {e}"))?;
            let out = std::process::Command::new(ffmpeg)
                .args(["-hide_banner", "-nostats", "-i"])
                .arg(path)
                .args(["-af", "ebur128=peak=true", "-f", "null", "-"])
                .output()
                .map_err(|e| format!("run_sound_gates: ffmpeg spawn failed: {e}"))?;
            let stderr = String::from_utf8_lossy(&out.stderr);
            if !out.status.success() {
                let tail: String = stderr
                    .chars()
                    .rev()
                    .take(400)
                    .collect::<String>()
                    .chars()
                    .rev()
                    .collect();
                return Err(format!("run_sound_gates: ffmpeg failed: {tail}"));
            }
            let stats: LoudnessStats = parse_ebur128(&stderr).ok_or_else(|| {
                "run_sound_gates: could not parse ebur128 summary from ffmpeg output".to_string()
            })?;
            let rel = path
                .strip_prefix(&ctx.project_root)
                .unwrap_or(path)
                .to_string_lossy()
                .to_string();
            (
                gates::loudness(&stats, &profile),
                serde_json::json!({
                    "integrated_lufs": stats.integrated_lufs,
                    "true_peak_db": stats.true_peak_db,
                }),
                serde_json::Value::String(rel),
            )
        }
    };

    let coverage_report = gates::split_edit_coverage(&split_stats, &profile);
    let passed = loudness_report.passed && coverage_report.passed;
    let body = serde_json::json!({
        "profile": profile.name,
        "render_path": render_json,
        "loudness": loudness_json,
        "speaker_turns": {"total": split_stats.total(), "covered": covered},
        "reports": [loudness_report, coverage_report],
        "passed": passed,
    });
    serde_json::to_string(&body).map_err(|e| format!("run_sound_gates serialize: {e}"))
}

pub const DESCRIPTION: &str = "\
Run deterministic sound-pass gates against a house style profile (doac, \
tbpn, technologia): loudness (integrated LUFS within the delivery target \
±tolerance and true peak under the ceiling, measured with ffmpeg ebur128 \
on the newest MP4 under renders/ — fails closed when no render exists) \
and split_edit_coverage (fraction of speaker-turn boundaries carrying a \
J/L-cut, from clip split_edit metadata and whisper speaker labels; skips \
when the program has no speaker turns). Returns per-gate reports with \
measured values and an overall `passed`. Read-only; run after the sound \
pass and before export/publish.\
";
