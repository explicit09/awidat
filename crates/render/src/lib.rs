//! ffmpeg wrapper for awidat.
//!
//! Public surface:
//! - [`ffmpeg::ffmpeg_path`] / [`ffmpeg::ffprobe_path`] — locate the
//!   binaries (env-var override → `which` lookup → fail).
//! - [`ffmpeg::extract_frame`] — single-frame extraction at time `t_s`.
//!   Used by `view_frame`.
//! - [`job::JobManager`] — long-running ffmpeg job orchestrator. Used by
//!   `start_render` / `poll_render`.
//! - [`progress::ProgressSnapshot`] — parsed view of ffmpeg's stderr
//!   progress lines (`frame=`, `time=`, `speed=`).

pub mod ffmpeg;
pub mod job;
pub mod progress;

pub use ffmpeg::{FfmpegError, ffmpeg_path, ffprobe_path};
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
