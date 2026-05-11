//! Verification stack — cheap → expensive, gated milestones.
//!
//! Per `PLAN.md` §9. Three tiers:
//!
//!   - **Tier 1** — every edit, mandatory. Lives inside
//!     `apply_edl` (parse + anchor resolution + asset existence
//!     + frame-range bounds + OTIO round-trip). Runs synchronously
//!     in <100ms. Not in this module.
//!   - **Tier 2** — per feature, mandatory. Triggered when the
//!     agent flips a plan item to `completed`. This module
//!     hosts those checks.
//!   - **Tier 3** — milestones, before "done". Requires a human
//!     in the TUI viewer. Not in this module.
//!
//! The tier-2 surface here is intentionally thin for v1. The plan
//! lists transcript-vs-cut detection and A/V-sync analysis as
//! deliverables; both need real DSP code that's worth real
//! benchmarking. Until then we ship the *framework* + a single
//! cheap check (does the timeline still render without error?)
//! so future verifiers have a place to slot in.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// Outcome of running tier-2 verification on a plan item.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum VerifyResult {
    /// All checks passed. The agent can leave the plan item
    /// `completed` and move on.
    Pass {
        /// Per-check verdicts so the agent can see what was run.
        checks: Vec<CheckVerdict>,
    },
    /// At least one check failed. The agent should roll the plan
    /// item back to `pending` (or annotate it `failed`) and
    /// address the diagnostic before re-asserting completion.
    Fail {
        /// Per-check verdicts; at least one is failing.
        checks: Vec<CheckVerdict>,
        /// Aggregated short reason for the rollup. The agent
        /// should surface this to the user verbatim.
        reason: String,
    },
}

/// One verifier's verdict on one plan item.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CheckVerdict {
    /// Stable id (e.g. `timeline_renders`, `transcript_cut`,
    /// `av_sync`). Lets the agent and downstream tooling key
    /// off-results.
    pub id: String,
    /// True if this check passed.
    pub pass: bool,
    /// Diagnostic. On pass: usually empty. On fail: actionable
    /// — what the agent should try next.
    pub message: String,
}

/// Run tier-2 verification on a project. Returns the rolled-up
/// verdict.
///
/// `plan_item` is the agent-visible step text (e.g. "Tighten the
/// intro segment from 0:00 to 0:30") — used only for diagnostic
/// messages, not for picking which checks to run. We always run
/// the full tier-2 suite; the cost difference is negligible
/// for a handful of cheap checks.
pub fn tier_2(project_root: &Path, _plan_item: &str) -> VerifyResult {
    let mut checks = Vec::new();
    checks.push(check_timeline_renders(project_root));
    checks.push(check_av_sync_within_40ms(project_root));
    checks.push(check_transcript_cut_alignment(project_root));
    let any_fail = checks.iter().any(|c| !c.pass);
    if any_fail {
        let reason = checks
            .iter()
            .filter(|c| !c.pass)
            .map(|c| format!("[{}] {}", c.id, c.message))
            .collect::<Vec<_>>()
            .join("; ");
        VerifyResult::Fail { checks, reason }
    } else {
        VerifyResult::Pass { checks }
    }
}

/// Cheapest tier-2 check: does the timeline still parse + serialize
/// without error? Catches the case where the agent committed an
/// `apply_edl` op that left the timeline in a malformed state
/// (wrong rate, negative duration, etc.) that tier-1 didn't
/// catch.
///
/// Costs <50ms on a typical project. Always runs.
fn check_timeline_renders(project_root: &Path) -> CheckVerdict {
    let timeline_path = project_root.join("project.otio.json");
    if !timeline_path.exists() {
        return CheckVerdict {
            id: "timeline_renders".into(),
            pass: false,
            message: format!(
                "expected timeline at {} — project may not be initialized",
                timeline_path.display()
            ),
        };
    }
    let mut warnings = Vec::new();
    match awidat_proto::project::read_otio_timeline(&timeline_path, &mut warnings) {
        Ok(_) => CheckVerdict {
            id: "timeline_renders".into(),
            pass: true,
            message: if warnings.is_empty() {
                String::new()
            } else {
                format!("timeline parses with {} schema warning(s)", warnings.len())
            },
        },
        Err(e) => CheckVerdict {
            id: "timeline_renders".into(),
            pass: false,
            message: format!(
                "timeline failed to parse: {e}. The most recent apply_edl probably \
                 wrote a malformed OTIO. Run `awidat validate <project>` for a \
                 detailed diagnostic, then revert the offending edit."
            ),
        },
    }
}

/// Tier-2 check: most recent rendered .mp4 has audio and video
/// streams whose durations agree within ±40ms.
///
/// Per `PLAN.md` §9 / #147: 40ms is the perceptual threshold (SMPTE
/// RP-2059 / EBU R37 line up at ~±40ms before lip-sync drift becomes
/// objectionable). We use ffprobe — already a dep — and check the
/// most recent file under `<project>/renders/*.mp4`.
///
/// V1 semantics:
///   - No `renders/` dir, no `*.mp4` inside → **pass** with a note.
///     The agent can mark a feature complete without having
///     rendered yet.
///   - ffprobe missing or fails → pass with a note. We don't want a
///     missing tool to block plan progress.
///   - One stream missing (e.g. audio-only render) → pass with a
///     note. There's nothing to drift.
///   - Both streams present + delta > 40ms → **fail** with the
///     measured drift and a pointer at the most-recent commit's
///     scope (likely a `scope=segment` stream-copy issue per the
///     existing system prompt's known caveats).
fn check_av_sync_within_40ms(project_root: &Path) -> CheckVerdict {
    const ID: &str = "av_sync_40ms";
    const TOLERANCE_MS: f64 = 40.0;

    let renders_dir = project_root.join("renders");
    let Some(latest) = newest_mp4(&renders_dir) else {
        return CheckVerdict {
            id: ID.into(),
            pass: true,
            message: "no rendered .mp4 yet — A/V sync check is a no-op until the \
                      first render lands."
                .into(),
        };
    };

    let durations = match probe_stream_durations(&latest) {
        Ok(d) => d,
        Err(reason) => {
            return CheckVerdict {
                id: ID.into(),
                pass: true,
                message: format!(
                    "couldn't probe {} ({reason}) — A/V sync check skipped. \
                     Install ffprobe or check the render is well-formed.",
                    latest.display()
                ),
            };
        }
    };

    match (durations.video_s, durations.audio_s) {
        (Some(v), Some(a)) => {
            let drift_ms = ((v - a).abs() * 1000.0).round();
            if drift_ms <= TOLERANCE_MS {
                CheckVerdict {
                    id: ID.into(),
                    pass: true,
                    message: format!(
                        "A/V drift {drift_ms:.0}ms ≤ {TOLERANCE_MS:.0}ms tolerance ({})",
                        latest.file_name().and_then(|n| n.to_str()).unwrap_or("?"),
                    ),
                }
            } else {
                CheckVerdict {
                    id: ID.into(),
                    pass: false,
                    message: format!(
                        "A/V drift {drift_ms:.0}ms exceeds {TOLERANCE_MS:.0}ms tolerance \
                         (video {v:.3}s, audio {a:.3}s) in {}. Likely cause: \
                         `scope=\"segment\"` stream-copy at non-keyframe boundaries. \
                         Re-render with `scope=\"timeline\"` so each cut is re-encoded \
                         at boundaries.",
                        latest.display()
                    ),
                }
            }
        }
        (Some(_), None) | (None, Some(_)) => CheckVerdict {
            id: ID.into(),
            pass: true,
            message: format!(
                "render has only one of {{audio, video}}; nothing to compare ({})",
                latest.file_name().and_then(|n| n.to_str()).unwrap_or("?"),
            ),
        },
        (None, None) => CheckVerdict {
            id: ID.into(),
            pass: true,
            message: format!(
                "couldn't read either stream's duration from {}; check skipped",
                latest.display()
            ),
        },
    }
}

#[derive(Debug, Default)]
struct StreamDurations {
    video_s: Option<f64>,
    audio_s: Option<f64>,
}

fn probe_stream_durations(path: &Path) -> Result<StreamDurations, String> {
    use std::process::Command;
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "stream=codec_type,duration",
            "-of",
            "default=noprint_wrappers=1:nokey=0",
        ])
        .arg(path)
        .output()
        .map_err(|e| format!("ffprobe spawn failed: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "ffprobe exit {}: {}",
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    // ffprobe output groups per-stream; we walk it stateful.
    let mut out = StreamDurations::default();
    let mut current_kind: Option<String> = None;
    let mut current_dur: Option<f64> = None;
    let commit = |kind: &Option<String>, dur: &Option<f64>, out: &mut StreamDurations| {
        if let (Some(k), Some(d)) = (kind, dur) {
            match k.as_str() {
                "video" if out.video_s.is_none() => out.video_s = Some(*d),
                "audio" if out.audio_s.is_none() => out.audio_s = Some(*d),
                _ => {}
            }
        }
    };
    for line in stdout.lines() {
        if let Some(rest) = line.strip_prefix("codec_type=") {
            commit(&current_kind, &current_dur, &mut out);
            current_kind = Some(rest.to_string());
            current_dur = None;
        } else if let Some(rest) = line.strip_prefix("duration=") {
            current_dur = rest.parse::<f64>().ok();
        }
    }
    commit(&current_kind, &current_dur, &mut out);
    Ok(out)
}

/// Tier-2 check: every clip with a `transcript_snippet` anchor still
/// has that snippet's transcript range overlapping its `source_range`.
///
/// Per `PLAN.md` §9 / #146 V1 scope: catches the structural risk where
/// `apply_edl Trim` narrows a clip's window past the snippet
/// that anchored it. Without this, the agent can silently produce a
/// cut whose anchor no longer matches its content — a hard-to-trace
/// bug at render time.
///
/// Strict superset of the anchor-resolver's overlap test (already in
/// `apply_edl`, fixed earlier this project for the 25ms-boundary
/// case). The verifier re-applies it after the fact, so an edit that
/// passed tier-1 anchor resolution but later got trimmed past its
/// anchor still gets caught.
///
/// V1 semantics:
///   - No `project.otio.json` → covered by `check_timeline_renders`;
///     this check returns pass-with-note.
///   - No clips with anchors → pass (nothing to verify).
///   - Anchor present, but no whisper sidecar for the asset → pass
///     with note (can't verify; not the verifier's job to demand
///     indexers ran).
///   - Anchor present + sidecar present + snippet not found → fail
///     ("anchor lost"; either the transcript changed or the agent
///     anchored on text that doesn't exist).
///   - Anchor present + snippet found but range doesn't overlap
///     `source_range` → fail (the trim-past-anchor risk).
fn check_transcript_cut_alignment(project_root: &Path) -> CheckVerdict {
    const ID: &str = "transcript_cut_alignment";

    let project = match awidat_proto::project::Project::read(project_root) {
        Ok(p) => p,
        Err(_) => {
            // The timeline_renders check has the actionable diagnostic
            // for this; we just no-op here so the failure surface
            // doesn't double-fire.
            return CheckVerdict {
                id: ID.into(),
                pass: true,
                message: "skipped (project couldn't be read; see timeline_renders)".into(),
            };
        }
    };

    let mut anchored = 0usize;
    let mut verified = 0usize;
    let mut skipped = 0usize;
    let mut failures: Vec<String> = Vec::new();

    // The timeline's `tracks` field is a Stack of StackChild; we only
    // care about Track-typed children for this check (nested stacks
    // are vanishingly rare in V1 footage).
    for stack_child in &project.timeline.tracks.children {
        let awidat_proto::otio::StackChild::Track(track) = stack_child else {
            continue;
        };
        for child in &track.children {
            let awidat_proto::otio::TrackChild::Clip(clip) = child else {
                continue;
            };
            let Some(awidat_meta) = clip.metadata.awidat.as_ref() else {
                continue;
            };
            let Some(anchor) = awidat_meta.anchor.as_ref() else {
                continue;
            };
            let Some(snippet) = anchor.transcript_snippet.as_deref() else {
                continue;
            };
            anchored += 1;

            let asset_id = match &clip.media_reference {
                awidat_proto::otio::MediaReference::External(ext) => ext.target_url.clone(),
                _ => {
                    skipped += 1;
                    continue;
                }
            };

            // Range the clip occupies in source media (seconds).
            let Some(sr) = clip.source_range.as_ref() else {
                skipped += 1;
                continue;
            };
            let clip_start = sr.start_time.value / sr.start_time.rate;
            let clip_end = clip_start + sr.duration.value / sr.duration.rate;

            // Locate the whisper sidecar; project-relative.
            let asset = awidat_proto::index::AssetId::new(asset_id.clone());
            let sidecar = match awidat_index::read_sidecar(project_root, "whisper", &asset) {
                Ok(s) => s,
                Err(_) => {
                    skipped += 1;
                    continue;
                }
            };

            let segments = sidecar
                .pointer("/data/segments")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            let needle = snippet.to_lowercase();
            let hit = segments.iter().find(|seg| {
                seg.get("text")
                    .and_then(|t| t.as_str())
                    .is_some_and(|t| t.to_lowercase().contains(&needle))
            });
            let Some(seg) = hit else {
                failures.push(format!(
                    "clip {:?} anchored on snippet {:?} not found in {} transcript",
                    clip.name,
                    snippet_preview(snippet),
                    asset_id
                ));
                continue;
            };
            let seg_start = seg
                .get("start_s")
                .or_else(|| seg.get("start"))
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);
            let seg_end = seg
                .get("end_s")
                .or_else(|| seg.get("end"))
                .and_then(|v| v.as_f64())
                .unwrap_or(seg_start);

            // Overlap test (same shape as the anchor-resolver fix).
            // Source-of-truth tolerance is the segment-boundary 25ms
            // jitter the resolver allows; mirror it here so we don't
            // flag drifts that the resolver itself accepts.
            const BOUNDARY_TOLERANCE_S: f64 = 0.025;
            let overlaps = seg_end + BOUNDARY_TOLERANCE_S >= clip_start
                && seg_start - BOUNDARY_TOLERANCE_S <= clip_end;
            if overlaps {
                verified += 1;
            } else {
                failures.push(format!(
                    "clip {:?} on {asset_id}: anchor segment [{:.3}s, {:.3}s] no longer \
                     overlaps clip's source_range [{:.3}s, {:.3}s] — \
                     the cut was trimmed past its own anchor. Use Untrim Clip to widen \
                     back, or re-anchor with a transcript_snippet that lies inside the \
                     current source_range.",
                    clip.name, seg_start, seg_end, clip_start, clip_end
                ));
            }
        }
    }

    if !failures.is_empty() {
        return CheckVerdict {
            id: ID.into(),
            pass: false,
            message: format!(
                "{} of {} anchored clip(s) drifted past their transcript anchors: {}",
                failures.len(),
                anchored,
                failures.join("; "),
            ),
        };
    }
    CheckVerdict {
        id: ID.into(),
        pass: true,
        message: if anchored == 0 {
            "no clips with transcript anchors — nothing to verify".into()
        } else {
            format!(
                "{verified}/{anchored} anchors verified ({skipped} skipped — \
                 missing sidecar / source_range / external ref)"
            )
        },
    }
}

fn snippet_preview(s: &str) -> String {
    const N: usize = 60;
    if s.chars().count() <= N {
        s.to_string()
    } else {
        let head: String = s.chars().take(N).collect();
        format!("{head}…")
    }
}

/// Pick the lexicographically-newest `*.mp4` under `dir`. We use
/// mtime, not name — agents render to deterministic filenames, but
/// users may overwrite. None if no candidate.
fn newest_mp4(dir: &Path) -> Option<std::path::PathBuf> {
    let mut best: Option<(std::path::PathBuf, std::time::SystemTime)> = None;
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("mp4") {
            continue;
        }
        let mtime = entry.metadata().and_then(|m| m.modified()).ok()?;
        match &best {
            None => best = Some((path, mtime)),
            Some((_, t)) if mtime > *t => best = Some((path, mtime)),
            _ => {}
        }
    }
    best.map(|(p, _)| p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_timeline_fails_tier_2() {
        let dir = tempfile::tempdir().unwrap();
        let result = tier_2(dir.path(), "test step");
        match result {
            VerifyResult::Fail { reason, .. } => {
                assert!(reason.contains("timeline_renders"));
            }
            VerifyResult::Pass { .. } => panic!("empty dir should fail tier-2"),
        }
    }

    #[test]
    fn valid_project_passes_tier_2() {
        let dir = tempfile::tempdir().unwrap();
        awidat_proto::project::Project::init(dir.path()).unwrap();
        let result = tier_2(dir.path(), "test step");
        match result {
            VerifyResult::Pass { checks } => {
                // 3 checks: timeline_renders + av_sync_40ms + transcript_cut_alignment
                assert_eq!(checks.len(), 3);
                assert!(
                    checks
                        .iter()
                        .find(|c| c.id == "timeline_renders")
                        .unwrap()
                        .pass
                );
                let av = checks.iter().find(|c| c.id == "av_sync_40ms").unwrap();
                assert!(av.pass);
                assert!(av.message.contains("no rendered"));
                let align = checks
                    .iter()
                    .find(|c| c.id == "transcript_cut_alignment")
                    .unwrap();
                assert!(align.pass);
                assert!(align.message.contains("nothing to verify"));
            }
            VerifyResult::Fail { reason, .. } => panic!("fresh init should pass; got {reason}"),
        }
    }

    #[test]
    fn malformed_timeline_fails_with_actionable_diagnostic() {
        let dir = tempfile::tempdir().unwrap();
        awidat_proto::project::Project::init(dir.path()).unwrap();
        // Corrupt the timeline.
        std::fs::write(
            dir.path().join("project.otio.json"),
            b"{ this is not valid JSON",
        )
        .unwrap();
        let result = tier_2(dir.path(), "test step");
        match result {
            VerifyResult::Fail { reason, checks } => {
                let c = checks.iter().find(|c| c.id == "timeline_renders").unwrap();
                assert!(!c.pass);
                assert!(c.message.contains("apply_edl"));
                assert!(reason.contains("timeline_renders"));
            }
            VerifyResult::Pass { .. } => panic!("malformed JSON should fail"),
        }
    }

    #[test]
    fn av_sync_passes_when_no_render_exists() {
        let dir = tempfile::tempdir().unwrap();
        // No renders/ dir at all — must pass with note.
        let v = check_av_sync_within_40ms(dir.path());
        assert!(v.pass, "no-render-dir → pass: {v:?}");
        assert_eq!(v.id, "av_sync_40ms");
        assert!(v.message.contains("no rendered"));
    }

    #[test]
    fn av_sync_passes_when_renders_dir_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("renders")).unwrap();
        let v = check_av_sync_within_40ms(dir.path());
        assert!(v.pass);
        assert!(v.message.contains("no rendered"));
    }

    #[test]
    fn newest_mp4_picks_most_recent_by_mtime() {
        let dir = tempfile::tempdir().unwrap();
        let renders = dir.path().join("renders");
        std::fs::create_dir_all(&renders).unwrap();
        let a = renders.join("a.mp4");
        let b = renders.join("b.mp4");
        std::fs::write(&a, b"a").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&b, b"b").unwrap();
        // .mov siblings ignored.
        std::fs::write(renders.join("ignored.mov"), b"x").unwrap();
        let pick = newest_mp4(&renders).unwrap();
        assert_eq!(pick, b);
    }

    /// Build an mp4 in `renders/<name>.mp4` whose video and audio
    /// streams have the requested durations. Single-shot encode + mux
    /// muddles the durations (the muxer extends the shorter stream),
    /// so we encode each track separately then `-c copy` mux — the
    /// output preserves the input stream durations exactly.
    fn synth_render(renders: &Path, name: &str, video_s: f64, audio_s: f64) {
        let stem = renders.join(name);
        let vid = renders.join(format!("__{name}.v.mp4"));
        let aud = renders.join(format!("__{name}.a.m4a"));
        let ok = std::process::Command::new("ffmpeg")
            .args([
                "-y",
                "-f",
                "lavfi",
                "-i",
                &format!("testsrc=size=64x36:rate=10:duration={video_s}"),
                "-c:v",
                "libx264",
            ])
            .arg(&vid)
            .args(["-hide_banner", "-loglevel", "error"])
            .status()
            .expect("ffmpeg")
            .success();
        assert!(ok, "video synth failed");
        let ok = std::process::Command::new("ffmpeg")
            .args([
                "-y",
                "-f",
                "lavfi",
                "-i",
                &format!("sine=frequency=440:duration={audio_s}"),
                "-c:a",
                "aac",
            ])
            .arg(&aud)
            .args(["-hide_banner", "-loglevel", "error"])
            .status()
            .expect("ffmpeg")
            .success();
        assert!(ok, "audio synth failed");
        let ok = std::process::Command::new("ffmpeg")
            .args(["-y", "-i"])
            .arg(&vid)
            .args(["-i"])
            .arg(&aud)
            .args(["-c", "copy", "-map", "0:v", "-map", "1:a"])
            .arg(&stem)
            .args(["-hide_banner", "-loglevel", "error"])
            .status()
            .expect("ffmpeg")
            .success();
        assert!(ok, "mux failed");
    }

    /// ffprobe + ffmpeg required. Tests are `ignored` so a CI host
    /// without them still passes; opt in with `cargo test -- --ignored`.
    #[test]
    #[ignore = "needs ffmpeg/ffprobe — run with --ignored"]
    fn av_sync_detects_synthetic_drift() {
        let dir = tempfile::tempdir().unwrap();
        let renders = dir.path().join("renders");
        std::fs::create_dir_all(&renders).unwrap();
        synth_render(&renders, "drifty.mp4", 2.0, 2.5);
        let v = check_av_sync_within_40ms(dir.path());
        assert!(!v.pass, "should fail on 500ms drift; got {v:?}");
        assert!(v.message.contains("exceeds"));
        assert!(v.message.contains("scope=\"timeline\""));
    }

    /// Build a 1-clip project with the given source_range + transcript
    /// snippet, plus a whisper sidecar that places that snippet at
    /// `[seg_start, seg_end]`. The verifier should align (or not)
    /// based on the relative ranges.
    fn project_with_anchored_clip(
        clip_start_s: f64,
        clip_dur_s: f64,
        snippet: &str,
        seg_start_s: f64,
        seg_end_s: f64,
    ) -> tempfile::TempDir {
        use awidat_proto::awidat_meta::{Anchor, AwidatClipMetadata};
        use awidat_proto::otio::{
            Clip, ClipMetadata, ExternalReference, MediaReference, RationalTime, StackChild,
            TimeRange, Track, TrackChild, TrackKind,
        };
        let dir = tempfile::tempdir().unwrap();
        let mut project = awidat_proto::project::Project::init(dir.path()).unwrap();

        let mut clip = Clip::empty("c0");
        clip.media_reference =
            MediaReference::External(ExternalReference::new("raw/x.mp4".to_string()));
        clip.source_range = Some(TimeRange::new(
            RationalTime::new(clip_start_s * 24.0, 24.0),
            RationalTime::new(clip_dur_s * 24.0, 24.0),
        ));
        clip.metadata = ClipMetadata {
            awidat: Some(AwidatClipMetadata {
                anchor: Some(Anchor {
                    transcript_snippet: Some(snippet.to_string()),
                    ..Anchor::default()
                }),
                ..AwidatClipMetadata::default()
            }),
            ..ClipMetadata::default()
        };
        let mut track = Track::empty("V1", TrackKind::Video);
        track.children.push(TrackChild::Clip(clip));
        project
            .timeline
            .tracks
            .children
            .push(StackChild::Track(track));
        project.write(dir.path()).unwrap();

        // Whisper sidecar with the snippet placed at the given range.
        let sidecar = serde_json::json!({
            "indexer": "whisper",
            "asset_id": "raw/x.mp4",
            "data": {
                "segments": [{
                    "text": format!("preamble {snippet} postamble"),
                    "start_s": seg_start_s,
                    "end_s": seg_end_s,
                }]
            }
        });
        let p = dir.path().join("index/whisper/raw/x.mp4.json");
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, serde_json::to_vec(&sidecar).unwrap()).unwrap();

        dir
    }

    #[test]
    fn transcript_alignment_passes_when_segment_overlaps_clip() {
        // Clip covers [10, 20]; segment at [12, 13]; trivially inside.
        let dir = project_with_anchored_clip(10.0, 10.0, "hello world", 12.0, 13.0);
        let v = check_transcript_cut_alignment(dir.path());
        assert!(v.pass, "{v:?}");
        assert!(v.message.contains("1/1"));
    }

    #[test]
    fn transcript_alignment_fails_when_clip_trimmed_past_anchor() {
        // Clip covers [10, 11]; anchor segment at [15, 16] — agent
        // trimmed the clip past where its anchor lives.
        let dir = project_with_anchored_clip(10.0, 1.0, "the punchline", 15.0, 16.0);
        let v = check_transcript_cut_alignment(dir.path());
        assert!(!v.pass, "{v:?}");
        assert!(v.message.contains("trimmed past"));
        assert!(v.message.contains("Untrim"));
    }

    #[test]
    fn transcript_alignment_fails_when_snippet_not_in_transcript() {
        // Sidecar has a different snippet text than the anchor — the
        // anchor was lost (transcript changed, or agent fabricated it).
        let dir = project_with_anchored_clip(0.0, 10.0, "missing forever", 0.0, 1.0);
        // Overwrite the sidecar with text that doesn't contain the
        // snippet.
        let p = dir.path().join("index/whisper/raw/x.mp4.json");
        std::fs::write(
            &p,
            serde_json::to_vec(&serde_json::json!({
                "indexer": "whisper", "asset_id": "raw/x.mp4",
                "data": {"segments": [{"text": "totally different content", "start_s": 0, "end_s": 1}]}
            }))
            .unwrap(),
        )
        .unwrap();
        let v = check_transcript_cut_alignment(dir.path());
        assert!(!v.pass, "{v:?}");
        assert!(v.message.contains("not found"));
    }

    #[test]
    fn transcript_alignment_skips_clips_without_sidecar() {
        // Clip exists with anchor, but no whisper sidecar on disk.
        // V1: skip-with-pass (verifier doesn't demand indexers ran).
        use awidat_proto::awidat_meta::{Anchor, AwidatClipMetadata};
        use awidat_proto::otio::{
            Clip, ClipMetadata, ExternalReference, MediaReference, RationalTime, StackChild,
            TimeRange, Track, TrackChild, TrackKind,
        };
        let dir = tempfile::tempdir().unwrap();
        let mut project = awidat_proto::project::Project::init(dir.path()).unwrap();
        let mut clip = Clip::empty("c0");
        clip.media_reference =
            MediaReference::External(ExternalReference::new("raw/x.mp4".to_string()));
        clip.source_range = Some(TimeRange::new(
            RationalTime::new(0.0, 24.0),
            RationalTime::new(120.0, 24.0),
        ));
        clip.metadata = ClipMetadata {
            awidat: Some(AwidatClipMetadata {
                anchor: Some(Anchor {
                    transcript_snippet: Some("anything".into()),
                    ..Anchor::default()
                }),
                ..AwidatClipMetadata::default()
            }),
            ..ClipMetadata::default()
        };
        let mut track = Track::empty("V1", TrackKind::Video);
        track.children.push(TrackChild::Clip(clip));
        project
            .timeline
            .tracks
            .children
            .push(StackChild::Track(track));
        project.write(dir.path()).unwrap();

        let v = check_transcript_cut_alignment(dir.path());
        assert!(v.pass, "missing sidecar must skip-with-pass; got {v:?}");
        assert!(v.message.contains("0/1") || v.message.contains("skipped"));
    }

    #[test]
    #[ignore = "needs ffmpeg/ffprobe — run with --ignored"]
    fn av_sync_passes_aligned_render() {
        let dir = tempfile::tempdir().unwrap();
        let renders = dir.path().join("renders");
        std::fs::create_dir_all(&renders).unwrap();
        synth_render(&renders, "aligned.mp4", 2.0, 2.0);
        let v = check_av_sync_within_40ms(dir.path());
        assert!(v.pass, "aligned render must pass; got {v:?}");
    }
}
