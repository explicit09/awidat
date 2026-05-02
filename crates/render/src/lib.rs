//! ffmpeg wrapper for awidat.
//!
//! Public surface for the editorial-tools batch:
//! - [`ffmpeg::ffmpeg_path`] / [`ffmpeg::ffprobe_path`] — locate the
//!   binaries (env-var override → `which` lookup → fail).
//! - [`ffmpeg::extract_frame`] — single-frame extraction at time `t_s`,
//!   returns PNG bytes. Used by `view_frame`.
//!
//! The job manager + progress parsing for `start_render` / `poll_render`
//! lands in the next batch.

pub mod ffmpeg;

pub use ffmpeg::{FfmpegError, ffmpeg_path, ffprobe_path};

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
