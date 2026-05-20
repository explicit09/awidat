//! Streaming RGBA frame I/O over FFmpeg subprocesses.
//!
//! These are the foundation primitives for the raw-stream composer
//! that replaces our filter-graph-based transition path for GPU
//! transitions (Approach B in the Phase 2 design notes — single
//! continuous raw-RGBA stream piped into one FFmpeg encode, instead of
//! pre-rendering transition MP4s and concatenating them).
//!
//! * [`FrameProvider`] spawns an FFmpeg decoder for one source clip
//!   covering a time range, scales/converts to RGBA8 at the target
//!   resolution and framerate, and lets callers pull one frame at a
//!   time. Backpressure comes for free from the subprocess pipe.
//!
//! * [`FrameEncoder`] spawns an FFmpeg encoder reading raw RGBA from
//!   stdin and writing an MP4 to disk. Callers push frames in order;
//!   `finish` closes the pipe and waits for the encoder to exit.
//!
//! Neither side touches audio. Audio crossfading runs through a
//! separate FFmpeg invocation that we mux with the video output at
//! the end — see the upcoming timeline composer for the orchestration.

use std::path::Path;
use std::process::Stdio;

use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

use crate::ffmpeg::{FfmpegError, ffmpeg_path};

/// Errors from the streaming frame I/O primitives.
#[derive(Debug, Error)]
pub enum FrameIoError {
    /// Wrapper for FFmpeg discovery failures.
    #[error("ffmpeg: {0}")]
    Ffmpeg(#[from] FfmpegError),
    /// Generic I/O on the pipe between us and FFmpeg.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// FFmpeg exited with a non-zero status.
    #[error("ffmpeg {stage} exited with status {status}: {stderr}")]
    FfmpegExit {
        /// Which subprocess failed (e.g. `decode`, `encode`).
        stage: &'static str,
        /// Exit status as reported by the child.
        status: String,
        /// Captured stderr tail.
        stderr: String,
    },
    /// Caller passed RGBA bytes whose length does not match the
    /// frame size the encoder was opened with.
    #[error(
        "frame size mismatch: expected {expected} bytes ({width}x{height} RGBA8), got {actual}"
    )]
    BadFrameSize {
        /// Expected `width * height * 4`.
        expected: usize,
        /// Actual length of the slice the caller passed.
        actual: usize,
        /// Frame width in pixels.
        width: u32,
        /// Frame height in pixels.
        height: u32,
    },
    /// The decoder stopped emitting bytes mid-frame.
    #[error("frame decoder stopped mid-frame after {bytes_read} of {frame_bytes} bytes")]
    PartialFrame {
        /// Bytes received for the incomplete frame.
        bytes_read: usize,
        /// Expected bytes per frame (`width * height * 4`).
        frame_bytes: usize,
    },
}

/// Streaming RGBA frame source.
///
/// Constructed by spawning FFmpeg with `-ss <start> -t <duration>
/// -vf "scale=W:H,fps=FPS" -pix_fmt rgba -f rawvideo -`. Each call to
/// [`next_frame`](FrameProvider::next_frame) reads exactly
/// `width*height*4` bytes from the subprocess pipe; returns `None`
/// when the source ends cleanly on a frame boundary.
pub struct FrameProvider {
    child: Child,
    stdout: ChildStdout,
    width: u32,
    height: u32,
    frame_bytes: usize,
    buf: Vec<u8>,
    eof_seen: bool,
}

impl FrameProvider {
    /// Open a streaming decoder over `source` covering
    /// `[source_start_s, source_start_s + duration_s)`. Frames are
    /// rescaled to `width x height` and resampled to `fps`.
    pub async fn open(
        source: &Path,
        source_start_s: f64,
        duration_s: f64,
        width: u32,
        height: u32,
        fps: u32,
    ) -> Result<Self, FrameIoError> {
        if width == 0 || height == 0 || fps == 0 {
            return Err(FrameIoError::BadFrameSize {
                expected: 0,
                actual: 0,
                width,
                height,
            });
        }
        let bin = ffmpeg_path()?;
        let mut child = Command::new(bin)
            .arg("-hide_banner")
            .arg("-loglevel")
            .arg("error")
            .arg("-ss")
            .arg(format!("{source_start_s}"))
            .arg("-i")
            .arg(source)
            .arg("-t")
            .arg(format!("{duration_s}"))
            .arg("-vf")
            .arg(format!("scale={width}:{height},fps={fps}"))
            .arg("-pix_fmt")
            .arg("rgba")
            .arg("-f")
            .arg("rawvideo")
            .arg("-")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(FrameIoError::Io)?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| FrameIoError::Io(std::io::Error::other("ffmpeg stdout missing")))?;
        let frame_bytes = (width as usize) * (height as usize) * 4;
        Ok(Self {
            child,
            stdout,
            width,
            height,
            frame_bytes,
            buf: vec![0u8; frame_bytes],
            eof_seen: false,
        })
    }

    /// Width of each yielded frame in pixels.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Height of each yielded frame in pixels.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Bytes per yielded frame (`width * height * 4`).
    pub fn frame_bytes(&self) -> usize {
        self.frame_bytes
    }

    /// Read the next frame.
    ///
    /// Returns `Ok(Some(&[u8]))` for a complete frame, `Ok(None)` when
    /// the source ends on a frame boundary, and `Err(PartialFrame)`
    /// when the source stops mid-frame.
    pub async fn next_frame(&mut self) -> Result<Option<&[u8]>, FrameIoError> {
        if self.eof_seen {
            return Ok(None);
        }
        let mut filled = 0usize;
        while filled < self.frame_bytes {
            let n = self.stdout.read(&mut self.buf[filled..]).await?;
            if n == 0 {
                self.eof_seen = true;
                if filled == 0 {
                    return Ok(None);
                }
                return Err(FrameIoError::PartialFrame {
                    bytes_read: filled,
                    frame_bytes: self.frame_bytes,
                });
            }
            filled += n;
        }
        Ok(Some(&self.buf[..self.frame_bytes]))
    }

    /// Close the underlying decoder.
    ///
    /// Behavior depends on whether the caller drained the stream:
    /// * If `next_frame` returned `None` at least once (`eof_seen`),
    ///   we wait for the child to exit cleanly and surface non-zero
    ///   exit codes with stderr context.
    /// * Otherwise we're closing early — we kill the child and
    ///   intentionally ignore exit status, since FFmpeg would
    ///   otherwise hit SIGPIPE on its next write and report a broken
    ///   pipe that is purely a consequence of the caller stopping.
    pub async fn close(mut self) -> Result<(), FrameIoError> {
        if self.eof_seen {
            drop(self.stdout);
            let status = self.child.wait().await?;
            if status.success() {
                return Ok(());
            }
            let mut stderr_buf = String::new();
            if let Some(mut e) = self.child.stderr.take() {
                let _ = e.read_to_string(&mut stderr_buf).await;
            }
            return Err(FrameIoError::FfmpegExit {
                stage: "decode",
                status: status.to_string(),
                stderr: stderr_buf,
            });
        }
        let _ = self.child.kill().await;
        let _ = self.child.wait().await;
        Ok(())
    }
}

/// Streaming RGBA frame sink.
///
/// Constructed by spawning FFmpeg with `-f rawvideo -pix_fmt rgba -s
/// WxH -r FPS -i - -c:v libx264 -pix_fmt yuv420p -an <out>.mp4`.
/// Callers push frames via [`push_frame`](FrameEncoder::push_frame),
/// then call [`finish`](FrameEncoder::finish) to close stdin and wait
/// for the encoder to flush.
pub struct FrameEncoder {
    child: Child,
    stdin: Option<ChildStdin>,
    width: u32,
    height: u32,
    frame_bytes: usize,
}

impl FrameEncoder {
    /// Open a streaming encoder writing to `output_path`. The output
    /// is a video-only MP4 at `width x height`, `fps` frames per
    /// second.
    pub async fn open(
        output_path: &Path,
        width: u32,
        height: u32,
        fps: u32,
    ) -> Result<Self, FrameIoError> {
        if width == 0 || height == 0 || fps == 0 {
            return Err(FrameIoError::BadFrameSize {
                expected: 0,
                actual: 0,
                width,
                height,
            });
        }
        let bin = ffmpeg_path()?;
        let mut child = Command::new(bin)
            .arg("-hide_banner")
            .arg("-loglevel")
            .arg("error")
            .arg("-y")
            .arg("-f")
            .arg("rawvideo")
            .arg("-pix_fmt")
            .arg("rgba")
            .arg("-s")
            .arg(format!("{width}x{height}"))
            .arg("-r")
            .arg(format!("{fps}"))
            .arg("-i")
            .arg("-")
            .arg("-c:v")
            .arg("libx264")
            .arg("-pix_fmt")
            .arg("yuv420p")
            .arg("-an")
            .arg(output_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(FrameIoError::Io)?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| FrameIoError::Io(std::io::Error::other("ffmpeg stdin missing")))?;
        Ok(Self {
            child,
            stdin: Some(stdin),
            width,
            height,
            frame_bytes: (width as usize) * (height as usize) * 4,
        })
    }

    /// Push one RGBA frame to the encoder. The slice must be exactly
    /// `width * height * 4` bytes.
    pub async fn push_frame(&mut self, rgba: &[u8]) -> Result<(), FrameIoError> {
        if rgba.len() != self.frame_bytes {
            return Err(FrameIoError::BadFrameSize {
                expected: self.frame_bytes,
                actual: rgba.len(),
                width: self.width,
                height: self.height,
            });
        }
        let stdin = self.stdin.as_mut().ok_or_else(|| {
            FrameIoError::Io(std::io::Error::other("frame encoder already finished"))
        })?;
        stdin.write_all(rgba).await?;
        Ok(())
    }

    /// Close stdin and wait for the encoder to flush. Returns an
    /// error if FFmpeg exits non-zero.
    pub async fn finish(mut self) -> Result<(), FrameIoError> {
        if let Some(stdin) = self.stdin.take() {
            drop(stdin);
        }
        let output = self.child.wait_with_output().await?;
        if output.status.success() {
            return Ok(());
        }
        Err(FrameIoError::FfmpegExit {
            stage: "encode",
            status: output.status.to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tokio::process::Command;

    /// Skip FFmpeg-touching tests when no ffmpeg binary is present.
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
        let bin = ffmpeg_path().expect("ffmpeg required for this test fixture");
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
            .expect("ffmpeg lavfi synth must spawn");
        assert!(status.success(), "lavfi synth failed: {status}");
    }

    #[tokio::test]
    async fn rejects_zero_dimensions() {
        let result = FrameEncoder::open(Path::new("/tmp/x.mp4"), 0, 8, 30).await;
        assert!(matches!(result, Err(FrameIoError::BadFrameSize { .. })));
    }

    #[tokio::test]
    async fn frame_provider_yields_exact_frame_count() {
        if ffmpeg_or_skip().is_none() {
            eprintln!("skipping: ffmpeg not installed");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let red = dir.path().join("red.mp4");
        synth_color_mp4(&red, "red", 32, 32, 0.5, 30).await;

        let mut provider = FrameProvider::open(&red, 0.0, 0.5, 32, 32, 30)
            .await
            .unwrap();
        let mut count = 0;
        while provider.next_frame().await.unwrap().is_some() {
            count += 1;
        }
        provider.close().await.unwrap();
        // 0.5s at 30fps is 15 frames. FFmpeg may emit 14-16 depending on rounding.
        assert!(
            (14..=16).contains(&count),
            "expected ~15 frames, got {count}"
        );
    }

    #[tokio::test]
    async fn frame_provider_returns_red_pixels_for_red_clip() {
        if ffmpeg_or_skip().is_none() {
            eprintln!("skipping: ffmpeg not installed");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let red = dir.path().join("red.mp4");
        synth_color_mp4(&red, "red", 16, 16, 0.3, 30).await;

        let mut provider = FrameProvider::open(&red, 0.0, 0.3, 16, 16, 30)
            .await
            .unwrap();
        let frame = provider
            .next_frame()
            .await
            .unwrap()
            .expect("first frame must be present");
        // FFmpeg's `color=red` is the rec601-ish red in yuv420p, which
        // decodes back to RGBA roughly (254, 0, 0, 255). Allow a small
        // tolerance for the colorspace round-trip.
        assert!(frame[0] > 240, "R channel low: {}", frame[0]);
        assert!(frame[1] < 16, "G channel high: {}", frame[1]);
        assert!(frame[2] < 16, "B channel high: {}", frame[2]);
        assert_eq!(frame[3], 255);
        provider.close().await.unwrap();
    }

    #[tokio::test]
    async fn round_trips_a_clip_through_provider_and_encoder() {
        if ffmpeg_or_skip().is_none() {
            eprintln!("skipping: ffmpeg not installed");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let red = dir.path().join("red.mp4");
        let out = dir.path().join("roundtrip.mp4");
        synth_color_mp4(&red, "red", 32, 32, 0.5, 30).await;

        let mut provider = FrameProvider::open(&red, 0.0, 0.5, 32, 32, 30)
            .await
            .unwrap();
        let mut encoder = FrameEncoder::open(&out, 32, 32, 30).await.unwrap();

        let mut frames_written = 0usize;
        loop {
            // Borrow the frame, copy into a small buffer so we can drop
            // the provider borrow before awaiting the encoder push.
            let owned: Option<Vec<u8>> = match provider.next_frame().await.unwrap() {
                Some(frame) => Some(frame.to_vec()),
                None => None,
            };
            let Some(frame) = owned else {
                break;
            };
            encoder.push_frame(&frame).await.unwrap();
            frames_written += 1;
        }
        provider.close().await.unwrap();
        encoder.finish().await.unwrap();

        assert!(frames_written >= 14, "wrote {frames_written} frames");
        let metadata = std::fs::metadata(&out).unwrap();
        assert!(metadata.len() > 0, "output mp4 should be non-empty");
    }

    #[tokio::test]
    async fn frame_encoder_rejects_wrong_size_buffer() {
        if ffmpeg_or_skip().is_none() {
            eprintln!("skipping: ffmpeg not installed");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("bad.mp4");
        let mut encoder = FrameEncoder::open(&out, 16, 16, 30).await.unwrap();
        let too_small = vec![0u8; 16];
        let err = encoder.push_frame(&too_small).await.unwrap_err();
        assert!(matches!(err, FrameIoError::BadFrameSize { .. }));
        // Even on the error path, finish() must drain the child or it
        // will linger as a zombie.
        let _ = encoder.finish().await;
    }
}
