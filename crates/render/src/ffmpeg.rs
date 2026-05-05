//! Locate the `ffmpeg` / `ffprobe` binaries and run small one-shot
//! commands against them.
//!
//! Discovery order (matches what every Rust ffmpeg wrapper does in
//! 2026): the `AWIDAT_FFMPEG` / `AWIDAT_FFPROBE` env overrides first,
//! then `which ffmpeg` / `which ffprobe`. We do not bundle a static
//! ffmpeg binary — that's a v2 packaging concern.
//!
//! Frame extraction uses `-ss` before `-i` for fast keyframe seek; for
//! sub-frame accuracy `-ss` lands after `-i` (slower). We choose the
//! fast path: editorial frame previews don't need sub-frame precision.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::OnceLock;
use std::time::Duration;

use thiserror::Error;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::time::timeout;

/// Errors talking to ffmpeg.
#[derive(Debug, Error)]
pub enum FfmpegError {
    /// Couldn't find the binary.
    #[error("could not locate '{which}': set AWIDAT_FFMPEG (or AWIDAT_FFPROBE) or install via your package manager")]
    NotFound {
        /// Either `"ffmpeg"` or `"ffprobe"`.
        which: &'static str,
    },
    /// Spawn failed.
    #[error("failed to spawn '{path}': {source}")]
    Spawn {
        /// Path attempted.
        path: PathBuf,
        /// Underlying.
        #[source]
        source: std::io::Error,
    },
    /// ffmpeg exited non-zero. `stderr_tail` is the last ~4KB of stderr
    /// (errors live at the end, not the middle).
    #[error("ffmpeg exited {code}: {stderr_tail}")]
    NonZero {
        /// Exit code (or -1 if killed by signal).
        code: i32,
        /// Tail of stderr, capped to STDERR_TAIL_BYTES.
        stderr_tail: String,
    },
    /// Timeout running a one-shot ffmpeg command.
    #[error("ffmpeg timed out after {0:?}")]
    Timeout(Duration),
    /// I/O reading stdout/stderr.
    #[error("ffmpeg I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// `t_s` was negative or non-finite.
    #[error("invalid timestamp {0}: must be finite and non-negative")]
    BadTimestamp(f64),
}

/// Cap on stderr we keep on error. Tail-truncated, NOT middle-truncated:
/// per the corpus survey, ffmpeg errors live at the *end* of stderr.
const STDERR_TAIL_BYTES: usize = 4 * 1024;

/// Default timeout for one-shot commands (frame extract, ffprobe). Long-
/// running renders use the JobManager's own timeout, not this.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Path to the `ffmpeg` binary. Cached after first lookup.
pub fn ffmpeg_path() -> Result<PathBuf, FfmpegError> {
    static CACHE: OnceLock<Result<PathBuf, ()>> = OnceLock::new();
    CACHE
        .get_or_init(|| resolve_binary("ffmpeg", "AWIDAT_FFMPEG"))
        .clone()
        .map_err(|_| FfmpegError::NotFound { which: "ffmpeg" })
}

/// Path to the `ffprobe` binary. Cached after first lookup.
pub fn ffprobe_path() -> Result<PathBuf, FfmpegError> {
    static CACHE: OnceLock<Result<PathBuf, ()>> = OnceLock::new();
    CACHE
        .get_or_init(|| resolve_binary("ffprobe", "AWIDAT_FFPROBE"))
        .clone()
        .map_err(|_| FfmpegError::NotFound { which: "ffprobe" })
}

fn resolve_binary(name: &str, env_var: &str) -> Result<PathBuf, ()> {
    if let Ok(p) = std::env::var(env_var)
        && !p.is_empty()
    {
        let pb = PathBuf::from(p);
        if pb.exists() {
            return Ok(pb);
        }
    }
    // PATH lookup. We don't pull in `which` to keep the dep tree small —
    // walk PATH ourselves.
    let path_env = std::env::var_os("PATH").ok_or(())?;
    for dir in std::env::split_paths(&path_env) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Ok(candidate);
        }
        // On macOS, `ffmpeg` may live in `/opt/homebrew/bin/` even when
        // PATH wasn't propagated (e.g. spawned from a non-login shell).
        // We could probe a few well-known paths here, but env override
        // is the documented escape hatch — keep this lookup simple.
    }
    Err(())
}

/// Extract a single frame at time `t_s` from `asset_path`. Returns the
/// raw image bytes in the requested format (`png` or `jpeg`).
///
/// `max_dim` (width or height, whichever is greater) clamps the output
/// size; the aspect ratio is preserved. `None` keeps the source size.
///
/// Implementation:
/// `ffmpeg -ss <t_s> -i <asset> -frames:v 1 [-vf scale=...] -f image2pipe -vcodec <codec> -`
pub async fn extract_frame(
    asset_path: &Path,
    t_s: f64,
    format: ImageFormat,
    max_dim: Option<u32>,
) -> Result<Vec<u8>, FfmpegError> {
    if !t_s.is_finite() || t_s < 0.0 {
        return Err(FfmpegError::BadTimestamp(t_s));
    }
    let bin = ffmpeg_path()?;

    let mut cmd = Command::new(&bin);
    // Order matters for seek perf:
    //   `-ss` BEFORE `-i` → input-side seek (fast, keyframe-accurate)
    //   `-ss` AFTER  `-i` → output-side seek (slow, sub-frame-accurate)
    // Editorial preview tolerates keyframe-aligned thumbnails.
    cmd.arg("-loglevel").arg("error")
        .arg("-y")
        .arg("-ss").arg(format!("{t_s}"))
        .arg("-i").arg(asset_path)
        .arg("-frames:v").arg("1");

    if let Some(dim) = max_dim {
        // Preserve aspect ratio; clamp the larger dimension to `dim`.
        // The `-2` keeps the other dimension even-numbered (ffmpeg
        // requires even dims for many codecs).
        cmd.arg("-vf")
            .arg(format!("scale='if(gt(iw,ih),{dim},-2)':'if(gt(iw,ih),-2,{dim})'"));
    }

    cmd.arg("-f").arg("image2pipe")
        .arg("-vcodec").arg(format.codec_name())
        .arg("-");

    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let mut child = cmd.spawn().map_err(|e| FfmpegError::Spawn {
        path: bin.clone(),
        source: e,
    })?;

    let stdout = child.stdout.take().ok_or_else(|| FfmpegError::Io(std::io::Error::other("ffmpeg stdout missing")))?;
    let stderr = child.stderr.take().ok_or_else(|| FfmpegError::Io(std::io::Error::other("ffmpeg stderr missing")))?;

    let collect_fut = async move {
        let mut so = Vec::new();
        let mut se = Vec::new();
        let mut stdout_buf = stdout;
        let mut stderr_buf = stderr;
        let (a, b) = tokio::join!(
            stdout_buf.read_to_end(&mut so),
            stderr_buf.read_to_end(&mut se),
        );
        a?;
        b?;
        let status = child.wait().await?;
        Ok::<_, std::io::Error>((status, so, se))
    };

    let (status, stdout_bytes, stderr_bytes) = match timeout(DEFAULT_TIMEOUT, collect_fut).await {
        Ok(Ok(t)) => t,
        Ok(Err(e)) => return Err(FfmpegError::Io(e)),
        Err(_) => return Err(FfmpegError::Timeout(DEFAULT_TIMEOUT)),
    };

    if !status.success() {
        let stderr_tail = tail_string(&stderr_bytes, STDERR_TAIL_BYTES);
        return Err(FfmpegError::NonZero {
            code: status.code().unwrap_or(-1),
            stderr_tail,
        });
    }
    Ok(stdout_bytes)
}

/// Probe the source duration (in seconds) of a media asset. Used by
/// [`transcode_proxy`] to compute progress percent. Returns `None`
/// if ffprobe couldn't determine the duration (some formats don't
/// expose it without scanning the whole file).
pub async fn probe_duration_s(asset_path: &Path) -> Result<Option<f64>, FfmpegError> {
    let bin = ffprobe_path()?;
    let mut cmd = Command::new(&bin);
    cmd.arg("-v")
        .arg("error")
        .arg("-show_entries")
        .arg("format=duration")
        .arg("-of")
        .arg("default=noprint_wrappers=1:nokey=1")
        .arg(asset_path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let mut child = cmd.spawn().map_err(|e| FfmpegError::Spawn {
        path: bin.clone(),
        source: e,
    })?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| FfmpegError::Io(std::io::Error::other("ffprobe stdout missing")))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| FfmpegError::Io(std::io::Error::other("ffprobe stderr missing")))?;

    let collect_fut = async move {
        let mut so = Vec::new();
        let mut se = Vec::new();
        let mut stdout_buf = stdout;
        let mut stderr_buf = stderr;
        let (a, b) = tokio::join!(
            stdout_buf.read_to_end(&mut so),
            stderr_buf.read_to_end(&mut se),
        );
        a?;
        b?;
        let status = child.wait().await?;
        Ok::<_, std::io::Error>((status, so, se))
    };

    let (status, stdout_bytes, stderr_bytes) = match timeout(DEFAULT_TIMEOUT, collect_fut).await {
        Ok(Ok(t)) => t,
        Ok(Err(e)) => return Err(FfmpegError::Io(e)),
        Err(_) => return Err(FfmpegError::Timeout(DEFAULT_TIMEOUT)),
    };
    if !status.success() {
        let stderr_tail = tail_string(&stderr_bytes, STDERR_TAIL_BYTES);
        return Err(FfmpegError::NonZero {
            code: status.code().unwrap_or(-1),
            stderr_tail,
        });
    }
    let s = String::from_utf8_lossy(&stdout_bytes);
    let parsed = s.trim().parse::<f64>().ok();
    Ok(parsed.filter(|d| d.is_finite() && *d > 0.0))
}

/// Per-tick progress emission from [`transcode_proxy`]. Identical
/// shape to `IndexProgress` to keep the desktop's protocol mapping
/// uniform across long-running tasks.
#[derive(Debug, Clone)]
pub enum TranscodeProgress {
    /// Fired before the first frame; carries the source duration if
    /// known so the UI can show "0% · 0s / 12m".
    Started {
        /// Total source duration, or None if ffprobe failed to
        /// determine it (the UI then runs as indeterminate).
        total_duration_s: Option<f64>,
    },
    /// Fired roughly every ~500ms during the run. `percent` is `None`
    /// when total duration was unknown at start (indeterminate).
    Tick {
        /// 0..=100 if computable, None for indeterminate progress.
        percent: Option<u8>,
        /// Most recent ffmpeg progress line, for diagnostics.
        line: String,
    },
}

/// Caller-supplied progress sink for [`transcode_proxy`]. Same
/// shape as `awidat_index::ProgressCallback` (Arc-wrapped trait
/// object, sync `Fn`, `Send + Sync + 'static`) so the desktop can
/// wrap it the same way.
pub type TranscodeProgressCallback = std::sync::Arc<dyn Fn(TranscodeProgress) + Send + Sync + 'static>;

/// Transcode `asset_path` into a 720p H.264 proxy at `output_path`.
///
/// Pipeline: `ffmpeg -i <src> -vf scale=-2:720 -c:v libx264 -preset
/// veryfast -crf 26 -g 1 -keyint_min 1 -sc_threshold 0 -c:a aac
/// -b:a 128k -movflags +faststart -y <out>`
///
/// `-g 1 -keyint_min 1 -sc_threshold 0` makes every frame a
/// keyframe — proxies prioritize random-access seeking over file
/// size. The output is roughly 5–10× larger than what a CRF 26
/// h264 with default GOP would be, but seeks are O(frame) instead
/// of O(GOP).
///
/// `cancel` is polled every progress tick; when fired the ffmpeg
/// child is killed and the function returns `Err(NonZero)` with
/// stderr empty (or `Err(Io)` if the kill races).
pub async fn transcode_proxy(
    asset_path: &Path,
    output_path: &Path,
    progress: Option<TranscodeProgressCallback>,
    cancel: tokio_util::sync::CancellationToken,
) -> Result<(), FfmpegError> {
    let bin = ffmpeg_path()?;

    // Ensure output dir exists (caller may have created the proxy/
    // dir first, but this saves a code path on the consumer side).
    if let Some(parent) = output_path.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(FfmpegError::Io)?;
    }

    // Probe duration up front so the UI can show a real percent. A
    // failure here downgrades us to indeterminate progress — we don't
    // bail the whole transcode.
    let total_duration_s = match probe_duration_s(asset_path).await {
        Ok(d) => d,
        Err(_) => None,
    };
    if let Some(cb) = progress.as_ref() {
        cb(TranscodeProgress::Started { total_duration_s });
    }

    let mut cmd = Command::new(&bin);
    cmd.arg("-loglevel")
        .arg("error")
        .arg("-progress")
        .arg("pipe:2")
        .arg("-nostats")
        .arg("-y")
        .arg("-i")
        .arg(asset_path)
        .arg("-vf")
        .arg("scale=-2:720")
        .arg("-c:v")
        .arg("libx264")
        .arg("-preset")
        .arg("veryfast")
        .arg("-crf")
        .arg("26")
        .arg("-g")
        .arg("1")
        .arg("-keyint_min")
        .arg("1")
        .arg("-sc_threshold")
        .arg("0")
        .arg("-pix_fmt")
        .arg("yuv420p")
        .arg("-c:a")
        .arg("aac")
        .arg("-b:a")
        .arg("128k")
        .arg("-movflags")
        .arg("+faststart")
        .arg(output_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let mut child = cmd.spawn().map_err(|e| FfmpegError::Spawn {
        path: bin.clone(),
        source: e,
    })?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| FfmpegError::Io(std::io::Error::other("ffmpeg stderr missing")))?;

    // Drive stderr in a parallel task so the wait() future and the
    // cancel future can race cleanly. The progress task ticks the
    // callback per `out_time_us=` line ffmpeg emits via `-progress`.
    let progress_task = {
        let progress = progress.clone();
        tokio::spawn(async move {
            use tokio::io::AsyncBufReadExt;
            let mut reader = tokio::io::BufReader::new(stderr).lines();
            let mut snapshot = crate::progress::ProgressSnapshot::default();
            let mut tail = Vec::<String>::new();
            while let Ok(Some(line)) = reader.next_line().await {
                // -progress emits key=value pairs; parse as a virtual
                // ffmpeg progress line so existing snapshot logic
                // works. Specifically: `out_time_ms=12345` ms and
                // `progress=continue|end` are what we care about.
                if let Some(rest) = line.strip_prefix("out_time_ms=") {
                    if let Ok(us) = rest.parse::<i64>() {
                        snapshot.time_done_s = Some((us as f64) / 1_000_000.0);
                    }
                } else if let Some(rest) = line.strip_prefix("frame=") {
                    if let Ok(n) = rest.parse::<u64>() {
                        snapshot.frames_done = Some(n);
                    }
                } else if let Some(rest) = line.strip_prefix("speed=") {
                    if let Some(num) = rest.trim().strip_suffix('x') {
                        if let Ok(s) = num.parse::<f64>() {
                            snapshot.speed = Some(s);
                        }
                    }
                } else if line == "progress=end" {
                    if let Some(cb) = progress.as_ref() {
                        cb(TranscodeProgress::Tick {
                            percent: Some(100),
                            line: "progress=end".into(),
                        });
                    }
                    continue;
                } else {
                    // Non-progress line — keep tail for error reporting.
                    if !line.is_empty() {
                        tail.push(line);
                        // Cap retained tail at ~100 lines.
                        if tail.len() > 100 {
                            tail.remove(0);
                        }
                    }
                    continue;
                }
                if let Some(cb) = progress.as_ref() {
                    let pct = snapshot
                        .percent(total_duration_s)
                        .map(|f| f.round().clamp(0.0, 100.0) as u8);
                    cb(TranscodeProgress::Tick {
                        percent: pct,
                        line: snapshot.last_line.clone().unwrap_or_default(),
                    });
                }
            }
            tail.join("\n")
        })
    };

    // Race wait against cancel.
    let status = tokio::select! {
        _ = cancel.cancelled() => {
            let _ = child.start_kill();
            // wait so we don't leak the child; we don't read stderr
            // tail because the kill races and stderr was moved to
            // the progress task already.
            let _ = child.wait().await;
            return Err(FfmpegError::NonZero {
                code: -1,
                stderr_tail: "cancelled".into(),
            });
        }
        st = child.wait() => st.map_err(FfmpegError::Io)?,
    };

    let stderr_tail = match progress_task.await {
        Ok(s) => s,
        Err(_) => String::new(),
    };

    if !status.success() {
        return Err(FfmpegError::NonZero {
            code: status.code().unwrap_or(-1),
            stderr_tail,
        });
    }
    Ok(())
}

/// Image format for [`extract_frame`].
#[derive(Debug, Clone, Copy)]
pub enum ImageFormat {
    /// PNG. Larger but lossless; preferred for editorial preview.
    Png,
    /// JPEG. Smaller; use when the model's image quota is tight.
    Jpeg,
}

impl ImageFormat {
    /// MIME type for the extracted bytes.
    pub fn media_type(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
        }
    }

    fn codec_name(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "mjpeg",
        }
    }
}

fn tail_string(bytes: &[u8], cap: usize) -> String {
    let start = bytes.len().saturating_sub(cap);
    String::from_utf8_lossy(&bytes[start..]).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bad_timestamp_is_error() {
        let result = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                extract_frame(Path::new("/nonexistent.mp4"), -1.0, ImageFormat::Png, None).await
            });
        assert!(matches!(result, Err(FfmpegError::BadTimestamp(_))));

        let result = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                extract_frame(Path::new("/nonexistent.mp4"), f64::NAN, ImageFormat::Png, None).await
            });
        assert!(matches!(result, Err(FfmpegError::BadTimestamp(_))));
    }

    #[test]
    fn media_types() {
        assert_eq!(ImageFormat::Png.media_type(), "image/png");
        assert_eq!(ImageFormat::Jpeg.media_type(), "image/jpeg");
    }

    #[test]
    fn tail_string_caps_at_n() {
        let bytes = b"a".repeat(100);
        let s = tail_string(&bytes, 10);
        assert_eq!(s.len(), 10);
        assert!(s.chars().all(|c| c == 'a'));
    }
}
