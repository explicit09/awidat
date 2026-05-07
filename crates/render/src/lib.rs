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
//! - [`job::JobManager`] — long-running ffmpeg job orchestrator. Used by
//!   `start_render` / `poll_render`.
//! - [`progress::ProgressSnapshot`] — parsed view of ffmpeg's stderr
//!   progress lines (`frame=`, `time=`, `speed=`).

pub mod ffmpeg;
pub mod job;
pub mod progress;
pub mod timeline;

pub use ffmpeg::{
    FfmpegError, TranscodeProgress, TranscodeProgressCallback, extract_frame, ffmpeg_path,
    ffprobe_path, generate_thumbnails, generate_waveform, probe_duration_s, transcode_proxy,
};
pub use timeline::{
    FilterPlan, FilterPlanner, RenderTimelineError, TimelineSegment, TitleAnimation, TitlePlan,
    TitlePosition, TitleWeight, TransitionPlan, build_timeline_argv,
    build_timeline_argv_full, build_timeline_argv_with_transitions,
    build_timeline_render_spec, collect_timeline_full_plan, collect_timeline_plan,
    collect_timeline_segments,
};
pub use job::{JobError, JobId, JobManager, JobState, JobStatus, RenderJobSpec};
pub use progress::ProgressSnapshot;

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
