//! ffmpeg wrapper for awidat.
//!
//! Public surface:
//! - [`ffmpeg::ffmpeg_path`] / [`ffmpeg::ffprobe_path`] — locate the
//!   binaries (env-var override → `which` lookup → fail).
//! - [`ffmpeg::extract_frame`] — single-frame extraction at time `t_s`.
//!   Used by `view_frame`.
//! - [`ffmpeg::probe_duration_s`] — ffprobe wrapper returning the
//!   asset's duration in seconds.
//! - [`ffmpeg::transcode_proxy`] — produce a 720p H.264 all-keyframe
//!   proxy of an asset, with progress callbacks. Used by the desktop
//!   import flow to make scrubbable previews.
//! - [`ffmpeg::generate_thumbnails`] — extract one filmstrip JPEG per
//!   second of source-time into a per-asset thumbnails dir. Used by
//!   the timeline canvas to draw filmstrip strips inside clips.
//! - [`ffmpeg::generate_waveform`] — pull mono PCM at 8 kHz and bucket
//!   into peak amplitudes. Used by the timeline canvas to draw audio
//!   waveforms on audio clips.
//! - [`ffmpeg::generate_silences`] — detect silence ranges via
//!   ffmpeg's silencedetect filter. Used by the `find_dead_air`
//!   editorial tool to surface silence-trim Notes.
//! - [`ffmpeg::generate_motion_signal`] — sample per-second motion
//!   magnitude via ffmpeg's scene-change score. Used by the
//!   continuity engine (Phase 2) to detect mid-motion cuts.
//! - [`job::JobManager`] — long-running ffmpeg job orchestrator. Used by
//!   `start_render` / `poll_render`.
//! - [`progress::ProgressSnapshot`] — parsed view of ffmpeg's stderr
//!   progress lines (`frame=`, `time=`, `speed=`).

#![cfg_attr(
    test,
    allow(
        clippy::expect_used,
        clippy::field_reassign_with_default,
        clippy::unwrap_used
    )
)]

pub mod animation;
pub(crate) mod ass;
pub mod ffmpeg;
pub mod frame_io;
pub mod job;
pub mod master_loudnorm;
pub mod output_safety;
pub mod professional;
pub mod progress;
pub mod raw_stream;
pub mod raw_stream_audio;
pub mod raw_stream_render;
pub mod reframe_smoothing;
pub mod timeline;

pub use ass::{build_ass_document_for_test, default_caption_font_name, resolve_caption_font_name};
pub use ffmpeg::{
    BlackFrameRange, FfmpegError, MediaProbe, MotionSignal, SilenceRange, TranscodeProgress,
    TranscodeProgressCallback, extract_frame, extract_frame_complex, extract_frame_filtered,
    ffmpeg_path, ffprobe_path, generate_black_frames, generate_motion_signal, generate_silences,
    generate_thumbnails, generate_waveform, probe_duration_s, probe_media, transcode_proxy,
};
pub use frame_io::{FrameEncoder, FrameIoError, FrameProvider};
pub use job::{
    JobError, JobId, JobManager, JobState, JobStatus, RenderJobSpec, RenderPlanLimitation,
};
pub use master_loudnorm::{
    JobManagerRunner, MasterLoudnormError, MasterLoudnormPlan, MeasuredLoudnorm, RenderJobRunner,
    RenderJobRunnerError, RenderRunnerOutput, build_master_loudnorm_apply_spec,
    build_master_loudnorm_measure_spec, parse_loudnorm_measure_json, read_master_loudnorm_plan,
    run_master_loudnorm_render,
};
pub use output_safety::{OutputPathPolicy, OutputPathSafetyError, validate_render_output_path};
pub use progress::ProgressSnapshot;
pub use raw_stream::{ComposeError, RawStreamComposer, RawStreamSegment, RawStreamTransition};
pub use raw_stream_audio::{AudioError, compose_audio, mux_video_and_audio};
pub use raw_stream_render::{
    RawStreamRenderError, build_timeline_raw_stream_render, should_route_through_raw_stream,
};
pub use timeline::{
    AnnotationKind, AnnotationPlan, AudioAutomationPlan, AudioClipPlan, AudioFxPlan,
    AudioTrackItemPlan, AudioTrackPlan, BroadcastOverlayPlan, ClipGradeChain, ClipGradePreview,
    ColorPipelinePlan, DuckingPlan, EqBandPlan, FilterPlan, FilterPlanner, LoudnessTargetPlan,
    RenderTimelineError, TimelineSegment, TitleAnimation, TitlePlan, TitlePosition, TitleWeight,
    TransitionPlan, VideoOverlayMode, VideoOverlayPlan, build_clip_grade_chain,
    build_clip_preview_filtergraph, build_timeline_argv, build_timeline_argv_full,
    build_timeline_argv_full_with_annotations, build_timeline_argv_with_audio_tracks,
    build_timeline_argv_with_audio_tracks_and_annotations, build_timeline_argv_with_transitions,
    build_timeline_render_spec, build_timeline_section_render_spec, collect_timeline_full_plan,
    collect_timeline_plan, collect_timeline_segments,
};

#[cfg(test)]
mod tests {
    use super::*;

    /// Smoke: the binary discovery returns *something* on macOS dev boxes
    /// (this assumes ffmpeg is installed). Skipped if neither ffmpeg nor
    /// the env override is present, so CI without ffmpeg still passes.
    #[test]
    fn ffmpeg_lookup_returns_or_skips() {
        match ffmpeg_path() {
            Ok(p) => assert!(p.exists(), "ffmpeg path must exist: {}", p.display()),
            Err(_) => {
                // No ffmpeg on this machine — fine for CI without media tooling.
            }
        }
    }
}
