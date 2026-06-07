//! Per-clip dialogue leveling.
//!
//! The master-bus [`crate::master_loudnorm`] two-pass loudnorm normalizes
//! the *program* as one stream — it does not even out clip-to-clip
//! loudness, so a timeline whose dialogue clips range from -20 to -33 LUFS
//! stays just as uneven after the master pass.
//!
//! This module fills that gap. For each dialogue clip it runs a one-shot
//! `loudnorm=...:print_format=json` analysis pass over the clip's source
//! range, parses the measured integrated loudness (reusing
//! [`crate::master_loudnorm::parse_loudnorm_measure_json`]), and writes a
//! per-clip [`AudioFxPlan::loudnorm_i`] target. Because the per-clip
//! loudnorm filter already runs *before* the master pass (see
//! `audio_fx_filter_chain` in [`crate::timeline`]), this evens the clips
//! up front and lets the master pass finish the program at its single
//! target.
//!
//! The whole capability is **opt-in**: nothing here runs unless the caller
//! sets the `MONTAGE_LEVEL_DIALOGUE` env var (or passes the flag through
//! [`dialogue_leveling_enabled`]). Default render behavior is unchanged.
//!
//! Design notes
//! - The pure pieces (argv building, target-fill) are unit-tested without
//!   touching ffmpeg.
//! - Manually-authored `loudnorm_i` targets are never overwritten —
//!   explicit clip settings win over auto-measurement.

use std::path::Path;
use std::process::Command;

use crate::ffmpeg::{FfmpegError, ffmpeg_path};
use crate::master_loudnorm::{MasterLoudnormError, parse_loudnorm_measure_json};
use crate::timeline::{AudioFxPlan, TimelineSegment};

/// Default integrated-loudness target every dialogue clip is leveled to,
/// in LUFS. -16 LUFS is the common streaming/dialogue reference and the
/// value the per-clip path already documents in its examples.
pub const DEFAULT_DIALOGUE_TARGET_LUFS: f64 = -16.0;

/// Default true-peak ceiling paired with the integrated target, in dBTP.
pub const DEFAULT_DIALOGUE_TARGET_TP: f64 = -1.5;

/// Env var that opts a render into automatic per-clip dialogue leveling.
/// Any non-empty value other than `0`/`false` enables it.
pub const LEVEL_DIALOGUE_ENV: &str = "MONTAGE_LEVEL_DIALOGUE";

/// True iff the process environment opts into dialogue leveling. Keeps the
/// default render path untouched: the feature is off unless the operator
/// explicitly turns it on.
pub fn dialogue_leveling_enabled() -> bool {
    matches!(std::env::var(LEVEL_DIALOGUE_ENV), Ok(v) if is_truthy(&v))
}

fn is_truthy(v: &str) -> bool {
    let v = v.trim();
    !v.is_empty() && !v.eq_ignore_ascii_case("0") && !v.eq_ignore_ascii_case("false")
}

/// A single dialogue clip's measured integrated loudness, keyed back to
/// its segment index so the target-fill step can map measurements onto the
/// right clip.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClipLoudness {
    /// Index into the segment slice this measurement came from.
    pub segment_index: usize,
    /// Measured integrated loudness in LUFS.
    pub measured_i: f64,
}

/// Build the ffmpeg argv for a per-clip loudness *measure* pass over a
/// source range. Mirrors the master measure pass but scoped to one clip:
/// `ffmpeg -nostats -ss <start> -t <dur> -i <asset> -map 0:a:0
/// -af loudnorm=I=<tgt>:TP=<tp>:LRA=11:print_format=json -f null -`.
///
/// The integrated-loudness *target* embedded here only steers ffmpeg's
/// gating; the value we actually consume is the `input_i` it prints, which
/// is independent of the target.
pub fn build_clip_measure_argv(
    asset_path: &Path,
    start_s: f64,
    duration_s: f64,
    target_lufs: f64,
    target_tp: f64,
) -> Vec<String> {
    vec![
        "-nostats".into(),
        "-loglevel".into(),
        "info".into(),
        "-ss".into(),
        format!("{start_s}"),
        "-t".into(),
        format!("{duration_s}"),
        "-i".into(),
        asset_path.to_string_lossy().into_owned(),
        "-map".into(),
        "0:a:0?".into(),
        "-af".into(),
        format!(
            "loudnorm=I={}:TP={}:LRA=11:print_format=json",
            fmt(target_lufs),
            fmt(target_tp),
        ),
        "-f".into(),
        "null".into(),
        "-".into(),
    ]
}

/// True iff a segment is a dialogue-leveling candidate: it carries audio
/// that will actually play (not muted) and has a positive duration to
/// measure. Segments whose picture is kept but audio is muted, or whose
/// audio is fully region-removed, are skipped.
pub fn is_dialogue_candidate(seg: &TimelineSegment) -> bool {
    !seg.audio_muted && seg.duration_s > 0.0
}

/// Fill per-clip `loudnorm_i` targets on `segments` from `measurements`,
/// leveling every measured dialogue clip to `target_lufs`.
///
/// Behavior:
/// - Sets `loudnorm_i = target_lufs` and a default `loudnorm_tp` on each
///   measured segment that doesn't already carry a manual `loudnorm_i`.
/// - Never overwrites an explicit, finite `loudnorm_i` — manual clip
///   settings win over auto-measurement.
/// - Allocates an [`AudioFxPlan`] for segments that have none yet so the
///   per-clip loudnorm filter has somewhere to attach.
///
/// `measurements` is informational for callers/tests; the *target* is
/// uniform (`target_lufs`). We still take it so this stays the single
/// place that decides which segments get a target, keeping the measure →
/// fill contract explicit. Measurements whose index is out of range are
/// ignored.
pub fn fill_dialogue_loudnorm_targets(
    segments: &mut [TimelineSegment],
    measurements: &[ClipLoudness],
    target_lufs: f64,
) -> usize {
    let mut filled = 0;
    for m in measurements {
        let Some(seg) = segments.get_mut(m.segment_index) else {
            continue;
        };
        if !is_dialogue_candidate(seg) {
            continue;
        }
        let fx = seg.audio_fx.get_or_insert_with(AudioFxPlan::default);
        // Explicit manual target wins — only auto-fill when unset.
        if fx.loudnorm_i.map(f64::is_finite).unwrap_or(false) {
            continue;
        }
        fx.loudnorm_i = Some(target_lufs);
        if fx.loudnorm_tp.is_none() {
            fx.loudnorm_tp = Some(DEFAULT_DIALOGUE_TARGET_TP);
        }
        filled += 1;
    }
    filled
}

/// Errors from the dialogue-leveling measure step.
#[derive(Debug, thiserror::Error)]
pub enum DialogueLevelingError {
    /// ffmpeg could not be located or spawned.
    #[error(transparent)]
    Ffmpeg(#[from] FfmpegError),
    /// The clip measure ran but its loudnorm JSON could not be parsed.
    #[error(transparent)]
    Measure(#[from] MasterLoudnormError),
    /// ffmpeg exited non-zero on a clip measure pass.
    #[error("clip loudness measure failed (exit {code}): {stderr_tail}")]
    NonZero {
        /// ffmpeg exit code.
        code: i32,
        /// Tail of captured stderr for diagnostics.
        stderr_tail: String,
    },
}

/// Run a single clip's loudness measure pass synchronously and parse the
/// measured integrated loudness out of ffmpeg's stderr.
///
/// Synchronous on purpose: leveling runs as a pre-render planning step,
/// not inside the async render job, so a blocking `std::process` call
/// keeps the module free of a runtime dependency and trivially testable.
pub fn measure_clip_loudness(
    asset_path: &Path,
    start_s: f64,
    duration_s: f64,
    target_lufs: f64,
    target_tp: f64,
) -> Result<f64, DialogueLevelingError> {
    let bin = ffmpeg_path()?;
    let argv = build_clip_measure_argv(asset_path, start_s, duration_s, target_lufs, target_tp);
    let output = Command::new(&bin)
        .args(&argv)
        .output()
        .map_err(|e| FfmpegError::Spawn {
            path: bin.clone(),
            source: e,
        })?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        return Err(DialogueLevelingError::NonZero {
            code: output.status.code().unwrap_or(-1),
            stderr_tail: tail(&stderr, 2048),
        });
    }
    let measured = parse_loudnorm_measure_json(&stderr)?;
    Ok(measured.measured_i)
}

/// Measure every dialogue-candidate segment and fill its per-clip
/// `loudnorm_i` target to `target_lufs`. Returns the number of clips
/// leveled. A clip whose measure pass fails is skipped (and logged) rather
/// than failing the whole render — leveling is a best-effort enhancement.
pub fn level_dialogue_clips(
    segments: &mut [TimelineSegment],
    target_lufs: f64,
    target_tp: f64,
) -> usize {
    let measurements: Vec<ClipLoudness> = segments
        .iter()
        .enumerate()
        .filter(|(_, seg)| is_dialogue_candidate(seg))
        .filter_map(|(i, seg)| {
            match measure_clip_loudness(
                &seg.asset_path,
                seg.start_s,
                seg.duration_s,
                target_lufs,
                target_tp,
            ) {
                Ok(measured_i) => Some(ClipLoudness {
                    segment_index: i,
                    measured_i,
                }),
                Err(e) => {
                    tracing::warn!(
                        segment_index = i,
                        asset = %seg.asset_path.display(),
                        error = %e,
                        "dialogue leveling: clip loudness measure failed; skipping",
                    );
                    None
                }
            }
        })
        .collect();
    fill_dialogue_loudnorm_targets(segments, &measurements, target_lufs)
}

/// Format a number for embedding in an ffmpeg filter expression: trim
/// trailing zeros and normalize `-0` to `0`. Mirrors the master module's
/// formatter so command-strings stay stable across machines.
fn fmt(value: f64) -> String {
    let mut s = format!("{value:.6}");
    while s.contains('.') && s.ends_with('0') {
        s.pop();
    }
    if s.ends_with('.') {
        s.pop();
    }
    if s == "-0" { "0".into() } else { s }
}

/// Tail of a string capped at `max` bytes, on a char boundary.
fn tail(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut start = s.len() - max;
    while start < s.len() && !s.is_char_boundary(start) {
        start += 1;
    }
    s[start..].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn seg(path: &str, start: f64, dur: f64) -> TimelineSegment {
        TimelineSegment {
            asset_path: PathBuf::from(path),
            start_s: start,
            duration_s: dur,
            ..Default::default()
        }
    }

    #[test]
    fn measure_argv_scopes_to_source_range_and_prints_json() {
        let argv = build_clip_measure_argv(Path::new("/tmp/a.mp4"), 12.5, 3.0, -16.0, -1.5);
        let cmd = argv.join(" ");
        assert!(cmd.contains("-ss 12.5"), "{cmd}");
        assert!(cmd.contains("-t 3"), "{cmd}");
        assert!(cmd.contains("-i /tmp/a.mp4"), "{cmd}");
        assert!(
            cmd.contains("loudnorm=I=-16:TP=-1.5:LRA=11:print_format=json"),
            "{cmd}"
        );
        // Null muxer: measure only, no output file.
        assert!(cmd.ends_with("-f null -"), "{cmd}");
    }

    #[test]
    fn parses_measured_loudness_from_stderr() {
        // Reuses the master parser; the per-clip path consumes input_i.
        let stderr = "ffmpeg noise\n{\n \"input_i\": \"-27.40\",\n \"input_tp\": \"-3.1\",\n \"input_lra\": \"5.0\",\n \"input_thresh\": \"-37.0\",\n \"target_offset\": \"0.9\"\n}\n";
        let measured = parse_loudnorm_measure_json(stderr).unwrap();
        assert!((measured.measured_i - (-27.40)).abs() < 1e-9);
    }

    #[test]
    fn fill_targets_levels_measured_dialogue_clips() {
        let mut segs = vec![seg("/tmp/a.mp4", 0.0, 2.0), seg("/tmp/b.mp4", 0.0, 2.0)];
        let measurements = vec![
            ClipLoudness {
                segment_index: 0,
                measured_i: -20.0,
            },
            ClipLoudness {
                segment_index: 1,
                measured_i: -33.0,
            },
        ];
        let filled = fill_dialogue_loudnorm_targets(&mut segs, &measurements, -16.0);
        assert_eq!(filled, 2);
        // Both clips get the SAME integrated target — that's what evens
        // -20 LUFS and -33 LUFS clips out before the master pass.
        assert_eq!(segs[0].audio_fx.as_ref().unwrap().loudnorm_i, Some(-16.0));
        assert_eq!(segs[1].audio_fx.as_ref().unwrap().loudnorm_i, Some(-16.0));
        // A true-peak ceiling is paired in so the per-clip filter is
        // well-formed.
        assert_eq!(
            segs[0].audio_fx.as_ref().unwrap().loudnorm_tp,
            Some(DEFAULT_DIALOGUE_TARGET_TP)
        );
    }

    #[test]
    fn fill_targets_preserves_manual_loudnorm_setting() {
        let mut segs = vec![seg("/tmp/a.mp4", 0.0, 2.0)];
        segs[0].audio_fx = Some(AudioFxPlan {
            loudnorm_i: Some(-14.0),
            loudnorm_tp: Some(-2.0),
            ..Default::default()
        });
        let measurements = vec![ClipLoudness {
            segment_index: 0,
            measured_i: -25.0,
        }];
        let filled = fill_dialogue_loudnorm_targets(&mut segs, &measurements, -16.0);
        assert_eq!(filled, 0, "manual targets must not be overwritten");
        assert_eq!(segs[0].audio_fx.as_ref().unwrap().loudnorm_i, Some(-14.0));
        assert_eq!(segs[0].audio_fx.as_ref().unwrap().loudnorm_tp, Some(-2.0));
    }

    #[test]
    fn fill_targets_skips_muted_clips_and_out_of_range_indices() {
        let mut a = seg("/tmp/a.mp4", 0.0, 2.0);
        a.audio_muted = true;
        let mut segs = vec![a, seg("/tmp/b.mp4", 0.0, 2.0)];
        let measurements = vec![
            ClipLoudness {
                segment_index: 0,
                measured_i: -20.0,
            }, // muted → skipped
            ClipLoudness {
                segment_index: 1,
                measured_i: -30.0,
            }, // leveled
            ClipLoudness {
                segment_index: 9,
                measured_i: -22.0,
            }, // out of range → ignored
        ];
        let filled = fill_dialogue_loudnorm_targets(&mut segs, &measurements, -16.0);
        assert_eq!(filled, 1);
        assert!(
            segs[0].audio_fx.is_none(),
            "muted clip must not get a target"
        );
        assert_eq!(segs[1].audio_fx.as_ref().unwrap().loudnorm_i, Some(-16.0));
    }

    #[test]
    fn enabled_flag_reads_truthy_env_values() {
        assert!(is_truthy("1"));
        assert!(is_truthy("true"));
        assert!(is_truthy("yes"));
        assert!(!is_truthy(""));
        assert!(!is_truthy("0"));
        assert!(!is_truthy("false"));
        assert!(!is_truthy("FALSE"));
    }
}
