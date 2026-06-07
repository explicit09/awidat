//! Raw-stream composer: drives GPU transitions through the streaming
//! frame primitives end-to-end.
//!
//! This is the "Approach B" core. Two source clips' RGBA streams come
//! in via [`FrameProvider`]s, the GPU shader blends them per frame via
//! [`GpuTransitionRenderer`], and the result streams out to one
//! [`FrameEncoder`] in a single continuous video. No intermediate
//! per-transition MP4s. No FFmpeg `concat` boundaries. One encode
//! pass.
//!
//! Phase 2.1 ships only the single-transition case
//! ([`RawStreamComposer::compose_transition_window`]) — enough to
//! validate the architecture against lavfi sources. Phase 2.2 grows
//! this into [`RawStreamComposer::compose_timeline`] that walks a
//! full segment+transition schedule.

use std::path::{Path, PathBuf};

use montage_proto::transitions::ParamCurve;
use montage_render_gpu::{FrameParams, GpuError, GpuTransitionRenderer, TransitionShader};
use thiserror::Error;

use crate::frame_io::{FrameEncoder, FrameIoError, FrameProvider};

/// One segment in a raw-stream timeline. References a source clip + a
/// source-time range. Internal to the composer; the production
/// integration in `render::timeline` is responsible for converting
/// `TimelineSegment` → `RawStreamSegment`.
#[derive(Debug, Clone)]
pub struct RawStreamSegment {
    /// Path to the source clip.
    pub source_path: PathBuf,
    /// Source-time at which this segment starts.
    pub source_start_s: f64,
    /// Duration in seconds. Must be positive.
    pub duration_s: f64,
}

/// One transition between two adjacent segments. `from_segment_index`
/// names the outgoing segment; `from_segment_index + 1` is the
/// incoming. `duration_s` is the overlap on the timeline (the same
/// convention FFmpeg `xfade` uses).
#[derive(Debug, Clone)]
pub struct RawStreamTransition {
    /// Index of the outgoing segment.
    pub from_segment_index: usize,
    /// Overlap duration on the timeline, in seconds.
    pub duration_s: f64,
    /// Shader to apply during the overlap.
    pub shader: TransitionShader,
    /// Up to four shader-specific parameter curves, evaluated by the
    /// composer at every output frame and packed into the GPU
    /// uniform's `params.xyzw`. Each shader documents which slots it
    /// consumes; unused slots stay at `ParamCurve::Const(0.0)`. This
    /// is the per-frame curve sampling that Phase 4 introduced.
    pub param_curves: [ParamCurve; 4],
}

impl RawStreamTransition {
    /// Convenience: build a transition with constant-zero parameter
    /// curves. Used by tests and by code paths that don't author
    /// per-frame parameters.
    pub fn new(from_segment_index: usize, duration_s: f64, shader: TransitionShader) -> Self {
        Self {
            from_segment_index,
            duration_s,
            shader,
            param_curves: [
                ParamCurve::Const(0.0),
                ParamCurve::Const(0.0),
                ParamCurve::Const(0.0),
                ParamCurve::Const(0.0),
            ],
        }
    }

    /// Convenience: build a transition with one parameter curve in
    /// slot 0 (used by `Blur`, `Wipe`, `Zoom`, …). The remaining
    /// slots are constant zero.
    pub fn with_primary_curve(
        from_segment_index: usize,
        duration_s: f64,
        shader: TransitionShader,
        primary: ParamCurve,
    ) -> Self {
        Self {
            from_segment_index,
            duration_s,
            shader,
            param_curves: [
                primary,
                ParamCurve::Const(0.0),
                ParamCurve::Const(0.0),
                ParamCurve::Const(0.0),
            ],
        }
    }
}

fn evaluate_curves(curves: &[ParamCurve; 4], progress: f64) -> [f32; 4] {
    [
        curves[0].evaluate(progress) as f32,
        curves[1].evaluate(progress) as f32,
        curves[2].evaluate(progress) as f32,
        curves[3].evaluate(progress) as f32,
    ]
}

/// Errors from the raw-stream composer.
#[derive(Debug, Error)]
pub enum ComposeError {
    /// Wrapper for streaming frame I/O failures.
    #[error("frame io: {0}")]
    FrameIo(#[from] FrameIoError),
    /// Wrapper for GPU shader failures.
    #[error("gpu: {0}")]
    Gpu(#[from] GpuError),
    /// A provider stopped yielding frames before we expected, leaving
    /// the encoder underrun.
    #[error(
        "provider underrun: {stage} stopped at frame {at_frame} of {expected_frames}; \
         source clip likely shorter than the transition window"
    )]
    Underrun {
        /// Which provider ran out (`from` / `to`).
        stage: &'static str,
        /// Output frame index where the underrun was detected.
        at_frame: u32,
        /// Total frames the composer was scheduled to produce.
        expected_frames: u32,
    },
    /// Caller passed an invalid composer configuration.
    #[error("invalid config: {0}")]
    BadConfig(&'static str),
}

/// Streaming composer that turns frame streams + GPU shader output
/// into a single MP4.
///
/// One composer instance can compose multiple outputs at the same
/// dimensions/framerate. The wgpu device is lazily created on first
/// compose call and reused across calls (cheap subsequent renders).
#[derive(Debug, Clone, Copy)]
pub struct RawStreamComposer {
    width: u32,
    height: u32,
    fps: u32,
}

impl RawStreamComposer {
    /// New composer for output frames of `width x height` at `fps`.
    pub fn new(width: u32, height: u32, fps: u32) -> Result<Self, ComposeError> {
        if width == 0 || height == 0 {
            return Err(ComposeError::BadConfig("width and height must be non-zero"));
        }
        if fps == 0 {
            return Err(ComposeError::BadConfig("fps must be non-zero"));
        }
        Ok(Self { width, height, fps })
    }

    /// Output frame width in pixels.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Output frame height in pixels.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Output framerate.
    pub fn fps(&self) -> u32 {
        self.fps
    }

    /// Compose one transition window into a video-only MP4.
    ///
    /// Reads `duration_s` seconds of frames from each source, runs the
    /// shader on each frame pair (progress 1/N → N/N), and writes the
    /// blended stream to `output_path`. Audio is intentionally
    /// skipped — the timeline composer that calls this is responsible
    /// for muxing audio via a parallel sub-pipeline.
    ///
    /// On error, all subprocesses are cleaned up (via `kill_on_drop`
    /// in the underlying providers/encoder).
    #[allow(clippy::too_many_arguments)]
    pub async fn compose_transition_window(
        &self,
        from_path: &Path,
        from_source_start_s: f64,
        to_path: &Path,
        to_source_start_s: f64,
        duration_s: f64,
        shader: TransitionShader,
        output_path: &Path,
    ) -> Result<u32, ComposeError> {
        self.compose_transition_window_with_curves(
            from_path,
            from_source_start_s,
            to_path,
            to_source_start_s,
            duration_s,
            shader,
            &[
                ParamCurve::Const(0.0),
                ParamCurve::Const(0.0),
                ParamCurve::Const(0.0),
                ParamCurve::Const(0.0),
            ],
            output_path,
        )
        .await
    }

    /// Same as [`compose_transition_window`] but with per-frame
    /// parameter curves. The shader receives the curves' evaluated
    /// values via `extra_params.xyzw` on every frame.
    #[allow(clippy::too_many_arguments)]
    pub async fn compose_transition_window_with_curves(
        &self,
        from_path: &Path,
        from_source_start_s: f64,
        to_path: &Path,
        to_source_start_s: f64,
        duration_s: f64,
        shader: TransitionShader,
        param_curves: &[ParamCurve; 4],
        output_path: &Path,
    ) -> Result<u32, ComposeError> {
        if !duration_s.is_finite() || duration_s <= 0.0 {
            return Err(ComposeError::BadConfig("duration_s must be positive"));
        }
        let frame_count = ((duration_s * self.fps as f64).round() as u32).max(1);

        let mut from_provider = FrameProvider::open(
            from_path,
            from_source_start_s,
            duration_s,
            self.width,
            self.height,
            self.fps,
        )
        .await?;
        let mut to_provider = FrameProvider::open(
            to_path,
            to_source_start_s,
            duration_s,
            self.width,
            self.height,
            self.fps,
        )
        .await?;
        let mut encoder =
            FrameEncoder::open(output_path, self.width, self.height, self.fps).await?;
        let renderer = GpuTransitionRenderer::new(shader)?;

        let mut frames_written = 0u32;
        for i in 0..frame_count {
            // Per-frame progress runs (1/N) → (N/N) inclusive. Frame
            // 0 already has a small contribution from the incoming
            // clip, frame N-1 is fully the incoming clip. This matches
            // FFmpeg xfade's convention so swap-in is safe later.
            let progress = (i as f32 + 1.0) / (frame_count as f32);

            // The provider returns a borrow tied to its internal
            // buffer; copy out before the next call invalidates it.
            let from_frame = match from_provider.next_frame().await? {
                Some(bytes) => bytes.to_vec(),
                None => {
                    return Err(ComposeError::Underrun {
                        stage: "from",
                        at_frame: i,
                        expected_frames: frame_count,
                    });
                }
            };
            let to_frame = match to_provider.next_frame().await? {
                Some(bytes) => bytes.to_vec(),
                None => {
                    return Err(ComposeError::Underrun {
                        stage: "to",
                        at_frame: i,
                        expected_frames: frame_count,
                    });
                }
            };

            let extra_params = evaluate_curves(
                &[
                    param_curves[0].clone(),
                    param_curves[1].clone(),
                    param_curves[2].clone(),
                    param_curves[3].clone(),
                ],
                progress as f64,
            );
            let blended = renderer.render_frame(
                &from_frame,
                &to_frame,
                FrameParams {
                    width: self.width,
                    height: self.height,
                    progress,
                    extra_params,
                },
            )?;
            encoder.push_frame(&blended).await?;
            frames_written += 1;
        }

        from_provider.close().await?;
        to_provider.close().await?;
        encoder.finish().await?;
        Ok(frames_written)
    }

    /// Compose a full timeline into a video-only MP4.
    ///
    /// `segments` is the ordered list of clips on the timeline.
    /// `transitions` is the list of overlaps between adjacent segments;
    /// any pair without a matching transition is a hard cut. Returns
    /// the total number of frames written.
    ///
    /// Provider lifecycle: only the current and (during a transition)
    /// the next segment's provider are open at any time, so a 50-clip
    /// timeline never has 50 FFmpeg subprocesses alive at once. Each
    /// provider is opened lazily, drained, then closed.
    pub async fn compose_timeline(
        &self,
        segments: &[RawStreamSegment],
        transitions: &[RawStreamTransition],
        output_path: &Path,
    ) -> Result<u32, ComposeError> {
        if segments.is_empty() {
            return Err(ComposeError::BadConfig("compose_timeline needs ≥1 segment"));
        }
        for (idx, seg) in segments.iter().enumerate() {
            if !seg.duration_s.is_finite() || seg.duration_s <= 0.0 {
                return Err(ComposeError::BadConfig(
                    "every segment must have a positive finite duration",
                ));
            }
            if !seg.source_start_s.is_finite() || seg.source_start_s < 0.0 {
                let _ = idx; // keep the message simple
                return Err(ComposeError::BadConfig(
                    "every segment must have a finite non-negative source_start_s",
                ));
            }
        }
        // Index transitions by their outgoing segment for O(1) lookup
        // and validate that each transition fits inside its outgoing
        // segment plus the incoming segment.
        let mut transition_by_from: Vec<Option<&RawStreamTransition>> = vec![None; segments.len()];
        for t in transitions {
            if !t.duration_s.is_finite() || t.duration_s <= 0.0 {
                return Err(ComposeError::BadConfig(
                    "transition duration_s must be positive",
                ));
            }
            if t.from_segment_index >= segments.len().saturating_sub(1) {
                return Err(ComposeError::BadConfig(
                    "transition from_segment_index must reference a non-terminal segment",
                ));
            }
            if transition_by_from[t.from_segment_index].is_some() {
                return Err(ComposeError::BadConfig(
                    "duplicate transition for a single segment boundary",
                ));
            }
            let from_seg = &segments[t.from_segment_index];
            let to_seg = &segments[t.from_segment_index + 1];
            if t.duration_s > from_seg.duration_s || t.duration_s > to_seg.duration_s {
                return Err(ComposeError::BadConfig(
                    "transition longer than one of the adjacent segments",
                ));
            }
            transition_by_from[t.from_segment_index] = Some(t);
        }

        let mut encoder =
            FrameEncoder::open(output_path, self.width, self.height, self.fps).await?;
        // Lazily-initialized renderer: only spin up wgpu if a GPU
        // transition is actually present in this timeline.
        let mut renderer: Option<GpuTransitionRenderer> = None;
        let mut current_shader: Option<TransitionShader> = None;
        let mut frames_written = 0u32;

        let mut current_provider: Option<FrameProvider> = None;
        for (i, seg) in segments.iter().enumerate() {
            // Open the current segment's provider if not already open
            // (it stays open from the previous iteration when a
            // transition just consumed its incoming side).
            if current_provider.is_none() {
                current_provider = Some(open_provider_for(seg, self).await?);
            }
            let outgoing_transition = transition_by_from[i];
            // A segment's incoming overlap is consumed during the
            // *previous* iteration's transition; its outgoing overlap
            // is consumed during the *current* iteration's transition.
            // The pure window between them is what we emit now.
            let incoming_duration_s = if i > 0 {
                transition_by_from[i - 1]
                    .map(|t| t.duration_s)
                    .unwrap_or(0.0)
            } else {
                0.0
            };
            let outgoing_duration_s = outgoing_transition.map(|t| t.duration_s).unwrap_or(0.0);
            let pure_duration =
                (seg.duration_s - incoming_duration_s - outgoing_duration_s).max(0.0);
            let pure_frames = (pure_duration * self.fps as f64).round() as u32;

            // Emit pure frames from the current segment.
            let provider = current_provider
                .as_mut()
                .ok_or(ComposeError::BadConfig("missing current provider"))?;
            for j in 0..pure_frames {
                let frame_owned = match provider.next_frame().await? {
                    Some(bytes) => bytes.to_vec(),
                    None => {
                        return Err(ComposeError::Underrun {
                            stage: "segment_pure",
                            at_frame: frames_written + j,
                            expected_frames: pure_frames,
                        });
                    }
                };
                encoder.push_frame(&frame_owned).await?;
                frames_written += 1;
            }

            // If there's an outgoing transition, blend with the next
            // segment for its overlap window. Otherwise the current
            // segment is finished — close its provider so the next
            // loop iteration opens a fresh one for the next segment.
            if outgoing_transition.is_none()
                && let Some(p) = current_provider.take()
            {
                p.close().await?;
            }
            if let Some(t) = outgoing_transition {
                let next_seg = &segments[i + 1];
                let mut next_provider = open_provider_for(next_seg, self).await?;

                // Lazily build / swap the renderer when the requested
                // shader changes.
                let needs_new_renderer = !matches!(current_shader, Some(s) if s == t.shader);
                if needs_new_renderer {
                    renderer = Some(GpuTransitionRenderer::new(t.shader)?);
                    current_shader = Some(t.shader);
                }
                let renderer_ref = renderer
                    .as_ref()
                    .ok_or(ComposeError::BadConfig("renderer not initialized"))?;

                let overlap_frames = ((t.duration_s * self.fps as f64).round() as u32).max(1);
                let from_provider = current_provider
                    .as_mut()
                    .ok_or(ComposeError::BadConfig("missing current provider"))?;
                for k in 0..overlap_frames {
                    let progress = (k as f32 + 1.0) / (overlap_frames as f32);
                    let from_frame = match from_provider.next_frame().await? {
                        Some(bytes) => bytes.to_vec(),
                        None => {
                            return Err(ComposeError::Underrun {
                                stage: "transition_from",
                                at_frame: frames_written,
                                expected_frames: overlap_frames,
                            });
                        }
                    };
                    let to_frame = match next_provider.next_frame().await? {
                        Some(bytes) => bytes.to_vec(),
                        None => {
                            return Err(ComposeError::Underrun {
                                stage: "transition_to",
                                at_frame: frames_written,
                                expected_frames: overlap_frames,
                            });
                        }
                    };
                    let extra_params = evaluate_curves(&t.param_curves, progress as f64);
                    let blended = renderer_ref.render_frame(
                        &from_frame,
                        &to_frame,
                        FrameParams {
                            width: self.width,
                            height: self.height,
                            progress,
                            extra_params,
                        },
                    )?;
                    encoder.push_frame(&blended).await?;
                    frames_written += 1;
                }

                // The outgoing provider is exhausted; close it.
                // Promote next_provider so the next loop iteration
                // continues with it.
                let exhausted = current_provider.take().ok_or(ComposeError::BadConfig(
                    "current provider missing at handoff",
                ))?;
                exhausted.close().await?;
                current_provider = Some(next_provider);
            }
        }

        if let Some(p) = current_provider.take() {
            p.close().await?;
        }
        encoder.finish().await?;
        Ok(frames_written)
    }
}

async fn open_provider_for(
    segment: &RawStreamSegment,
    composer: &RawStreamComposer,
) -> Result<FrameProvider, ComposeError> {
    FrameProvider::open(
        &segment.source_path,
        segment.source_start_s,
        segment.duration_s,
        composer.width,
        composer.height,
        composer.fps,
    )
    .await
    .map_err(ComposeError::from)
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

    async fn synth_color_mp4(
        out: &Path,
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
            .arg("-pix_fmt")
            .arg("yuv420p")
            .arg(out)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .expect("ffmpeg spawn");
        assert!(status.success(), "lavfi synth failed: {status}");
    }

    /// Probe an MP4's frame count via ffprobe. Returns None if ffprobe
    /// isn't installed or the file is unreadable.
    async fn probe_frame_count(path: &Path) -> Option<u64> {
        let probe = crate::ffmpeg::ffprobe_path().ok()?;
        let output = Command::new(probe)
            .arg("-v")
            .arg("error")
            .arg("-select_streams")
            .arg("v:0")
            .arg("-count_frames")
            .arg("-show_entries")
            .arg("stream=nb_read_frames")
            .arg("-of")
            .arg("default=nokey=1:noprint_wrappers=1")
            .arg(path)
            .output()
            .await
            .ok()?;
        let text = String::from_utf8_lossy(&output.stdout);
        text.trim().parse().ok()
    }

    #[tokio::test]
    async fn rejects_zero_dimensions() {
        let err = RawStreamComposer::new(0, 16, 30).unwrap_err();
        assert!(matches!(err, ComposeError::BadConfig(_)));
    }

    #[tokio::test]
    async fn rejects_zero_fps() {
        let err = RawStreamComposer::new(16, 16, 0).unwrap_err();
        assert!(matches!(err, ComposeError::BadConfig(_)));
    }

    #[tokio::test]
    async fn rejects_non_positive_duration() {
        if ffmpeg_or_skip().is_none() {
            eprintln!("skipping: ffmpeg not installed");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let red = dir.path().join("red.mp4");
        synth_color_mp4(&red, "red", 16, 16, 0.5, 30).await;
        let composer = RawStreamComposer::new(16, 16, 30).unwrap();
        let err = composer
            .compose_transition_window(
                &red,
                0.0,
                &red,
                0.0,
                0.0,
                TransitionShader::CrossDissolve,
                &dir.path().join("out.mp4"),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, ComposeError::BadConfig(_)));
    }

    /// The end-to-end test for Phase 2.1: two synthetic clips of
    /// different colors, GPU cross-dissolve between them, output MP4
    /// has the expected duration and decodes back to a midpoint blend.
    #[tokio::test]
    async fn composes_red_to_blue_dissolve_end_to_end() {
        if ffmpeg_or_skip().is_none() {
            eprintln!("skipping: ffmpeg not installed");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let red = dir.path().join("red.mp4");
        let blue = dir.path().join("blue.mp4");
        let out = dir.path().join("dissolve.mp4");

        synth_color_mp4(&red, "red", 32, 32, 1.0, 30).await;
        synth_color_mp4(&blue, "blue", 32, 32, 1.0, 30).await;

        let composer = RawStreamComposer::new(32, 32, 30).unwrap();
        let result = composer
            .compose_transition_window(
                &red,
                0.0,
                &blue,
                0.0,
                0.5,
                TransitionShader::CrossDissolve,
                &out,
            )
            .await;

        match result {
            Ok(frames) => {
                assert_eq!(frames, 15, "expected 15 frames at 0.5s * 30fps");
                let probed = probe_frame_count(&out).await;
                if let Some(count) = probed {
                    // The encoder may pad by one frame depending on
                    // its internal timestamping; accept ±1 of nominal.
                    assert!(
                        (14..=16).contains(&count),
                        "ffprobe reports {count} frames, expected ~15"
                    );
                }
                let metadata = std::fs::metadata(&out).unwrap();
                assert!(metadata.len() > 0, "output mp4 should be non-empty");
            }
            Err(ComposeError::Gpu(GpuError::NoAdapter(_)))
            | Err(ComposeError::Gpu(GpuError::NoDevice(_))) => {
                eprintln!("skipping: no GPU adapter available");
            }
            Err(other) => panic!("unexpected compose error: {other}"),
        }
    }

    #[tokio::test]
    async fn compose_timeline_rejects_empty_segments() {
        let composer = RawStreamComposer::new(16, 16, 30).unwrap();
        let err = composer
            .compose_timeline(&[], &[], Path::new("/tmp/empty.mp4"))
            .await
            .unwrap_err();
        assert!(matches!(err, ComposeError::BadConfig(_)));
    }

    #[tokio::test]
    async fn compose_timeline_rejects_transition_longer_than_segment() {
        let composer = RawStreamComposer::new(16, 16, 30).unwrap();
        let segments = vec![
            RawStreamSegment {
                source_path: PathBuf::from("/tmp/a.mp4"),
                source_start_s: 0.0,
                duration_s: 0.2,
            },
            RawStreamSegment {
                source_path: PathBuf::from("/tmp/b.mp4"),
                source_start_s: 0.0,
                duration_s: 0.2,
            },
        ];
        let transitions = vec![RawStreamTransition {
            from_segment_index: 0,
            duration_s: 0.5,
            shader: TransitionShader::CrossDissolve,
            param_curves: [
                montage_proto::transitions::ParamCurve::Const(0.0),
                montage_proto::transitions::ParamCurve::Const(0.0),
                montage_proto::transitions::ParamCurve::Const(0.0),
                montage_proto::transitions::ParamCurve::Const(0.0),
            ],
        }];
        let err = composer
            .compose_timeline(&segments, &transitions, Path::new("/tmp/bad.mp4"))
            .await
            .unwrap_err();
        assert!(matches!(err, ComposeError::BadConfig(_)));
    }

    /// Three-segment timeline with two cross-dissolves. Exercises:
    /// pure-segment frames, transition overlap with provider handoff,
    /// and lazy provider lifecycle (only 2 providers alive at once).
    #[tokio::test]
    async fn composes_three_segment_timeline_with_two_transitions() {
        if ffmpeg_or_skip().is_none() {
            eprintln!("skipping: ffmpeg not installed");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let red = dir.path().join("red.mp4");
        let green = dir.path().join("green.mp4");
        let blue = dir.path().join("blue.mp4");
        let out = dir.path().join("three.mp4");

        synth_color_mp4(&red, "red", 32, 32, 1.0, 30).await;
        synth_color_mp4(&green, "green", 32, 32, 1.0, 30).await;
        synth_color_mp4(&blue, "blue", 32, 32, 1.0, 30).await;

        let segments = vec![
            RawStreamSegment {
                source_path: red.clone(),
                source_start_s: 0.0,
                duration_s: 1.0,
            },
            RawStreamSegment {
                source_path: green.clone(),
                source_start_s: 0.0,
                duration_s: 1.0,
            },
            RawStreamSegment {
                source_path: blue.clone(),
                source_start_s: 0.0,
                duration_s: 1.0,
            },
        ];
        let transitions = vec![
            RawStreamTransition {
                from_segment_index: 0,
                duration_s: 0.4,
                shader: TransitionShader::CrossDissolve,
                param_curves: [
                    montage_proto::transitions::ParamCurve::Const(0.0),
                    montage_proto::transitions::ParamCurve::Const(0.0),
                    montage_proto::transitions::ParamCurve::Const(0.0),
                    montage_proto::transitions::ParamCurve::Const(0.0),
                ],
            },
            RawStreamTransition {
                from_segment_index: 1,
                duration_s: 0.4,
                shader: TransitionShader::CrossDissolve,
                param_curves: [
                    montage_proto::transitions::ParamCurve::Const(0.0),
                    montage_proto::transitions::ParamCurve::Const(0.0),
                    montage_proto::transitions::ParamCurve::Const(0.0),
                    montage_proto::transitions::ParamCurve::Const(0.0),
                ],
            },
        ];
        let composer = RawStreamComposer::new(32, 32, 30).unwrap();
        let result = composer
            .compose_timeline(&segments, &transitions, &out)
            .await;
        match result {
            Ok(frames) => {
                // Total timeline: 1.0 + 1.0 + 1.0 - 0.4 - 0.4 = 2.2s.
                // At 30 fps that's 66 frames; the round-trip can shift
                // by ±1 depending on intermediate rounding.
                assert!(
                    (65..=67).contains(&frames),
                    "expected ~66 frames, got {frames}"
                );
                let metadata = std::fs::metadata(&out).unwrap();
                assert!(metadata.len() > 0, "output mp4 should be non-empty");
            }
            Err(ComposeError::Gpu(GpuError::NoAdapter(_)))
            | Err(ComposeError::Gpu(GpuError::NoDevice(_))) => {
                eprintln!("skipping: no GPU adapter available");
            }
            Err(other) => panic!("unexpected compose error: {other}"),
        }
    }

    /// Two segments with no transition between them (hard cut). The
    /// composer should consume A fully, then B fully, without
    /// spinning up wgpu at all.
    #[tokio::test]
    async fn composes_hard_cut_without_gpu_initialization() {
        if ffmpeg_or_skip().is_none() {
            eprintln!("skipping: ffmpeg not installed");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let red = dir.path().join("red.mp4");
        let blue = dir.path().join("blue.mp4");
        let out = dir.path().join("cut.mp4");
        synth_color_mp4(&red, "red", 16, 16, 0.5, 30).await;
        synth_color_mp4(&blue, "blue", 16, 16, 0.5, 30).await;

        let segments = vec![
            RawStreamSegment {
                source_path: red.clone(),
                source_start_s: 0.0,
                duration_s: 0.5,
            },
            RawStreamSegment {
                source_path: blue.clone(),
                source_start_s: 0.0,
                duration_s: 0.5,
            },
        ];
        let composer = RawStreamComposer::new(16, 16, 30).unwrap();
        let frames = composer
            .compose_timeline(&segments, &[], &out)
            .await
            .expect("hard cut must succeed without GPU");
        assert!(
            (28..=32).contains(&frames),
            "expected ~30 frames, got {frames}"
        );
    }

    /// Phase 4 goal contract test: a keyframed Blur with amount curve
    /// `[(0.0, 0.0), (0.5, 0.8), (1.0, 0.0)]` produces output frames
    /// whose middle is measurably more blurred than the endpoints.
    /// "More blurred" = lower standard deviation of red-channel pixel
    /// values (a blurred test pattern has less contrast than a sharp
    /// one).
    #[tokio::test]
    async fn keyframed_blur_amount_animates_over_transition_window() {
        if ffmpeg_or_skip().is_none() {
            eprintln!("skipping: ffmpeg not installed");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("testsrc.mp4");
        let out = dir.path().join("blurred.mp4");

        // Generate a test pattern with sharp edges.
        let bin = crate::ffmpeg::ffmpeg_path().expect("ffmpeg");
        let status = Command::new(bin)
            .arg("-hide_banner")
            .arg("-loglevel")
            .arg("error")
            .arg("-y")
            .arg("-f")
            .arg("lavfi")
            .arg("-i")
            .arg("testsrc=size=64x64:duration=1:rate=30")
            .arg("-pix_fmt")
            .arg("yuv420p")
            .arg(&a)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .unwrap();
        assert!(status.success(), "testsrc synth must succeed");

        let blur_curve = ParamCurve::Keyframes(vec![
            montage_proto::transitions::Keyframe {
                t: 0.0,
                v: 0.0,
                easing: montage_proto::transitions::TransitionEasing::Linear,
            },
            montage_proto::transitions::Keyframe {
                t: 0.5,
                v: 0.95,
                easing: montage_proto::transitions::TransitionEasing::Linear,
            },
            montage_proto::transitions::Keyframe {
                t: 1.0,
                v: 0.0,
                easing: montage_proto::transitions::TransitionEasing::Linear,
            },
        ]);

        let composer = RawStreamComposer::new(64, 64, 30).unwrap();
        let result = composer
            .compose_transition_window_with_curves(
                &a,
                0.0,
                &a,
                0.0,
                1.0,
                TransitionShader::Blur,
                &[
                    blur_curve,
                    ParamCurve::Const(0.0),
                    ParamCurve::Const(0.0),
                    ParamCurve::Const(0.0),
                ],
                &out,
            )
            .await;

        match result {
            Ok(_frames) => {}
            Err(ComposeError::Gpu(GpuError::NoAdapter(_)))
            | Err(ComposeError::Gpu(GpuError::NoDevice(_))) => {
                eprintln!("skipping: no GPU adapter");
                return;
            }
            Err(other) => panic!("compose failed: {other}"),
        }

        // Decode the output back to raw RGBA and measure red-channel
        // standard deviation per frame.
        let mut provider = crate::frame_io::FrameProvider::open(&out, 0.0, 1.0, 64, 64, 30)
            .await
            .unwrap();
        let mut frames: Vec<Vec<u8>> = Vec::new();
        while let Some(frame) = provider.next_frame().await.unwrap() {
            frames.push(frame.to_vec());
        }
        provider.close().await.unwrap();

        assert!(
            frames.len() >= 10,
            "expected at least 10 output frames, got {}",
            frames.len()
        );

        // Sample 3 frames: one near the start (low blur), one at the
        // middle (peak blur), one near the end (low blur). At 30 fps
        // over 1 second this is roughly indices 1, 15, 28.
        let early = std_dev_red(&frames[1]);
        let middle_idx = frames.len() / 2;
        let middle = std_dev_red(&frames[middle_idx]);
        let late = std_dev_red(&frames[frames.len().saturating_sub(2)]);

        eprintln!("blur std-dev: early={early:.2} middle={middle:.2} late={late:.2}");
        // Sharper (lower-blur) frames have higher std dev because the
        // test pattern's hard color boundaries survive. The peak-blur
        // frame at index ~15 should have measurably lower std dev
        // than both shoulders. We use 5% as the threshold to avoid
        // flaking on tiny round-trip drift.
        let threshold_ratio = 0.95;
        assert!(
            middle < early * threshold_ratio,
            "middle blur ({middle:.2}) should be <{threshold_ratio:.2} of early ({early:.2})"
        );
        assert!(
            middle < late * threshold_ratio,
            "middle blur ({middle:.2}) should be <{threshold_ratio:.2} of late ({late:.2})"
        );
    }

    fn std_dev_red(frame: &[u8]) -> f64 {
        let n = (frame.len() / 4) as f64;
        if n == 0.0 {
            return 0.0;
        }
        let mut sum = 0.0;
        let mut sum_sq = 0.0;
        for chunk in frame.chunks(4) {
            let r = chunk[0] as f64;
            sum += r;
            sum_sq += r * r;
        }
        let mean = sum / n;
        let var = (sum_sq / n) - mean * mean;
        var.max(0.0).sqrt()
    }

    /// Sanity: an underrun (source clip shorter than the requested
    /// window) surfaces as the typed error, not as a corrupt MP4.
    #[tokio::test]
    async fn reports_underrun_when_source_too_short() {
        if ffmpeg_or_skip().is_none() {
            eprintln!("skipping: ffmpeg not installed");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let short = dir.path().join("short.mp4");
        let other = dir.path().join("other.mp4");
        let out = dir.path().join("out.mp4");
        synth_color_mp4(&short, "red", 16, 16, 0.1, 30).await;
        synth_color_mp4(&other, "blue", 16, 16, 1.0, 30).await;

        let composer = RawStreamComposer::new(16, 16, 30).unwrap();
        let result = composer
            .compose_transition_window(
                &short,
                0.0,
                &other,
                0.0,
                1.0, // Wants 30 frames but `short` only has ~3.
                TransitionShader::CrossDissolve,
                &out,
            )
            .await;

        match result {
            Err(ComposeError::Underrun { stage: "from", .. }) => {}
            Err(ComposeError::Gpu(GpuError::NoAdapter(_)))
            | Err(ComposeError::Gpu(GpuError::NoDevice(_))) => {
                eprintln!("skipping: no GPU adapter available");
            }
            other => panic!("expected underrun on `from`, got {other:?}"),
        }
    }
}
