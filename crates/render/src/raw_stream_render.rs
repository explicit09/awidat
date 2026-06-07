//! High-level orchestrator for the raw-stream render path.
//!
//! Wires the existing `TimelineSegment` / `TransitionPlan` types into
//! the raw-stream composer (`raw_stream`) and the audio sub-pipeline
//! (`raw_stream_audio`). The flow per render is:
//!
//! 1. Convert `TimelineSegment[]` and `TransitionPlan[]` into the
//!    composer's internal model.
//! 2. Render video-only MP4 via the streaming GPU composer.
//! 3. Render audio-only AAC via the audio sub-pipeline (acrossfade
//!    chain + concat).
//! 4. Mux the two with `-c copy` into the final output.
//! 5. Clean up the temporary files.
//!
//! The detection function [`should_route_through_raw_stream`] is what
//! callers use to decide between this orchestrator and the existing
//! `build_timeline_argv_with_transitions` filter-graph path. The
//! contract: only timelines where every transition has
//! `TransitionPlan.gpu_shader: Some(_)` go through raw-stream GPU.
//! Mixed GPU/FFmpeg timelines fall back to the legacy path with
//! explicit backend evidence.

use std::path::Path;

use serde::Serialize;
use thiserror::Error;

use crate::RenderBackendKind;
use crate::raw_stream::{ComposeError, RawStreamComposer, RawStreamSegment, RawStreamTransition};
use crate::raw_stream_audio::{AudioError, compose_audio, mux_video_and_audio};
use crate::timeline::{TimelineSegment, TransitionPlan};

/// Errors from the raw-stream render orchestrator.
#[derive(Debug, Error)]
pub enum RawStreamRenderError {
    /// Wrapper for the streaming video composer.
    #[error("video compose: {0}")]
    VideoCompose(#[from] ComposeError),
    /// Wrapper for the audio sub-pipeline.
    #[error("audio: {0}")]
    Audio(#[from] AudioError),
    /// Wrapper for I/O setting up temp files.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// At least one transition has `gpu_shader: None`. The raw-stream
    /// path is only safe when every transition is GPU-routed (mixing
    /// xfade and GPU transitions in one render is not yet supported).
    #[error(
        "transition {index} between segments {from} and {to} has no gpu_shader; \
         mixed xfade/GPU transitions are not yet supported"
    )]
    UnsupportedMixedTransition {
        /// Transition list index.
        index: usize,
        /// Outgoing segment index.
        from: usize,
        /// Incoming segment index.
        to: usize,
    },
    /// A non-adjacent transition slipped through into the orchestrator.
    #[error(
        "non-adjacent transition at index {index}: from={from}, to={to} (expected to = from + 1)"
    )]
    NonAdjacentTransition {
        /// Transition list index.
        index: usize,
        /// Outgoing segment index.
        from: usize,
        /// Incoming segment index.
        to: usize,
    },
}

/// True when every transition is GPU-routed.
///
/// Callers use this to dispatch between the raw-stream render path
/// and the legacy filter-graph path.
pub fn should_route_through_raw_stream(transitions: &[TransitionPlan]) -> bool {
    !transitions.is_empty() && transitions.iter().all(|t| t.gpu_shader.is_some())
}

/// Select the timeline render backend from content-level transition evidence.
pub fn select_timeline_render_backend(transitions: &[TransitionPlan]) -> RenderBackendKind {
    select_timeline_render_backend_evidence(transitions, 0).backend
}

/// Content evidence used to choose a timeline render backend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TimelineBackendEvidence {
    /// Selected backend.
    pub backend: RenderBackendKind,
    /// Stable short reason for the selection.
    pub reason: String,
    /// Number of transitions routed through GPU shaders.
    pub gpu_transition_count: usize,
    /// Number of transitions routed through FFmpeg/filtergraph logic.
    pub ffmpeg_transition_count: usize,
    /// Number of caption overlays eligible for libass burn-in.
    pub libass_caption_count: usize,
}

impl TimelineBackendEvidence {
    /// Flatten evidence into manifest metadata fields.
    pub fn metadata_pairs(&self) -> std::collections::BTreeMap<String, String> {
        let backend = serde_json::to_value(&self.backend)
            .ok()
            .and_then(|value| value.as_str().map(str::to_string))
            .unwrap_or_else(|| format!("{:?}", self.backend));
        std::collections::BTreeMap::from([
            ("timeline_backend".into(), backend),
            ("timeline_backend_reason".into(), self.reason.clone()),
            (
                "gpu_transition_count".into(),
                self.gpu_transition_count.to_string(),
            ),
            (
                "ffmpeg_transition_count".into(),
                self.ffmpeg_transition_count.to_string(),
            ),
            (
                "libass_caption_count".into(),
                self.libass_caption_count.to_string(),
            ),
        ])
    }
}

/// Select the timeline render backend and preserve content evidence.
pub fn select_timeline_render_backend_evidence(
    transitions: &[TransitionPlan],
    libass_caption_count: usize,
) -> TimelineBackendEvidence {
    let gpu_transition_count = transitions
        .iter()
        .filter(|transition| transition.gpu_shader.is_some())
        .count();
    let ffmpeg_transition_count = transitions.len().saturating_sub(gpu_transition_count);
    if gpu_transition_count > 0 && ffmpeg_transition_count == 0 {
        TimelineBackendEvidence {
            backend: RenderBackendKind::TimelineRawStreamGpu,
            reason: "gpu_transition_present".into(),
            gpu_transition_count,
            ffmpeg_transition_count,
            libass_caption_count,
        }
    } else if gpu_transition_count > 0 {
        TimelineBackendEvidence {
            backend: RenderBackendKind::TimelineFfmpegReencode,
            reason: "mixed_gpu_ffmpeg_transitions".into(),
            gpu_transition_count,
            ffmpeg_transition_count,
            libass_caption_count,
        }
    } else {
        let reason = if libass_caption_count > 0 {
            "ffmpeg_with_libass_captions"
        } else {
            "ffmpeg_filtergraph"
        };
        TimelineBackendEvidence {
            backend: RenderBackendKind::TimelineFfmpegReencode,
            reason: reason.into(),
            gpu_transition_count,
            ffmpeg_transition_count,
            libass_caption_count,
        }
    }
}

/// Render a full timeline through the raw-stream path.
///
/// `width`, `height`, and `fps` define the output canvas. `output_path`
/// is the final MP4. Temporary intermediates live next to it and are
/// cleaned up on success or failure.
///
/// Requires *every* transition to have `gpu_shader: Some(_)`. Mixing
/// xfade and GPU transitions in one render is a future extension; for
/// now the dispatcher must route 100% GPU projects through this
/// function and 100% xfade projects through the legacy path.
pub async fn build_timeline_raw_stream_render(
    segments: &[TimelineSegment],
    transitions: &[TransitionPlan],
    width: u32,
    height: u32,
    fps: u32,
    output_path: &Path,
) -> Result<(), RawStreamRenderError> {
    let (raw_segments, raw_transitions) = convert_inputs(segments, transitions)?;

    let parent = output_path
        .parent()
        .ok_or_else(|| std::io::Error::other("output_path has no parent directory"))?;
    let stem = output_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("montage_raw_stream");
    let video_temp = parent.join(format!(".{stem}.video.mp4"));
    let audio_temp = parent.join(format!(".{stem}.audio.aac"));

    let composer = RawStreamComposer::new(width, height, fps)?;

    let render_result = run_render(
        &composer,
        &raw_segments,
        &raw_transitions,
        &video_temp,
        &audio_temp,
        output_path,
    )
    .await;

    // Best-effort cleanup of intermediates regardless of outcome.
    let _ = tokio::fs::remove_file(&video_temp).await;
    let _ = tokio::fs::remove_file(&audio_temp).await;

    render_result
}

async fn run_render(
    composer: &RawStreamComposer,
    raw_segments: &[RawStreamSegment],
    raw_transitions: &[RawStreamTransition],
    video_temp: &Path,
    audio_temp: &Path,
    output_path: &Path,
) -> Result<(), RawStreamRenderError> {
    composer
        .compose_timeline(raw_segments, raw_transitions, video_temp)
        .await?;
    compose_audio(raw_segments, raw_transitions, audio_temp).await?;
    mux_video_and_audio(video_temp, audio_temp, output_path).await?;
    Ok(())
}

/// Convert the timeline crate's types into the raw-stream composer's
/// internal model. Validates adjacency and GPU-shader presence.
fn convert_inputs(
    segments: &[TimelineSegment],
    transitions: &[TransitionPlan],
) -> Result<(Vec<RawStreamSegment>, Vec<RawStreamTransition>), RawStreamRenderError> {
    let raw_segments: Vec<RawStreamSegment> = segments
        .iter()
        .map(|s| RawStreamSegment {
            source_path: s.asset_path.clone(),
            source_start_s: s.start_s,
            duration_s: s.duration_s,
        })
        .collect();

    let mut raw_transitions: Vec<RawStreamTransition> = Vec::with_capacity(transitions.len());
    for (idx, t) in transitions.iter().enumerate() {
        if t.to_segment_index != t.from_segment_index + 1 {
            return Err(RawStreamRenderError::NonAdjacentTransition {
                index: idx,
                from: t.from_segment_index,
                to: t.to_segment_index,
            });
        }
        let Some(shader) = t.gpu_shader else {
            return Err(RawStreamRenderError::UnsupportedMixedTransition {
                index: idx,
                from: t.from_segment_index,
                to: t.to_segment_index,
            });
        };
        let param_curves = extract_param_curves(t);
        raw_transitions.push(RawStreamTransition {
            from_segment_index: t.from_segment_index,
            duration_s: t.duration_s,
            shader,
            param_curves,
        });
    }

    Ok((raw_segments, raw_transitions))
}

/// Inspect a [`TransitionPlan`]'s composition for the highest-priority
/// primitive whose parameters drive a GPU shader, and return the
/// shader's input curves packed into `[ParamCurve; 4]`. Slots not used
/// by the chosen shader stay at `Const(0.0)`.
fn extract_param_curves(plan: &TransitionPlan) -> [montage_proto::transitions::ParamCurve; 4] {
    use montage_proto::transitions::{ParamCurve, TransitionPrimitiveOp};
    let default = || {
        [
            ParamCurve::Const(0.0),
            ParamCurve::Const(0.0),
            ParamCurve::Const(0.0),
            ParamCurve::Const(0.0),
        ]
    };
    let Some(composition) = plan.composition.as_ref() else {
        return default();
    };
    // For now we only sample primitives whose curve maps to a known
    // shader slot. Take the first matching primitive in the
    // composition; multi-primitive compositions that fight for the
    // same slot are out of scope for Phase 4.
    for primitive in &composition.primitives {
        match &primitive.op {
            TransitionPrimitiveOp::Blur { amount, .. } => {
                return [
                    amount.clone(),
                    ParamCurve::Const(0.0),
                    ParamCurve::Const(0.0),
                    ParamCurve::Const(0.0),
                ];
            }
            TransitionPrimitiveOp::Wipe { softness, .. } => {
                return [
                    softness.clone(),
                    ParamCurve::Const(0.0),
                    ParamCurve::Const(0.0),
                    ParamCurve::Const(0.0),
                ];
            }
            TransitionPrimitiveOp::Zoom { scale } => {
                return [
                    scale.clone(),
                    ParamCurve::Const(0.0),
                    ParamCurve::Const(0.0),
                    ParamCurve::Const(0.0),
                ];
            }
            _ => continue,
        }
    }
    default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ffmpeg::ffmpeg_path;
    use std::path::PathBuf;
    use std::process::Stdio;
    use tokio::process::Command;

    fn ffmpeg_or_skip() -> Option<PathBuf> {
        ffmpeg_path().ok()
    }

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
        assert!(status.success(), "av synth failed: {status}");
    }

    fn ts(asset_path: PathBuf, start_s: f64, duration_s: f64) -> TimelineSegment {
        TimelineSegment {
            clip_name: "test".to_string(),
            asset_path,
            start_s,
            duration_s,
            ..Default::default()
        }
    }

    fn tp_gpu(
        from: usize,
        kind: &str,
        duration_s: f64,
        shader: montage_render_gpu::TransitionShader,
    ) -> TransitionPlan {
        TransitionPlan {
            from_segment_index: from,
            to_segment_index: from + 1,
            kind: kind.to_string(),
            in_offset_s: duration_s / 2.0,
            out_offset_s: duration_s / 2.0,
            duration_s,
            composition: None,
            gpu_shader: Some(shader),
        }
    }

    fn tp_no_gpu(from: usize, kind: &str, duration_s: f64) -> TransitionPlan {
        TransitionPlan {
            from_segment_index: from,
            to_segment_index: from + 1,
            kind: kind.to_string(),
            in_offset_s: duration_s / 2.0,
            out_offset_s: duration_s / 2.0,
            duration_s,
            composition: None,
            gpu_shader: None,
        }
    }

    #[test]
    fn dispatch_detects_gpu_transitions() {
        let with_gpu = vec![tp_gpu(
            0,
            "montage.cross_dissolve",
            0.3,
            montage_render_gpu::TransitionShader::CrossDissolve,
        )];
        assert!(should_route_through_raw_stream(&with_gpu));

        let without_gpu = vec![tp_no_gpu(0, "montage.cross_dissolve", 0.3)];
        assert!(!should_route_through_raw_stream(&without_gpu));

        let mixed = vec![
            tp_gpu(
                0,
                "montage.cross_dissolve",
                0.3,
                montage_render_gpu::TransitionShader::CrossDissolve,
            ),
            tp_no_gpu(1, "montage.cross_dissolve", 0.3),
        ];
        assert!(!should_route_through_raw_stream(&mixed));

        let empty: Vec<TransitionPlan> = Vec::new();
        assert!(!should_route_through_raw_stream(&empty));
    }

    #[test]
    fn selected_timeline_backend_reflects_gpu_transition_content() {
        let with_gpu = vec![tp_gpu(
            0,
            "montage.match_dissolve",
            0.3,
            montage_render_gpu::TransitionShader::CrossDissolve,
        )];
        let without_gpu = vec![tp_no_gpu(0, "montage.cross_dissolve", 0.3)];

        assert_eq!(
            select_timeline_render_backend(&with_gpu),
            crate::RenderBackendKind::TimelineRawStreamGpu
        );
        assert_eq!(
            select_timeline_render_backend(&without_gpu),
            crate::RenderBackendKind::TimelineFfmpegReencode
        );
    }

    #[test]
    fn selected_timeline_backend_falls_back_for_mixed_gpu_and_ffmpeg_transitions() {
        let mixed = vec![
            tp_gpu(
                0,
                "montage.match_dissolve",
                0.3,
                montage_render_gpu::TransitionShader::CrossDissolve,
            ),
            tp_no_gpu(1, "montage.cross_dissolve", 0.3),
        ];

        let evidence = select_timeline_render_backend_evidence(&mixed, 0);

        assert_eq!(
            evidence.backend,
            crate::RenderBackendKind::TimelineFfmpegReencode
        );
        assert_eq!(evidence.reason, "mixed_gpu_ffmpeg_transitions");
        assert_eq!(evidence.gpu_transition_count, 1);
        assert_eq!(evidence.ffmpeg_transition_count, 1);
    }

    #[test]
    fn timeline_backend_evidence_records_libass_caption_reason() {
        let without_gpu = vec![tp_no_gpu(0, "montage.cross_dissolve", 0.3)];

        let evidence = select_timeline_render_backend_evidence(&without_gpu, 2);

        assert_eq!(
            evidence.backend,
            crate::RenderBackendKind::TimelineFfmpegReencode
        );
        assert_eq!(evidence.reason, "ffmpeg_with_libass_captions");
        assert_eq!(evidence.ffmpeg_transition_count, 1);
        assert_eq!(evidence.gpu_transition_count, 0);
        assert_eq!(evidence.libass_caption_count, 2);
        let metadata = evidence.metadata_pairs();
        assert_eq!(
            metadata.get("timeline_backend").map(String::as_str),
            Some("timeline_ffmpeg_reencode")
        );
        assert_eq!(
            metadata.get("libass_caption_count").map(String::as_str),
            Some("2")
        );
    }

    #[tokio::test]
    async fn rejects_mixed_xfade_and_gpu_transitions() {
        let segments = vec![
            ts(PathBuf::from("/tmp/a.mp4"), 0.0, 1.0),
            ts(PathBuf::from("/tmp/b.mp4"), 0.0, 1.0),
            ts(PathBuf::from("/tmp/c.mp4"), 0.0, 1.0),
        ];
        let transitions = vec![
            tp_gpu(
                0,
                "montage.cross_dissolve",
                0.3,
                montage_render_gpu::TransitionShader::CrossDissolve,
            ),
            tp_no_gpu(1, "montage.cross_dissolve", 0.3),
        ];
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("out.mp4");
        let err = build_timeline_raw_stream_render(&segments, &transitions, 16, 16, 30, &out)
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            RawStreamRenderError::UnsupportedMixedTransition { .. }
        ));
    }

    /// The load-bearing end-to-end test: two synthetic AV clips,
    /// one GPU cross-dissolve, output MP4 has both video and audio
    /// at the expected duration.
    #[tokio::test]
    async fn end_to_end_renders_video_and_audio_through_raw_stream() {
        if ffmpeg_or_skip().is_none() {
            eprintln!("skipping: ffmpeg not installed");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.mp4");
        let b = dir.path().join("b.mp4");
        let out = dir.path().join("final.mp4");

        synth_av_mp4(&a, 440, "red", 32, 32, 1.0, 30).await;
        synth_av_mp4(&b, 880, "blue", 32, 32, 1.0, 30).await;

        let segments = vec![ts(a, 0.0, 1.0), ts(b, 0.0, 1.0)];
        let transitions = vec![tp_gpu(
            0,
            "montage.cross_dissolve",
            0.4,
            montage_render_gpu::TransitionShader::CrossDissolve,
        )];

        let result =
            build_timeline_raw_stream_render(&segments, &transitions, 32, 32, 30, &out).await;
        match result {
            Ok(()) => {
                let probe = crate::ffmpeg::ffprobe_path().expect("ffprobe");
                let video_streams = Command::new(&probe)
                    .arg("-v")
                    .arg("error")
                    .arg("-select_streams")
                    .arg("v")
                    .arg("-show_entries")
                    .arg("stream=codec_type")
                    .arg("-of")
                    .arg("default=nokey=1:noprint_wrappers=1")
                    .arg(&out)
                    .output()
                    .await
                    .unwrap();
                assert!(
                    String::from_utf8_lossy(&video_streams.stdout).contains("video"),
                    "muxed output must have a video stream"
                );
                let audio_streams = Command::new(&probe)
                    .arg("-v")
                    .arg("error")
                    .arg("-select_streams")
                    .arg("a")
                    .arg("-show_entries")
                    .arg("stream=codec_type")
                    .arg("-of")
                    .arg("default=nokey=1:noprint_wrappers=1")
                    .arg(&out)
                    .output()
                    .await
                    .unwrap();
                assert!(
                    String::from_utf8_lossy(&audio_streams.stdout).contains("audio"),
                    "muxed output must have an audio stream"
                );

                let duration_out = Command::new(&probe)
                    .arg("-v")
                    .arg("error")
                    .arg("-show_entries")
                    .arg("format=duration")
                    .arg("-of")
                    .arg("default=nokey=1:noprint_wrappers=1")
                    .arg(&out)
                    .output()
                    .await
                    .unwrap();
                let duration: f64 = String::from_utf8_lossy(&duration_out.stdout)
                    .trim()
                    .parse()
                    .unwrap_or(0.0);
                // Total expected: 1.0 + 1.0 - 0.4 = 1.6s, ±0.1s for
                // codec boundary effects.
                assert!(
                    (1.5..=1.75).contains(&duration),
                    "expected ~1.6s muxed output, got {duration}"
                );
            }
            Err(RawStreamRenderError::VideoCompose(ComposeError::Gpu(
                montage_render_gpu::GpuError::NoAdapter(_),
            )))
            | Err(RawStreamRenderError::VideoCompose(ComposeError::Gpu(
                montage_render_gpu::GpuError::NoDevice(_),
            ))) => {
                eprintln!("skipping: no GPU adapter in this environment");
            }
            Err(other) => panic!("unexpected error: {other}"),
        }
    }
}
