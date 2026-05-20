//! Audio sub-pipeline for the raw-stream composer.
//!
//! When the video path runs through [`crate::raw_stream`], audio
//! still wants the existing `acrossfade` treatment for transition
//! overlaps. This module owns that audio-only filter graph: one
//! FFmpeg invocation per render that takes every source clip as an
//! input, atrims each one to its segment range, chains adjacent
//! audio streams through `acrossfade` for transition windows, and
//! emits a single AAC file. A subsequent [`mux_video_and_audio`]
//! step combines that with the GPU-rendered video into the final
//! MP4.
//!
//! This intentionally re-implements the audio half of
//! `plan_with_transitions` rather than refactoring it. The video
//! filter graph lives inside a planner that touches volume effects,
//! speed effects, broadcast overlays, and a half-dozen other
//! features; trying to extract the audio half cleanly without
//! risking regressions in the existing xfade-filter path is a much
//! larger refactor than the audio composer itself. We'll consolidate
//! once both paths have proved themselves end-to-end.

use std::path::Path;
use std::process::Stdio;

use thiserror::Error;
use tokio::io::AsyncReadExt;
use tokio::process::Command;

use crate::ffmpeg::{FfmpegError, ffmpeg_path};
use crate::raw_stream::{ComposeError, RawStreamSegment, RawStreamTransition};

/// Errors from the audio sub-pipeline and mux step.
#[derive(Debug, Error)]
pub enum AudioError {
    /// Wrapper for FFmpeg discovery errors.
    #[error("ffmpeg: {0}")]
    Ffmpeg(#[from] FfmpegError),
    /// Generic I/O from the subprocess pipe.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// The FFmpeg subprocess exited with a non-zero status.
    #[error("ffmpeg {stage} exited with status {status}: {stderr}")]
    FfmpegExit {
        /// Which subprocess failed (e.g. `audio_compose`, `mux`).
        stage: &'static str,
        /// Exit status as reported by the child.
        status: String,
        /// Captured stderr tail.
        stderr: String,
    },
    /// Caller passed an invalid configuration to the audio composer.
    #[error("invalid config: {0}")]
    BadConfig(&'static str),
    /// Caller asked for a chained transition graph but supplied a
    /// transition whose `from_segment_index` points past the end of
    /// the segment list.
    #[error("transition references segment {index} but only {count} segments are present")]
    BadTransition {
        /// The bad index.
        index: usize,
        /// Total segment count.
        count: usize,
    },
}

impl From<AudioError> for ComposeError {
    fn from(err: AudioError) -> ComposeError {
        // ComposeError doesn't currently model audio failures
        // separately; surface them as BadConfig with a stringified
        // message. Callers needing structured matching should use
        // AudioError directly.
        ComposeError::BadConfig(Box::leak(err.to_string().into_boxed_str()))
    }
}

/// Compose the audio half of a raw-stream render to `output_path` as
/// an AAC file. Same segment/transition data as the video composer;
/// transitions become `acrossfade` overlaps.
pub async fn compose_audio(
    segments: &[RawStreamSegment],
    transitions: &[RawStreamTransition],
    output_path: &Path,
) -> Result<(), AudioError> {
    if segments.is_empty() {
        return Err(AudioError::BadConfig("compose_audio needs ≥1 segment"));
    }
    let mut transition_by_from: Vec<Option<&RawStreamTransition>> = vec![None; segments.len()];
    for t in transitions {
        if t.from_segment_index >= segments.len().saturating_sub(1) {
            return Err(AudioError::BadTransition {
                index: t.from_segment_index,
                count: segments.len(),
            });
        }
        if t.duration_s <= 0.0 || !t.duration_s.is_finite() {
            return Err(AudioError::BadConfig(
                "transition duration_s must be positive",
            ));
        }
        transition_by_from[t.from_segment_index] = Some(t);
    }

    let filter = build_audio_filter_complex(segments, &transition_by_from)?;
    let bin = ffmpeg_path()?;
    let mut cmd = Command::new(bin);
    cmd.arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-y");
    for seg in segments {
        cmd.arg("-ss").arg(format!("{}", seg.source_start_s));
        cmd.arg("-t").arg(format!("{}", seg.duration_s));
        cmd.arg("-i").arg(&seg.source_path);
    }
    cmd.arg("-filter_complex")
        .arg(&filter)
        .arg("-map")
        .arg("[outa]")
        .arg("-c:a")
        .arg("aac")
        .arg("-b:a")
        .arg("192k")
        .arg(output_path)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let output = cmd.output().await?;
    if !output.status.success() {
        return Err(AudioError::FfmpegExit {
            stage: "audio_compose",
            status: output.status.to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    Ok(())
}

/// Build the `-filter_complex` string for the audio sub-pipeline.
///
/// Each input `[i:a:0]` is asetpts/atrim'd to `[seg_i]`, then chained
/// through `acrossfade` for transition windows and `concat` for hard
/// cuts. The terminal label is always `[outa]`.
fn build_audio_filter_complex(
    segments: &[RawStreamSegment],
    transition_by_from: &[Option<&RawStreamTransition>],
) -> Result<String, AudioError> {
    let mut filter = String::new();
    // Stage each segment's audio. We use `aresample=async=1` to
    // smooth out timestamp resets and `asetpts=PTS-STARTPTS` to
    // normalize PTS so concat doesn't fight us.
    for i in 0..segments.len() {
        filter.push_str(&format!(
            "[{i}:a:0]aresample=async=1,asetpts=PTS-STARTPTS[seg{i}];"
        ));
    }
    // Walk segments left-to-right, chaining acrossfades for
    // transitions and concat for hard cuts.
    let mut current_label = "[seg0]".to_string();
    let mut concat_inputs: Vec<String> = Vec::new();
    let mut chain_id = 0usize;
    let mut i = 0;
    while i < segments.len() {
        // Greedily extend the current chain by acrossfade for as long
        // as adjacent segments have transitions.
        let mut tail_label = current_label.clone();
        let mut next = i;
        while next + 1 < segments.len() {
            let Some(t) = transition_by_from[next] else {
                break;
            };
            let new_label = format!("[chain{chain_id}]");
            let policy_extra = match transition_audio_policy() {
                AudioPolicy::Crossfade => String::new(),
                AudioPolicy::Cut => ":c1=nofade:c2=nofade".to_string(),
            };
            filter.push_str(&format!(
                "{from}[seg{to_idx}]acrossfade=d={dur}{extra}{out};",
                from = tail_label,
                to_idx = next + 1,
                dur = t.duration_s,
                extra = policy_extra,
                out = new_label,
            ));
            tail_label = new_label;
            chain_id += 1;
            next += 1;
        }
        concat_inputs.push(tail_label);
        i = next + 1;
        if i < segments.len() {
            current_label = format!("[seg{i}]");
        }
    }
    if concat_inputs.len() == 1 {
        // Single chain (one segment, or all segments joined by
        // transitions) — just rename the tail to [outa].
        filter.push_str(&format!(
            "{tail}aresample=async=1[outa]",
            tail = concat_inputs[0],
        ));
    } else {
        for label in &concat_inputs {
            filter.push_str(label);
        }
        filter.push_str(&format!(
            "concat=n={n}:v=0:a=1[outa]",
            n = concat_inputs.len(),
        ));
    }
    Ok(filter)
}

/// Audio behavior at a transition. The raw-stream composer defaults
/// to crossfade for now; future work threads per-transition policy
/// from `TransitionAudioPolicy` through `RawStreamTransition`.
#[derive(Debug, Clone, Copy)]
enum AudioPolicy {
    Crossfade,
    #[allow(dead_code)]
    Cut,
}

fn transition_audio_policy() -> AudioPolicy {
    // Phase 2.3 ships crossfade-only. Per-transition policy lands
    // when RawStreamTransition gains an audio-policy field, alongside
    // the integration in `render::timeline`.
    AudioPolicy::Crossfade
}

/// Final mux step: combine a video-only MP4 and an AAC audio file
/// into one MP4 with `-c copy` (stream-copy, no re-encode).
pub async fn mux_video_and_audio(
    video_path: &Path,
    audio_path: &Path,
    output_path: &Path,
) -> Result<(), AudioError> {
    let bin = ffmpeg_path()?;
    let mut child = Command::new(bin)
        .arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-y")
        .arg("-i")
        .arg(video_path)
        .arg("-i")
        .arg(audio_path)
        .arg("-c")
        .arg("copy")
        .arg("-shortest")
        .arg(output_path)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let status = child.wait().await?;
    if status.success() {
        return Ok(());
    }
    let mut stderr_buf = String::new();
    if let Some(mut e) = child.stderr.take() {
        let _ = e.read_to_string(&mut stderr_buf).await;
    }
    Err(AudioError::FfmpegExit {
        stage: "mux",
        status: status.to_string(),
        stderr: stderr_buf,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn ffmpeg_or_skip() -> Option<PathBuf> {
        ffmpeg_path().ok()
    }

    /// Produce a synthetic audio+video MP4 with a sine wave audio
    /// track. Used by tests that need real audio to inspect.
    async fn synth_av_mp4(
        out: &Path,
        sine_freq_hz: u32,
        color: &str,
        width: u32,
        height: u32,
        duration_s: f64,
        fps: u32,
    ) {
        let bin = ffmpeg_path().expect("ffmpeg required");
        let status = Command::new(bin)
            .arg("-hide_banner")
            .arg("-loglevel")
            .arg("error")
            .arg("-y")
            .arg("-f")
            .arg("lavfi")
            .arg("-i")
            .arg(format!(
                "color={color}:size={width}x{height}:duration={duration_s}:rate={fps}"
            ))
            .arg("-f")
            .arg("lavfi")
            .arg("-i")
            .arg(format!(
                "sine=frequency={sine_freq_hz}:duration={duration_s}:sample_rate=48000"
            ))
            .arg("-pix_fmt")
            .arg("yuv420p")
            .arg("-c:a")
            .arg("aac")
            .arg("-shortest")
            .arg(out)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .expect("ffmpeg spawn");
        assert!(status.success(), "lavfi av synth failed: {status}");
    }

    /// Probe duration of an MP4 via ffprobe. Returns None if ffprobe
    /// is missing or the file is unreadable.
    async fn probe_duration_s(path: &Path) -> Option<f64> {
        let probe = crate::ffmpeg::ffprobe_path().ok()?;
        let out = Command::new(probe)
            .arg("-v")
            .arg("error")
            .arg("-show_entries")
            .arg("format=duration")
            .arg("-of")
            .arg("default=nokey=1:noprint_wrappers=1")
            .arg(path)
            .output()
            .await
            .ok()?;
        String::from_utf8_lossy(&out.stdout).trim().parse().ok()
    }

    #[tokio::test]
    async fn compose_audio_rejects_empty_segments() {
        let err = compose_audio(&[], &[], Path::new("/tmp/x.aac"))
            .await
            .unwrap_err();
        assert!(matches!(err, AudioError::BadConfig(_)));
    }

    /// Single segment, no transitions — exercises the "lone chain"
    /// branch where the filter graph is just a rename to [outa].
    #[tokio::test]
    async fn compose_audio_single_segment_passthrough() {
        if ffmpeg_or_skip().is_none() {
            eprintln!("skipping: ffmpeg not installed");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src.mp4");
        let out = dir.path().join("out.aac");
        synth_av_mp4(&src, 440, "red", 16, 16, 0.5, 30).await;
        let segments = vec![RawStreamSegment {
            source_path: src.clone(),
            source_start_s: 0.0,
            duration_s: 0.5,
        }];
        compose_audio(&segments, &[], &out).await.expect("compose");
        let duration = probe_duration_s(&out).await.unwrap_or(0.0);
        assert!(
            (0.45..=0.6).contains(&duration),
            "expected ~0.5s, got {duration}"
        );
    }

    /// Two segments + one transition — exercises the acrossfade
    /// chain branch.
    #[tokio::test]
    async fn compose_audio_two_segment_acrossfade() {
        if ffmpeg_or_skip().is_none() {
            eprintln!("skipping: ffmpeg not installed");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.mp4");
        let b = dir.path().join("b.mp4");
        let out = dir.path().join("out.aac");
        synth_av_mp4(&a, 440, "red", 16, 16, 1.0, 30).await;
        synth_av_mp4(&b, 880, "blue", 16, 16, 1.0, 30).await;
        let segments = vec![
            RawStreamSegment {
                source_path: a,
                source_start_s: 0.0,
                duration_s: 1.0,
            },
            RawStreamSegment {
                source_path: b,
                source_start_s: 0.0,
                duration_s: 1.0,
            },
        ];
        let transitions = vec![RawStreamTransition {
            from_segment_index: 0,
            duration_s: 0.4,
            shader: awidat_render_gpu::TransitionShader::CrossDissolve,
            param_curves: [
                awidat_proto::transitions::ParamCurve::Const(0.0),
                awidat_proto::transitions::ParamCurve::Const(0.0),
                awidat_proto::transitions::ParamCurve::Const(0.0),
                awidat_proto::transitions::ParamCurve::Const(0.0),
            ],
        }];
        compose_audio(&segments, &transitions, &out)
            .await
            .expect("compose");
        let duration = probe_duration_s(&out).await.unwrap_or(0.0);
        // Total expected timeline = 1.0 + 1.0 - 0.4 = 1.6s, with
        // AAC encoder boundary tolerance.
        assert!(
            (1.5..=1.7).contains(&duration),
            "expected ~1.6s acrossfade'd audio, got {duration}"
        );
    }

    /// Two segments, hard cut (no transition) — exercises the
    /// concat branch.
    #[tokio::test]
    async fn compose_audio_two_segment_hard_cut_concat() {
        if ffmpeg_or_skip().is_none() {
            eprintln!("skipping: ffmpeg not installed");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.mp4");
        let b = dir.path().join("b.mp4");
        let out = dir.path().join("out.aac");
        synth_av_mp4(&a, 440, "red", 16, 16, 0.5, 30).await;
        synth_av_mp4(&b, 880, "blue", 16, 16, 0.5, 30).await;
        let segments = vec![
            RawStreamSegment {
                source_path: a,
                source_start_s: 0.0,
                duration_s: 0.5,
            },
            RawStreamSegment {
                source_path: b,
                source_start_s: 0.0,
                duration_s: 0.5,
            },
        ];
        compose_audio(&segments, &[], &out).await.expect("compose");
        let duration = probe_duration_s(&out).await.unwrap_or(0.0);
        assert!(
            (0.95..=1.10).contains(&duration),
            "expected ~1.0s of concat'd audio, got {duration}"
        );
    }

    /// End-to-end mux: video-only + audio-only → final mp4 with
    /// roughly the right duration.
    #[tokio::test]
    async fn muxes_video_and_audio_into_one_mp4() {
        if ffmpeg_or_skip().is_none() {
            eprintln!("skipping: ffmpeg not installed");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let video = dir.path().join("video.mp4");
        let audio = dir.path().join("audio.aac");
        let out = dir.path().join("final.mp4");
        // Synthesize a video-only and an audio-only file.
        let bin = ffmpeg_path().unwrap();
        let v_status = Command::new(&bin)
            .arg("-hide_banner")
            .arg("-loglevel")
            .arg("error")
            .arg("-y")
            .arg("-f")
            .arg("lavfi")
            .arg("-i")
            .arg("color=red:size=32x32:duration=1:rate=30")
            .arg("-pix_fmt")
            .arg("yuv420p")
            .arg(&video)
            .status()
            .await
            .unwrap();
        assert!(v_status.success());
        let a_status = Command::new(&bin)
            .arg("-hide_banner")
            .arg("-loglevel")
            .arg("error")
            .arg("-y")
            .arg("-f")
            .arg("lavfi")
            .arg("-i")
            .arg("sine=frequency=440:duration=1:sample_rate=48000")
            .arg("-c:a")
            .arg("aac")
            .arg(&audio)
            .status()
            .await
            .unwrap();
        assert!(a_status.success());

        mux_video_and_audio(&video, &audio, &out)
            .await
            .expect("mux");
        let duration = probe_duration_s(&out).await.unwrap_or(0.0);
        assert!(
            (0.95..=1.10).contains(&duration),
            "expected ~1s muxed mp4, got {duration}"
        );
    }
}
