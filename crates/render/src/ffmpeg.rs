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

/// Lightweight media stream summary from ffprobe.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MediaProbe {
    /// Container/source duration in seconds when available.
    pub duration_s: Option<f64>,
    /// True when at least one video stream exists.
    pub has_video: bool,
    /// True when at least one audio stream exists.
    pub has_audio: bool,
    /// Codec type strings reported by ffprobe, e.g. video/audio.
    pub stream_types: Vec<String>,
    /// Width of the first video stream in pixels, when available.
    pub video_width: Option<u32>,
    /// Height of the first video stream in pixels, when available.
    pub video_height: Option<u32>,
}

/// Errors talking to ffmpeg.
#[derive(Debug, Error)]
pub enum FfmpegError {
    /// Couldn't find the binary.
    #[error(
        "could not locate '{which}': set AWIDAT_FFMPEG (or AWIDAT_FFPROBE) or install via your package manager"
    )]
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
    // Walk PATH ourselves so we don't pull in `which`.
    let path_env = std::env::var_os("PATH").ok_or(())?;
    for dir in std::env::split_paths(&path_env) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Ok(candidate);
        }
        // On macOS ffmpeg often lives in /opt/homebrew/bin/ even when
        // PATH wasn't propagated (e.g. non-login shells); the env-var
        // override is the documented escape hatch in that case.
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
    extract_frame_filtered(asset_path, t_s, format, max_dim, None).await
}

/// Same as [`extract_frame`] but runs the source through a full
/// `-filter_complex` graph (labeled, multi-segment) before scale.
/// Required when the graph has internal labels (e.g. the
/// `split → lut3d → blend` strength block) that the flat `-vf`
/// form can't represent.
///
/// `filter_complex` must use `[in]` for the source label and end
/// in `out_label` (typically `[grade_out]`). The scale stage is
/// spliced on automatically based on `max_dim`. The final
/// `-map` value is the scale stage's output when scaling, or
/// `out_label` directly when not.
pub async fn extract_frame_complex(
    asset_path: &Path,
    t_s: f64,
    format: ImageFormat,
    filter_complex: &str,
    out_label: &str,
    max_dim: Option<u32>,
) -> Result<Vec<u8>, FfmpegError> {
    if !t_s.is_finite() || t_s < 0.0 {
        return Err(FfmpegError::BadTimestamp(t_s));
    }
    let bin = ffmpeg_path()?;
    let prepared = filter_complex.replace("[in]", "[0:v:0]");
    let (final_graph, map_label) = if let Some(dim) = max_dim {
        let scale = format!("scale='if(gt(iw,ih),{dim},-2)':'if(gt(iw,ih),-2,{dim})'");
        let final_label = "[grade_scaled]";
        let graph = format!("{prepared};{out_label}{scale}{final_label}");
        (graph, final_label.to_string())
    } else {
        (prepared, out_label.to_string())
    };

    let mut cmd = Command::new(&bin);
    cmd.arg("-loglevel")
        .arg("error")
        .arg("-y")
        .arg("-ss")
        .arg(format!("{t_s}"))
        .arg("-i")
        .arg(asset_path)
        .arg("-filter_complex")
        .arg(final_graph)
        .arg("-map")
        .arg(map_label)
        .arg("-frames:v")
        .arg("1")
        .arg("-f")
        .arg("image2pipe")
        .arg("-vcodec")
        .arg(format.codec_name())
        .arg("-");

    cmd.stdin(Stdio::null())
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
        .ok_or_else(|| FfmpegError::Io(std::io::Error::other("ffmpeg stdout missing")))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| FfmpegError::Io(std::io::Error::other("ffmpeg stderr missing")))?;

    let collect_fut = async move {
        let mut so = Vec::new();
        let mut se = Vec::new();
        let mut stdout_buf = stdout;
        let mut stderr_buf = stderr;
        let (a, b) = tokio::join!(
            stdout_buf.read_to_end(&mut so),
            stderr_buf.read_to_end(&mut se),
        );
        a.map_err(FfmpegError::Io)?;
        b.map_err(FfmpegError::Io)?;
        Ok::<(Vec<u8>, Vec<u8>), FfmpegError>((so, se))
    };

    let status_fut = async { child.wait().await.map_err(FfmpegError::Io) };
    let ((stdout_bytes, stderr_bytes), status) = tokio::try_join!(collect_fut, status_fut)?;
    if !status.success() {
        let stderr_tail = tail_string(&stderr_bytes, STDERR_TAIL_BYTES);
        return Err(FfmpegError::NonZero {
            code: status.code().unwrap_or(-1),
            stderr_tail,
        });
    }
    Ok(stdout_bytes)
}

/// Same as [`extract_frame`] but prepends an additional flat filter
/// chain (comma-separated, no labels) before the optional `scale`
/// filter. Used to render graded previews where the chain mirrors
/// the per-clip color filters the full timeline render would apply.
///
/// `vf_prefix` must be a syntactically valid `-vf` chain; the caller
/// is expected to have escaped paths via [`crate::timeline`] helpers.
/// Passing `None` is identical to [`extract_frame`].
pub async fn extract_frame_filtered(
    asset_path: &Path,
    t_s: f64,
    format: ImageFormat,
    max_dim: Option<u32>,
    vf_prefix: Option<&str>,
) -> Result<Vec<u8>, FfmpegError> {
    if !t_s.is_finite() || t_s < 0.0 {
        return Err(FfmpegError::BadTimestamp(t_s));
    }
    let bin = ffmpeg_path()?;

    let mut cmd = Command::new(&bin);
    // `-ss` before `-i` → input-side seek (fast, keyframe-accurate);
    // `-ss` after `-i` → output-side seek (slow, sub-frame-accurate).
    // Editorial previews tolerate keyframe-aligned thumbnails.
    cmd.arg("-loglevel")
        .arg("error")
        .arg("-y")
        .arg("-ss")
        .arg(format!("{t_s}"))
        .arg("-i")
        .arg(asset_path)
        .arg("-frames:v")
        .arg("1");

    let scale_chain = max_dim.map(|dim| {
        // The `-2` keeps the other dimension even-numbered, which many
        // codecs require.
        format!("scale='if(gt(iw,ih),{dim},-2)':'if(gt(iw,ih),-2,{dim})'")
    });
    let combined_vf = match (vf_prefix, scale_chain) {
        (None, None) => None,
        (Some(p), None) => Some(p.to_string()),
        (None, Some(s)) => Some(s),
        (Some(p), Some(s)) => Some(format!("{p},{s}")),
    };
    if let Some(vf) = combined_vf {
        cmd.arg("-vf").arg(vf);
    }

    cmd.arg("-f")
        .arg("image2pipe")
        .arg("-vcodec")
        .arg(format.codec_name())
        .arg("-");

    cmd.stdin(Stdio::null())
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
        .ok_or_else(|| FfmpegError::Io(std::io::Error::other("ffmpeg stdout missing")))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| FfmpegError::Io(std::io::Error::other("ffmpeg stderr missing")))?;

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

/// Extract a single grayscale frame at time `t_s` from `asset_path`, returning
/// `(width, height, luma)` where `luma.len() == width * height` and pixels are
/// row-major 8-bit luma. Designed for downstream caption rendered-output
/// scoring where the consumer only needs the luma plane and a PNG encode +
/// decode roundtrip is wasted work.
///
/// Implementation:
/// 1. `ffmpeg -hide_banner -loglevel error -i <asset> -frames:v 1 -vf showinfo
///    -f null -` is used to discover the post-decode frame dimensions from
///    `showinfo`'s `s:WxH` field.
/// 2. `ffmpeg -ss <t_s> -i <asset> -frames:v 1 -f rawvideo -pix_fmt gray -`
///    emits the raw luma bytes for the seek position.
pub async fn extract_frame_raw_gray(
    asset_path: &Path,
    t_s: f64,
) -> Result<(u32, u32, Vec<u8>), FfmpegError> {
    if !t_s.is_finite() || t_s < 0.0 {
        return Err(FfmpegError::BadTimestamp(t_s));
    }
    let bin = ffmpeg_path()?;

    let mut probe = Command::new(&bin);
    probe
        .arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-i")
        .arg(asset_path)
        .arg("-frames:v")
        .arg("1")
        .arg("-vf")
        .arg("showinfo")
        .arg("-f")
        .arg("null")
        .arg("-")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let probe_output = probe.output().await.map_err(|e| FfmpegError::Spawn {
        path: bin.clone(),
        source: e,
    })?;
    let stderr_text = String::from_utf8_lossy(&probe_output.stderr);
    let (width, height) = parse_showinfo_dimensions(&stderr_text).ok_or_else(|| {
        FfmpegError::Io(std::io::Error::other(
            "ffmpeg showinfo did not report stream dimensions",
        ))
    })?;

    let mut cmd = Command::new(&bin);
    cmd.arg("-loglevel")
        .arg("error")
        .arg("-y")
        .arg("-ss")
        .arg(format!("{t_s}"))
        .arg("-i")
        .arg(asset_path)
        .arg("-frames:v")
        .arg("1")
        .arg("-f")
        .arg("rawvideo")
        .arg("-pix_fmt")
        .arg("gray")
        .arg("-")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let output = cmd.output().await.map_err(|e| FfmpegError::Spawn {
        path: bin.clone(),
        source: e,
    })?;
    if !output.status.success() {
        let stderr_tail = tail_string(&output.stderr, STDERR_TAIL_BYTES);
        return Err(FfmpegError::NonZero {
            code: output.status.code().unwrap_or(-1),
            stderr_tail,
        });
    }
    let expected = (width as usize).saturating_mul(height as usize);
    if output.stdout.len() < expected {
        return Err(FfmpegError::Io(std::io::Error::other(format!(
            "ffmpeg rawvideo returned {} bytes, expected at least {}",
            output.stdout.len(),
            expected
        ))));
    }
    let mut luma = output.stdout;
    luma.truncate(expected);
    Ok((width, height, luma))
}

fn parse_showinfo_dimensions(stderr_text: &str) -> Option<(u32, u32)> {
    for line in stderr_text.lines() {
        let Some(idx) = line.find(" s:") else {
            continue;
        };
        let rest = &line[idx + 3..];
        let end = rest
            .find(|c: char| !(c.is_ascii_digit() || c == 'x'))
            .unwrap_or(rest.len());
        let spec = &rest[..end];
        let (w, h) = spec.split_once('x')?;
        if let (Ok(w), Ok(h)) = (w.parse::<u32>(), h.parse::<u32>())
            && w > 0
            && h > 0
        {
            return Some((w, h));
        }
    }
    None
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

/// Probe duration and stream presence in one ffprobe call.
pub async fn probe_media(asset_path: &Path) -> Result<MediaProbe, FfmpegError> {
    let bin = ffprobe_path()?;
    let mut cmd = Command::new(&bin);
    cmd.arg("-v")
        .arg("error")
        .arg("-show_entries")
        .arg("format=duration:stream=codec_type,width,height")
        .arg("-of")
        .arg("json")
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

    #[derive(serde::Deserialize)]
    struct ProbeJson {
        #[serde(default)]
        streams: Vec<StreamJson>,
        format: Option<FormatJson>,
    }
    #[derive(serde::Deserialize)]
    struct StreamJson {
        codec_type: Option<String>,
        width: Option<u32>,
        height: Option<u32>,
    }
    #[derive(serde::Deserialize)]
    struct FormatJson {
        duration: Option<String>,
    }

    let parsed: ProbeJson = serde_json::from_slice(&stdout_bytes)
        .map_err(|e| FfmpegError::Io(std::io::Error::other(format!("parse ffprobe json: {e}"))))?;
    let duration_s = parsed
        .format
        .and_then(|f| f.duration)
        .and_then(|d| d.parse::<f64>().ok())
        .filter(|d| d.is_finite() && *d > 0.0);
    let video_width = parsed
        .streams
        .iter()
        .find(|s| s.codec_type.as_deref() == Some("video"))
        .and_then(|s| s.width)
        .filter(|w| *w > 0);
    let video_height = parsed
        .streams
        .iter()
        .find(|s| s.codec_type.as_deref() == Some("video"))
        .and_then(|s| s.height)
        .filter(|h| *h > 0);
    let stream_types: Vec<String> = parsed
        .streams
        .into_iter()
        .filter_map(|s| s.codec_type)
        .collect();
    let has_video = stream_types.iter().any(|s| s == "video");
    let has_audio = stream_types.iter().any(|s| s == "audio");
    Ok(MediaProbe {
        duration_s,
        has_video,
        has_audio,
        stream_types,
        video_width,
        video_height,
    })
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
pub type TranscodeProgressCallback =
    std::sync::Arc<dyn Fn(TranscodeProgress) + Send + Sync + 'static>;

/// Proxy resolution + codec quality.
///
/// **Why 1440p, not 1080p?** Modern displays are 2x DPR (Retina,
/// 4K, HDR monitors). A 1080p proxy rendered into a CSS-540 box on a
/// 2x display only fills ~540 physical pixels — the browser then
/// upscales for the actual screen, producing visible blur. 1440p
/// gives us enough source pixels that even a half-width preview pane
/// on a Retina screen shows crisp detail.
///
/// **Why CRF 20, not 26?** CRF 26 trades ~5 dB of PSNR for file size.
/// On a still frame that's the visual difference between "looks good"
/// and "looks soft". DaVinci's preview proxies are typically CRF
/// 18-20. Bumped to 20 — file size grows ~50% but the all-keyframe
/// path was already inflating size 5-10x over a normal GOP, so the
/// absolute hit is modest.
const PROXY_TARGET_HEIGHT: u32 = 1440;
const PROXY_CRF: &str = "20";
/// Schema tag embedded in proxy filenames. Bumping this string
/// invalidates every existing proxy at the next project-load orphan
/// scan, forcing a clean re-transcode under the new height/CRF
/// targets. Older `*-1080p-*` files become orphans on the next scan
/// and get cleaned up. Format: `<height>q<crf>` so a future bump
/// (e.g. `2160q18`) is self-describing in the filename.
pub const PROXY_SCHEMA_TAG: &str = "1440q20";

fn proxy_scale_filter() -> String {
    format!("scale=-2:{PROXY_TARGET_HEIGHT}")
}

/// Transcode `asset_path` into a 1080p H.264 proxy at `output_path`.
///
/// Pipeline: `ffmpeg -i <src> -vf scale=-2:1080 -c:v libx264 -preset
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

    if let Some(parent) = output_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(FfmpegError::Io)?;
    }

    // Probe duration up front so the UI can render a real percent;
    // failure downgrades to indeterminate progress rather than aborting.
    let total_duration_s = probe_duration_s(asset_path).await.unwrap_or_default();
    if let Some(cb) = progress.as_ref() {
        cb(TranscodeProgress::Started { total_duration_s });
    }

    if should_try_lossless_mp4_remux(asset_path) {
        let mut remux = Command::new(&bin);
        remux
            .arg("-loglevel")
            .arg("error")
            .arg("-y")
            .arg("-i")
            .arg(asset_path)
            .arg("-map")
            .arg("0:v:0")
            .arg("-map")
            .arg("0:a?")
            .arg("-c")
            .arg("copy")
            .arg("-movflags")
            .arg("+faststart")
            .arg("-f")
            .arg("mp4")
            .arg(output_path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        match run_short_ffmpeg(remux, cancel.clone()).await {
            Ok(()) => {
                if let Some(cb) = progress.as_ref() {
                    cb(TranscodeProgress::Tick {
                        percent: Some(100),
                        line: "lossless mp4 remux complete".into(),
                    });
                }
                return Ok(());
            }
            Err(e) => {
                let _ = tokio::fs::remove_file(output_path).await;
                tracing::debug!(
                    error = %e,
                    asset = %asset_path.display(),
                    "lossless mp4 remux failed; falling back to encoded proxy",
                );
            }
        }
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
        .arg(proxy_scale_filter())
        .arg("-c:v")
        .arg("libx264")
        .arg("-preset")
        .arg("veryfast")
        .arg("-crf")
        .arg(PROXY_CRF)
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
        .arg("-f")
        .arg("mp4")
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
    // cancel future race cleanly. The progress task ticks the callback
    // per `out_time_us=` line ffmpeg emits via `-progress`.
    let progress_task = {
        let progress = progress.clone();
        tokio::spawn(async move {
            use tokio::io::AsyncBufReadExt;
            let mut reader = tokio::io::BufReader::new(stderr).lines();
            let mut snapshot = crate::progress::ProgressSnapshot::default();
            let mut tail = Vec::<String>::new();
            while let Ok(Some(line)) = reader.next_line().await {
                if let Some(rest) = line.strip_prefix("out_time_ms=") {
                    if let Ok(us) = rest.parse::<i64>() {
                        snapshot.time_done_s = Some((us as f64) / 1_000_000.0);
                    }
                } else if let Some(rest) = line.strip_prefix("frame=") {
                    if let Ok(n) = rest.parse::<u64>() {
                        snapshot.frames_done = Some(n);
                    }
                } else if let Some(rest) = line.strip_prefix("speed=") {
                    if let Some(num) = rest.trim().strip_suffix('x')
                        && let Ok(s) = num.parse::<f64>()
                    {
                        snapshot.speed = Some(s);
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

    let stderr_tail = progress_task.await.unwrap_or_default();

    if !status.success() {
        return Err(FfmpegError::NonZero {
            code: status.code().unwrap_or(-1),
            stderr_tail,
        });
    }
    Ok(())
}

fn should_try_lossless_mp4_remux(asset_path: &Path) -> bool {
    matches!(
        asset_path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("mov" | "m4v")
    )
}

async fn run_short_ffmpeg(
    mut cmd: Command,
    cancel: tokio_util::sync::CancellationToken,
) -> Result<(), FfmpegError> {
    let mut child = cmd.spawn().map_err(|e| FfmpegError::Spawn {
        path: PathBuf::from("ffmpeg"),
        source: e,
    })?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| FfmpegError::Io(std::io::Error::other("ffmpeg stderr missing")))?;
    let stderr_task = tokio::spawn(async move {
        let mut stderr = stderr;
        let mut bytes = Vec::new();
        let _ = stderr.read_to_end(&mut bytes).await;
        tail_string(&bytes, STDERR_TAIL_BYTES)
    });
    let status = tokio::select! {
        _ = cancel.cancelled() => {
            let _ = child.start_kill();
            let _ = child.wait().await;
            return Err(FfmpegError::NonZero {
                code: -1,
                stderr_tail: "cancelled".into(),
            });
        }
        st = child.wait() => st.map_err(FfmpegError::Io)?,
    };
    let stderr_tail = stderr_task.await.unwrap_or_default();
    if status.success() {
        Ok(())
    } else {
        Err(FfmpegError::NonZero {
            code: status.code().unwrap_or(-1),
            stderr_tail,
        })
    }
}

/// Target spec for a platform reframe: output dimensions and an
/// optional video bitrate (kbps). Audio passes through unchanged.
/// See [`reframe_to_target`].
#[derive(Debug, Clone, Copy)]
pub struct ReframeTarget {
    /// Output width in pixels.
    pub width: u32,
    /// Output height in pixels.
    pub height: u32,
    /// Encoded with `-b:v <kbps>k` when set; otherwise ffmpeg picks
    /// a default based on CRF.
    pub video_bitrate_kbps: Option<u32>,
}

/// Reframe / re-encode `input_path` into `output_path` at the target
/// dimensions. The video gets a center-crop to match the target's
/// aspect ratio (so a 16:9 master becomes a 9:16 vertical by trimming
/// the left/right edges, or a 1:1 square by trimming the same edges
/// further), then a scale to the target's pixel size. Audio is copied
/// unchanged so the reframe doesn't re-encode the (already-mastered)
/// audio track.
///
/// Pipeline: `ffmpeg -i <src> -vf
///   "crop=ih*<tw>/<th>*<sh>/<sw>:ih,scale=W:H,setsar=1" \
///   -c:v libx264 -preset veryfast -crf 20 [-b:v <kbps>k] \
///   -pix_fmt yuv420p -c:a copy -movflags +faststart -y <out>`
///
/// We always run the crop because guessing whether the source aspect
/// equals the target aspect would require probing first; using
/// `min(iw/sw, ih/sh)` style math in the filter ensures the crop is a
/// no-op when source and target aspects already match.
pub async fn reframe_to_target(
    input_path: &Path,
    output_path: &Path,
    target: ReframeTarget,
    progress: Option<TranscodeProgressCallback>,
    cancel: tokio_util::sync::CancellationToken,
) -> Result<(), FfmpegError> {
    let bin = ffmpeg_path()?;

    if let Some(parent) = output_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(FfmpegError::Io)?;
    }

    // Probe duration so percent progress is meaningful.
    let total_duration_s = probe_duration_s(input_path).await.unwrap_or_default();
    if let Some(cb) = progress.as_ref() {
        cb(TranscodeProgress::Started { total_duration_s });
    }

    // Build a center-crop + scale filter.
    //
    // The crop expression picks the largest centered rectangle in the
    // source that matches the target's aspect ratio. ffmpeg evaluates
    // `iw`/`ih` at runtime so we don't need to know the source size
    // here. `min(iw, ih*tw/th)` is the crop width that keeps target
    // aspect when the source is wider than target; symmetric on the
    // height side. With both as `min(...)`, ffmpeg picks the smaller
    // dim that fits — effectively a center crop.
    let crop = format!(
        "crop='min(iw,ih*{tw}/{th})':'min(ih,iw*{th}/{tw})'",
        tw = target.width,
        th = target.height,
    );
    let scale = format!("scale={}:{}", target.width, target.height);
    let vf = format!("{crop},{scale},setsar=1");

    let mut cmd = Command::new(&bin);
    cmd.arg("-loglevel")
        .arg("error")
        .arg("-progress")
        .arg("pipe:2")
        .arg("-nostats")
        .arg("-y")
        .arg("-i")
        .arg(input_path)
        .arg("-vf")
        .arg(&vf)
        .arg("-c:v")
        .arg("libx264")
        .arg("-preset")
        .arg("veryfast")
        .arg("-crf")
        .arg("20");
    if let Some(kbps) = target.video_bitrate_kbps {
        cmd.arg("-b:v").arg(format!("{kbps}k"));
        cmd.arg("-maxrate").arg(format!("{kbps}k"));
        cmd.arg("-bufsize").arg(format!("{}k", kbps * 2));
    }
    cmd.arg("-pix_fmt")
        .arg("yuv420p")
        .arg("-c:a")
        .arg("copy")
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

    let progress_task = {
        let progress = progress.clone();
        tokio::spawn(async move {
            use tokio::io::AsyncBufReadExt;
            let mut reader = tokio::io::BufReader::new(stderr).lines();
            let mut snapshot = crate::progress::ProgressSnapshot::default();
            let mut tail = Vec::<String>::new();
            while let Ok(Some(line)) = reader.next_line().await {
                if let Some(rest) = line.strip_prefix("out_time_ms=") {
                    if let Ok(us) = rest.parse::<i64>() {
                        snapshot.time_done_s = Some((us as f64) / 1_000_000.0);
                    }
                } else if let Some(rest) = line.strip_prefix("frame=") {
                    if let Ok(n) = rest.parse::<u64>() {
                        snapshot.frames_done = Some(n);
                    }
                } else if let Some(rest) = line.strip_prefix("speed=") {
                    if let Some(num) = rest.trim().strip_suffix('x')
                        && let Ok(s) = num.parse::<f64>()
                    {
                        snapshot.speed = Some(s);
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
                    if !line.is_empty() {
                        tail.push(line);
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

    let status = tokio::select! {
        _ = cancel.cancelled() => {
            let _ = child.start_kill();
            let _ = child.wait().await;
            return Err(FfmpegError::NonZero {
                code: -1,
                stderr_tail: "cancelled".into(),
            });
        }
        st = child.wait() => st.map_err(FfmpegError::Io)?,
    };

    let stderr_tail = progress_task.await.unwrap_or_default();

    if !status.success() {
        return Err(FfmpegError::NonZero {
            code: status.code().unwrap_or(-1),
            stderr_tail,
        });
    }
    Ok(())
}

/// Extract one filmstrip JPEG per second of source-time from
/// `asset_path` into `output_dir` as `frame-0001.jpg`, `frame-0002.jpg`,
/// … The frontend tiles these across the clip's pixel width on the
/// timeline canvas; per the Step 10 plan we generate at the maximum
/// density we'd ever need (1 / second) and the frontend skips frames
/// when zoomed out.
///
/// Pipeline: `ffmpeg -i <src> -vf "fps=1,scale=120:-2" -vsync vfr
/// <out_dir>/frame-%04d.jpg`. `scale=120:-2` keeps aspect ratio with
/// the long edge clamped to 120 px (`-2` keeps the other dim
/// even-numbered, ffmpeg's mjpeg requirement). `-vsync vfr` skips
/// timestamp duplicates so we don't emit identical frames on
/// variable-FPS sources.
///
/// Idempotency is the caller's job (mtime check, like `proxy_is_fresh`).
/// This function unconditionally re-runs ffmpeg.
///
/// `cancel` kills the ffmpeg child when fired and returns
/// `Err(NonZero { stderr_tail: "cancelled", … })`, mirroring
/// [`transcode_proxy`].
pub async fn generate_thumbnails(
    asset_path: &Path,
    output_dir: &Path,
    cancel: tokio_util::sync::CancellationToken,
) -> Result<(), FfmpegError> {
    let bin = ffmpeg_path()?;

    tokio::fs::create_dir_all(output_dir)
        .await
        .map_err(FfmpegError::Io)?;

    let pattern = output_dir.join("frame-%04d.jpg");

    let mut cmd = Command::new(&bin);
    cmd.arg("-loglevel")
        .arg("error")
        .arg("-nostats")
        .arg("-y")
        .arg("-i")
        .arg(asset_path)
        .arg("-vf")
        .arg("fps=1,scale=120:-2")
        .arg("-vsync")
        .arg("vfr")
        .arg("-q:v")
        .arg("4")
        .arg(&pattern)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let mut child = cmd.spawn().map_err(|e| FfmpegError::Spawn {
        path: bin.clone(),
        source: e,
    })?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| FfmpegError::Io(std::io::Error::other("ffmpeg stderr missing")))?;

    let stderr_task = tokio::spawn(async move {
        let mut buf = Vec::new();
        let _ = stderr.read_to_end(&mut buf).await;
        buf
    });

    let status = tokio::select! {
        _ = cancel.cancelled() => {
            let _ = child.start_kill();
            let _ = child.wait().await;
            return Err(FfmpegError::NonZero {
                code: -1,
                stderr_tail: "cancelled".into(),
            });
        }
        st = child.wait() => st.map_err(FfmpegError::Io)?,
    };

    let stderr_bytes = stderr_task.await.unwrap_or_default();

    if !status.success() {
        return Err(FfmpegError::NonZero {
            code: status.code().unwrap_or(-1),
            stderr_tail: tail_string(&stderr_bytes, STDERR_TAIL_BYTES),
        });
    }
    Ok(())
}

/// Pull a mono 8 kHz f32-PCM stream from `asset_path`, then bucket
/// the absolute samples into `bucket_count` peak amplitudes in
/// `[0.0, 1.0]`. The frontend draws these as a centered amplitude
/// line under each audio clip.
///
/// Pipeline:
/// `ffmpeg -i <src> -map 0:a:0? -ac 1 -ar 8000 -f f32le -`
///
/// `-map 0:a:0?` picks the first audio stream if present, with the
/// `?` making the map optional — assets without audio streams
/// resolve to an empty `Vec<f32>` so callers can short-circuit the
/// "this asset has no audio" path. We pipe raw f32 samples on stdout
/// and bucket on the Rust side; this avoids spawning two ffmpeg
/// processes and keeps the bucketing logic uniform across formats.
///
/// 8 kHz is plenty: the waveform draw resamples again to display
/// width, so the source sample rate just needs to be high enough
/// that long buckets average over a real signal. At 8 kHz a
/// 60-minute podcast yields ~28.8 M samples → ~115 MB of stdout —
/// large but tractable, and we throw the samples away as we
/// bucket. An hour of audio runs in seconds.
///
/// `cancel` kills ffmpeg and returns `Err(NonZero { stderr_tail:
/// "cancelled" })`, mirroring [`transcode_proxy`].
/// Sample rate (Hz) ffmpeg resamples audio to before bucketing. The
/// decoded sample count divided by this is the exact audio duration the
/// peak buckets cover — used to slice the buckets for trimmed clips.
const WAVEFORM_SAMPLE_RATE: usize = 8000;

/// Peak buckets plus the audio duration they span. `duration_s` is
/// derived from the decoded sample count (`samples / 8000`), so it is
/// exactly the span the buckets represent — the correct denominator for
/// mapping a trimmed clip's source range onto the bucket array.
#[derive(Debug, Clone, Default)]
pub struct Waveform {
    /// Peak amplitudes in `[0.0, 1.0]`, one per bucket. Empty when the
    /// asset has no audio stream.
    pub buckets: Vec<f32>,
    /// Total audio duration in seconds the buckets span. `0.0` when the
    /// asset has no audio.
    pub duration_s: f64,
}

pub async fn generate_waveform(
    asset_path: &Path,
    bucket_count: usize,
    cancel: tokio_util::sync::CancellationToken,
) -> Result<Waveform, FfmpegError> {
    if bucket_count == 0 {
        return Ok(Waveform::default());
    }
    let bin = ffmpeg_path()?;

    let mut cmd = Command::new(&bin);
    cmd.arg("-loglevel")
        .arg("error")
        .arg("-nostats")
        .arg("-i")
        .arg(asset_path)
        .arg("-map")
        .arg("0:a:0?")
        .arg("-ac")
        .arg("1")
        .arg("-ar")
        .arg(WAVEFORM_SAMPLE_RATE.to_string())
        .arg("-f")
        .arg("f32le")
        .arg("-")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let mut child = cmd.spawn().map_err(|e| FfmpegError::Spawn {
        path: bin.clone(),
        source: e,
    })?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| FfmpegError::Io(std::io::Error::other("ffmpeg stdout missing")))?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| FfmpegError::Io(std::io::Error::other("ffmpeg stderr missing")))?;

    // Drive stdout into the bucket aggregator and stderr into a tail
    // for error reporting, in parallel. Read in chunks (each f32 is
    // 4 bytes LE) to avoid per-sample syscall overhead.
    let stdout_task = tokio::spawn(async move {
        let mut buf = vec![0u8; 64 * 1024];
        let mut samples = Vec::<f32>::with_capacity(1024 * 1024);
        loop {
            let n = stdout.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            // Drop any trailing bytes the kernel hands us that don't
            // make a full f32 frame.
            let aligned = n - (n % 4);
            for chunk in buf[..aligned].chunks_exact(4) {
                let mut bytes = [0u8; 4];
                bytes.copy_from_slice(chunk);
                samples.push(f32::from_le_bytes(bytes));
            }
        }
        Ok::<_, std::io::Error>(samples)
    });

    let stderr_task = tokio::spawn(async move {
        let mut buf = Vec::new();
        let _ = stderr.read_to_end(&mut buf).await;
        buf
    });

    let status = tokio::select! {
        _ = cancel.cancelled() => {
            let _ = child.start_kill();
            let _ = child.wait().await;
            return Err(FfmpegError::NonZero {
                code: -1,
                stderr_tail: "cancelled".into(),
            });
        }
        st = child.wait() => st.map_err(FfmpegError::Io)?,
    };

    let stderr_bytes = stderr_task.await.unwrap_or_default();
    let samples = match stdout_task.await {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => return Err(FfmpegError::Io(e)),
        Err(_) => {
            return Err(FfmpegError::Io(std::io::Error::other(
                "waveform stdout task panicked",
            )));
        }
    };

    if !status.success() {
        return Err(FfmpegError::NonZero {
            code: status.code().unwrap_or(-1),
            stderr_tail: tail_string(&stderr_bytes, STDERR_TAIL_BYTES),
        });
    }

    // No audio stream (or `-map 0:a:0?` matched nothing). The desktop
    // wrapper treats this as "asset has no audio; skip the sidecar".
    if samples.is_empty() {
        return Ok(Waveform::default());
    }

    let duration_s = samples.len() as f64 / WAVEFORM_SAMPLE_RATE as f64;
    Ok(Waveform {
        buckets: bucket_peaks(&samples, bucket_count),
        duration_s,
    })
}

/// Reduce a sample stream to `bucket_count` peak amplitudes. Each
/// bucket is the maximum absolute sample value in its slice. Output
/// values are `[0.0, 1.0]` (clamped — f32 PCM can technically exceed
/// 1.0 on overdriven sources). Pure-function so unit-testable.
fn bucket_peaks(samples: &[f32], bucket_count: usize) -> Vec<f32> {
    if bucket_count == 0 || samples.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(bucket_count);
    let total = samples.len();
    for i in 0..bucket_count {
        let start = (total * i) / bucket_count;
        let end = (total * (i + 1)) / bucket_count;
        let slice = if start < end {
            &samples[start..end]
        } else {
            // bucket_count > sample count: fall back to one sample per bucket.
            &samples[(start.min(total - 1))..(start.min(total - 1) + 1)]
        };
        let peak = slice.iter().fold(0f32, |acc, &s| acc.max(s.abs())).min(1.0);
        out.push(peak);
    }
    out
}

/// One detected silence range in source-time, produced by
/// [`generate_silences`]. `start_s` / `end_s` bound the silence;
/// `db_floor` is the noise floor ffmpeg measured during the range
/// (closer to `-inf` = quieter; clamped to `-90.0` for assets ffmpeg
/// reports as `-inf`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SilenceRange {
    /// Where the silence began, in source-media seconds.
    pub start_s: f64,
    /// Where the silence ended, in source-media seconds.
    pub end_s: f64,
    /// Average noise floor during the silence (dBFS, negative).
    pub db_floor: f64,
}

/// One detected black-frame range in source time.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BlackFrameRange {
    /// Where the black range began, in source-media seconds.
    pub start_s: f64,
    /// Where the black range ended, in source-media seconds.
    pub end_s: f64,
    /// Duration of the black range, in seconds.
    pub duration_s: f64,
}

/// Detect silence ranges in `asset_path` using ffmpeg's
/// `silencedetect` filter. Returns ranges where audio level was
/// below `threshold_db` for at least `min_duration_s` seconds.
///
/// Pipeline:
/// `ffmpeg -i <src> -af silencedetect=noise=<threshold>dB:d=<min_duration> -f null -`
///
/// `silencedetect` writes pairs of stderr lines per range:
/// ```text
/// [silencedetect @ 0x...] silence_start: 12.345
/// [silencedetect @ 0x...] silence_end: 17.890 | silence_duration: 5.545
/// ```
/// We parse these lines incrementally — assets with no audio
/// stream just produce zero matches, which is the right answer
/// (an empty `Vec<SilenceRange>` round-trips through the sidecar
/// and the find_dead_air tool short-circuits on it).
///
/// Threshold notes: `-40.0 dB` is a sensible default for podcast
/// audio (typical noise floor is around `-50 to -60`, conversational
/// speech is `-15 to -25`). Quieter rooms can use `-50.0`. The
/// caller decides; we just parse what ffmpeg measures.
///
/// `cancel` kills ffmpeg and returns `Err(NonZero { stderr_tail:
/// "cancelled" })`, mirroring [`transcode_proxy`].
pub async fn generate_silences(
    asset_path: &Path,
    threshold_db: f64,
    min_duration_s: f64,
    cancel: tokio_util::sync::CancellationToken,
) -> Result<Vec<SilenceRange>, FfmpegError> {
    if !threshold_db.is_finite() || threshold_db >= 0.0 {
        return Err(FfmpegError::Io(std::io::Error::other(format!(
            "invalid threshold_db {threshold_db}: must be finite and < 0"
        ))));
    }
    if !min_duration_s.is_finite() || min_duration_s <= 0.0 {
        return Err(FfmpegError::Io(std::io::Error::other(format!(
            "invalid min_duration_s {min_duration_s}: must be finite and > 0"
        ))));
    }
    let bin = ffmpeg_path()?;

    let filter = format!("silencedetect=noise={threshold_db}dB:d={min_duration_s}");
    let mut cmd = Command::new(&bin);
    cmd.arg("-loglevel")
        .arg("info") // silencedetect emits at info level
        .arg("-nostats")
        .arg("-i")
        .arg(asset_path)
        .arg("-map")
        .arg("0:a:0?")
        .arg("-af")
        .arg(&filter)
        .arg("-f")
        .arg("null")
        .arg("-")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let mut child = cmd.spawn().map_err(|e| FfmpegError::Spawn {
        path: bin.clone(),
        source: e,
    })?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| FfmpegError::Io(std::io::Error::other("ffmpeg stderr missing")))?;

    let stderr_task = tokio::spawn(async move {
        let mut buf = Vec::new();
        let _ = stderr.read_to_end(&mut buf).await;
        buf
    });

    let status = tokio::select! {
        _ = cancel.cancelled() => {
            let _ = child.start_kill();
            let _ = child.wait().await;
            return Err(FfmpegError::NonZero {
                code: -1,
                stderr_tail: "cancelled".into(),
            });
        }
        st = child.wait() => st.map_err(FfmpegError::Io)?,
    };

    let stderr_bytes = stderr_task.await.unwrap_or_default();

    if !status.success() {
        return Err(FfmpegError::NonZero {
            code: status.code().unwrap_or(-1),
            stderr_tail: tail_string(&stderr_bytes, STDERR_TAIL_BYTES),
        });
    }

    // Empty stderr = audio stream missing or no silences detected.
    // Either way, return an empty vec — caller treats both the same.
    Ok(parse_silence_ranges(&String::from_utf8_lossy(
        &stderr_bytes,
    )))
}

/// Parse silencedetect's stderr output into [`SilenceRange`]s.
/// Pure function, separated for unit testing without spawning ffmpeg.
///
/// silencedetect emits paired lines per range; this scanner walks
/// them in order and matches each `silence_start: <t>` with the
/// next `silence_end: <t> | silence_duration: <t>`. A start without
/// a matching end (truncated stderr) drops the range silently.
fn parse_silence_ranges(stderr: &str) -> Vec<SilenceRange> {
    let mut out = Vec::new();
    let mut pending_start: Option<f64> = None;
    for line in stderr.lines() {
        if let Some(rest) = line.split_once("silence_start:") {
            if let Ok(t) = rest.1.trim().parse::<f64>() {
                pending_start = Some(t);
            }
        } else if let Some(rest) = line.split_once("silence_end:")
            && let Some(start) = pending_start.take()
        {
            // silence_end's payload is "<end_t> | silence_duration: <d>".
            let end_t = rest
                .1
                .trim()
                .split_once('|')
                .map(|(t, _)| t.trim())
                .unwrap_or(rest.1.trim())
                .parse::<f64>()
                .ok();
            if let Some(end) = end_t
                && end > start
            {
                // silencedetect doesn't expose a per-range dB floor —
                // it only reports the threshold we passed in. Use that
                // as a conservative upper bound; the find_dead_air tool
                // only needs ranges, not exact floors.
                out.push(SilenceRange {
                    start_s: start,
                    end_s: end,
                    db_floor: -90.0,
                });
            }
        }
    }
    out
}

/// Detect black video ranges with FFmpeg's `blackdetect` filter.
pub async fn generate_black_frames(
    asset_path: &Path,
    picture_black_ratio_th: f64,
    min_duration_s: f64,
    cancel: tokio_util::sync::CancellationToken,
) -> Result<Vec<BlackFrameRange>, FfmpegError> {
    if !picture_black_ratio_th.is_finite() || !(0.0..=1.0).contains(&picture_black_ratio_th) {
        return Err(FfmpegError::Io(std::io::Error::other(format!(
            "invalid picture_black_ratio_th {picture_black_ratio_th}: must be finite and in 0..=1"
        ))));
    }
    if !min_duration_s.is_finite() || min_duration_s <= 0.0 {
        return Err(FfmpegError::Io(std::io::Error::other(format!(
            "invalid min_duration_s {min_duration_s}: must be finite and > 0"
        ))));
    }
    let bin = ffmpeg_path()?;
    let filter = format!("blackdetect=d={min_duration_s}:pic_th={picture_black_ratio_th}");

    let mut cmd = Command::new(&bin);
    cmd.arg("-loglevel")
        .arg("info")
        .arg("-nostats")
        .arg("-i")
        .arg(asset_path)
        .arg("-map")
        .arg("0:v:0?")
        .arg("-vf")
        .arg(&filter)
        .arg("-an")
        .arg("-f")
        .arg("null")
        .arg("-")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let mut child = cmd.spawn().map_err(|e| FfmpegError::Spawn {
        path: bin.clone(),
        source: e,
    })?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| FfmpegError::Io(std::io::Error::other("ffmpeg stderr missing")))?;

    let stderr_task = tokio::spawn(async move {
        let mut buf = Vec::new();
        let _ = stderr.read_to_end(&mut buf).await;
        buf
    });

    let status = tokio::select! {
        _ = cancel.cancelled() => {
            let _ = child.start_kill();
            let _ = child.wait().await;
            return Err(FfmpegError::NonZero {
                code: -1,
                stderr_tail: "cancelled".into(),
            });
        }
        st = child.wait() => st.map_err(FfmpegError::Io)?,
    };

    let stderr_bytes = stderr_task.await.unwrap_or_default();

    if !status.success() {
        return Err(FfmpegError::NonZero {
            code: status.code().unwrap_or(-1),
            stderr_tail: tail_string(&stderr_bytes, STDERR_TAIL_BYTES),
        });
    }

    Ok(parse_black_frame_ranges(&String::from_utf8_lossy(
        &stderr_bytes,
    )))
}

fn parse_black_frame_ranges(stderr: &str) -> Vec<BlackFrameRange> {
    let mut out = Vec::new();
    for line in stderr.lines() {
        let Some(start_s) = parse_blackdetect_field(line, "black_start:") else {
            continue;
        };
        let Some(end_s) = parse_blackdetect_field(line, "black_end:") else {
            continue;
        };
        let duration_s =
            parse_blackdetect_field(line, "black_duration:").unwrap_or(end_s - start_s);
        if end_s > start_s && duration_s.is_finite() && duration_s > 0.0 {
            out.push(BlackFrameRange {
                start_s,
                end_s,
                duration_s,
            });
        }
    }
    out
}

fn parse_blackdetect_field(line: &str, key: &str) -> Option<f64> {
    let rest = line.split_once(key)?.1.trim_start();
    let token = rest.split_whitespace().next()?;
    let value = token.parse::<f64>().ok()?;
    value.is_finite().then_some(value)
}

/// Mean per-second motion magnitude for [`generate_motion_signal`].
/// Each entry is the scene-change score ffmpeg's `select=` filter
/// computes for one second of source-time, in `[0.0, 1.0]` (clamped
/// — ffmpeg's score is bounded by frame difference normalization).
/// Higher = more motion; near-zero = static frame.
pub type MotionSignal = Vec<f32>;

/// Sample one frame per second of source-time and compute ffmpeg's
/// scene-change score for each. The output is a coarse motion
/// signal: high for action / camera movement, low for talking heads.
/// Used by Phase 2's continuity engine to detect mid-motion cuts —
/// a cut at `t` lands in low-magnitude territory = clean; cut in
/// high-magnitude territory = risky/dirty.
///
/// Implementation: substitutes the plan's `cv2.calcOpticalFlowFarneback`
/// recipe with ffmpeg's built-in scene-change score. Functionally
/// equivalent for the continuity-cut decision (both produce a
/// per-frame "how much did this differ from the previous frame"
/// scalar) and 10× cheaper to ship — no OpenCV system dep, no
/// Python subprocess, no model bundle.
///
/// Pipeline:
/// `ffmpeg -i <src> -vf "fps=1,select='gte(scene\,0)',metadata=mode=print" -f null -`
///
/// `fps=1` downsamples to 1 frame per second BEFORE the scene
/// filter so we get one motion score per source-second instead of
/// per-frame. `select='gte(scene\,0)'` keeps every frame so the
/// metadata filter prints all of them. ffmpeg writes lines like
/// `lavfi.scene_score=0.123456` to stderr; we parse them into the
/// returned vec.
///
/// `cancel` kills ffmpeg and returns `Err(NonZero { stderr_tail:
/// "cancelled" })`, mirroring [`transcode_proxy`].
pub async fn generate_motion_signal(
    asset_path: &Path,
    cancel: tokio_util::sync::CancellationToken,
) -> Result<MotionSignal, FfmpegError> {
    let bin = ffmpeg_path()?;

    // The scene filter runs on every frame fps=1 emits. We use
    // `metadata=mode=print:file=-` to write to stdout — but the
    // print filter has historically written to stderr too, and we
    // pipe `-f null -` with `-loglevel info` so the metadata lines
    // surface. Empirically ffmpeg prints them to stderr at info
    // level; that's what the parser reads.
    let mut cmd = Command::new(&bin);
    cmd.arg("-loglevel")
        .arg("info")
        .arg("-nostats")
        .arg("-i")
        .arg(asset_path)
        .arg("-map")
        .arg("0:v:0?")
        .arg("-vf")
        .arg("fps=1,select='gte(scene\\,0)',metadata=mode=print")
        .arg("-f")
        .arg("null")
        .arg("-")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let mut child = cmd.spawn().map_err(|e| FfmpegError::Spawn {
        path: bin.clone(),
        source: e,
    })?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| FfmpegError::Io(std::io::Error::other("ffmpeg stderr missing")))?;

    let stderr_task = tokio::spawn(async move {
        let mut buf = Vec::new();
        let _ = stderr.read_to_end(&mut buf).await;
        buf
    });

    let status = tokio::select! {
        _ = cancel.cancelled() => {
            let _ = child.start_kill();
            let _ = child.wait().await;
            return Err(FfmpegError::NonZero {
                code: -1,
                stderr_tail: "cancelled".into(),
            });
        }
        st = child.wait() => st.map_err(FfmpegError::Io)?,
    };

    let stderr_bytes = stderr_task.await.unwrap_or_default();

    if !status.success() {
        return Err(FfmpegError::NonZero {
            code: status.code().unwrap_or(-1),
            stderr_tail: tail_string(&stderr_bytes, STDERR_TAIL_BYTES),
        });
    }

    Ok(parse_scene_scores(&String::from_utf8_lossy(&stderr_bytes)))
}

/// Parse ffmpeg's `metadata=mode=print` stderr output into a flat
/// motion-signal vec. Each `lavfi.scene_score=<f>` line is one
/// sample. Unparseable lines are skipped silently. The first frame
/// has no predecessor to diff against, so its score is typically
/// 0 — we keep it (the consumer treats time-zero specially anyway).
///
/// Pure function so unit tests can drive the parser without
/// spinning up ffmpeg.
fn parse_scene_scores(stderr: &str) -> MotionSignal {
    let mut out = Vec::new();
    for line in stderr.lines() {
        if let Some(rest) = line.split("lavfi.scene_score=").nth(1)
            && let Ok(score) = rest.trim().parse::<f32>()
            && score.is_finite()
        {
            out.push(score.clamp(0.0, 1.0));
        }
    }
    out
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
    fn proxy_scale_filter_targets_current_viewer_grade() {
        // Schema bumped from 1080p to 1440p so DPR-2 displays see
        // crisp detail in the preview pane.
        assert_eq!(
            proxy_scale_filter(),
            format!("scale=-2:{PROXY_TARGET_HEIGHT}")
        );
        assert_eq!(PROXY_TARGET_HEIGHT, 1440);
    }

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
                extract_frame(
                    Path::new("/nonexistent.mp4"),
                    f64::NAN,
                    ImageFormat::Png,
                    None,
                )
                .await
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

    #[test]
    fn bucket_peaks_picks_max_abs_per_bucket() {
        let samples = [0.1, -0.5, 0.2, 0.9, -0.3, 0.0, 0.4, -0.8];
        let buckets = bucket_peaks(&samples, 4);
        // Buckets split [0..2], [2..4], [4..6], [6..8].
        assert_eq!(buckets.len(), 4);
        assert!((buckets[0] - 0.5).abs() < 1e-6);
        assert!((buckets[1] - 0.9).abs() < 1e-6);
        assert!((buckets[2] - 0.3).abs() < 1e-6);
        assert!((buckets[3] - 0.8).abs() < 1e-6);
    }

    #[test]
    fn bucket_peaks_clamps_to_one() {
        // f32 PCM can exceed 1.0 on overdriven sources; output is clamped.
        let samples = [3.5_f32, -2.7, 1.001];
        let buckets = bucket_peaks(&samples, 1);
        assert_eq!(buckets, vec![1.0]);
    }

    #[test]
    fn bucket_peaks_zero_buckets_returns_empty() {
        let samples = [0.1_f32, 0.2, 0.3];
        assert!(bucket_peaks(&samples, 0).is_empty());
    }

    #[test]
    fn bucket_peaks_more_buckets_than_samples_does_not_panic() {
        let samples = [0.5_f32, -0.3];
        let buckets = bucket_peaks(&samples, 8);
        assert_eq!(buckets.len(), 8);
        // Each bucket gets *some* peak; first sample dominates the
        // first half, second the latter — the exact split depends on
        // (total*i)/bucket_count rounding, but every value must be
        // one of the two source amplitudes.
        for b in buckets {
            assert!((b - 0.5).abs() < 1e-6 || (b - 0.3).abs() < 1e-6);
        }
    }

    /// End-to-end: synthesize a 2s sine wave with `lavfi` and run
    /// `generate_waveform`. Skipped when ffmpeg isn't on the box.
    #[test]
    fn generate_waveform_against_synthesized_sine() {
        let Ok(_bin) = ffmpeg_path() else {
            return;
        };

        let dir = tempfile::tempdir().unwrap();
        let asset = dir.path().join("sine.wav");

        // Synth a 2s sine via lavfi, write as wav.
        let status = std::process::Command::new(ffmpeg_path().unwrap())
            .arg("-loglevel")
            .arg("error")
            .arg("-y")
            .arg("-f")
            .arg("lavfi")
            .arg("-i")
            .arg("sine=frequency=440:duration=2")
            .arg(&asset)
            .status()
            .expect("synth ffmpeg spawn");
        assert!(status.success(), "synth wav exit: {status}");

        let wave = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                generate_waveform(&asset, 256, tokio_util::sync::CancellationToken::new()).await
            })
            .expect("generate_waveform");

        assert_eq!(wave.buckets.len(), 256);
        // The synth source is exactly 2s; duration is derived from the
        // decoded sample count, so it should land within one resample
        // frame of 2.0s.
        assert!(
            (wave.duration_s - 2.0).abs() < 0.05,
            "expected ~2.0s duration, got {}",
            wave.duration_s,
        );
        // ffmpeg's lavfi `sine=` defaults to ~0.125 amplitude (-18
        // dBFS); we just need to confirm a real signal landed, not
        // any specific level. The bucket-peak max should be well
        // above the noise floor.
        let max_peak = wave.buckets.iter().fold(0f32, |acc, &p| acc.max(p));
        assert!(
            max_peak > 0.05,
            "expected a real signal in the buckets, max was {max_peak}",
        );
        // Sanity: every bucket of a continuous sine should have a
        // non-zero peak — a sine never goes silent.
        let zero_buckets = wave.buckets.iter().filter(|p| **p < 1e-6).count();
        assert!(
            zero_buckets < 5,
            "too many zero buckets: {zero_buckets}/256",
        );
    }

    /// End-to-end: synthesize a 3s test video with `lavfi` and run
    /// `generate_thumbnails` against it. Skipped when ffmpeg isn't on
    /// the box (CI without media tooling).
    #[test]
    fn generate_thumbnails_against_synthesized_mp4() {
        let Ok(bin) = ffmpeg_path() else {
            return; // No ffmpeg, skip — same pattern as other tests.
        };

        let dir = tempfile::tempdir().unwrap();
        let asset = dir.path().join("synth.mp4");

        // Build a 3s testsrc input directly with a blocking ffmpeg
        // call — we don't need the full async machinery here.
        let status = std::process::Command::new(&bin)
            .arg("-loglevel")
            .arg("error")
            .arg("-y")
            .arg("-f")
            .arg("lavfi")
            .arg("-i")
            .arg("testsrc=duration=3:size=320x240:rate=30")
            .arg("-c:v")
            .arg("libx264")
            .arg("-pix_fmt")
            .arg("yuv420p")
            .arg(&asset)
            .status()
            .expect("synth ffmpeg spawn");
        assert!(status.success(), "synth ffmpeg exit: {status}");

        let out_dir = dir.path().join("thumbs");
        let result = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                generate_thumbnails(&asset, &out_dir, tokio_util::sync::CancellationToken::new())
                    .await
            });
        assert!(result.is_ok(), "generate_thumbnails failed: {result:?}");

        // 3s source @ fps=1 → 3 frames. Allow ±1 for vfr edge cases.
        let entries: Vec<_> = std::fs::read_dir(&out_dir)
            .unwrap()
            .filter_map(std::result::Result::ok)
            .filter(|e| e.file_name().to_string_lossy().starts_with("frame-"))
            .collect();
        assert!(
            (2..=4).contains(&entries.len()),
            "expected 2-4 frames, got {}: {:?}",
            entries.len(),
            entries
                .iter()
                .map(std::fs::DirEntry::file_name)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn parse_silence_ranges_pairs_start_and_end() {
        let stderr = "\
[silencedetect @ 0x111] silence_start: 1.234
[silencedetect @ 0x111] silence_end: 5.678 | silence_duration: 4.444
[silencedetect @ 0x111] silence_start: 10.0
[silencedetect @ 0x111] silence_end: 12.5 | silence_duration: 2.5
";
        let ranges = parse_silence_ranges(stderr);
        assert_eq!(ranges.len(), 2);
        assert!((ranges[0].start_s - 1.234).abs() < 1e-9);
        assert!((ranges[0].end_s - 5.678).abs() < 1e-9);
        assert!((ranges[1].start_s - 10.0).abs() < 1e-9);
        assert!((ranges[1].end_s - 12.5).abs() < 1e-9);
    }

    #[test]
    fn parse_silence_ranges_drops_unmatched_starts() {
        // Truncated stderr: a start without a matching end should be
        // skipped rather than crashing the parser.
        let stderr = "\
[silencedetect @ 0x111] silence_start: 1.0
[silencedetect @ 0x111] silence_end: 2.0 | silence_duration: 1.0
[silencedetect @ 0x111] silence_start: 5.0
";
        let ranges = parse_silence_ranges(stderr);
        assert_eq!(ranges.len(), 1);
    }

    #[test]
    fn parse_silence_ranges_empty_input_yields_empty_vec() {
        assert!(parse_silence_ranges("").is_empty());
        assert!(parse_silence_ranges("frame= 12 fps=24\n").is_empty());
    }

    #[test]
    fn parse_silence_ranges_skips_inverted_ranges() {
        // end <= start would mean the parser misread something —
        // drop rather than emit a degenerate range.
        let stderr = "\
[silencedetect @ 0x111] silence_start: 5.0
[silencedetect @ 0x111] silence_end: 3.0 | silence_duration: -2.0
";
        let ranges = parse_silence_ranges(stderr);
        assert!(ranges.is_empty());
    }

    #[test]
    fn generate_silences_rejects_invalid_threshold() {
        // Positive threshold makes no sense for dBFS (always negative).
        let result = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                generate_silences(
                    Path::new("/nonexistent.wav"),
                    0.0,
                    0.6,
                    tokio_util::sync::CancellationToken::new(),
                )
                .await
            });
        assert!(matches!(result, Err(FfmpegError::Io(_))));
    }

    #[test]
    fn generate_silences_rejects_zero_min_duration() {
        let result = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                generate_silences(
                    Path::new("/nonexistent.wav"),
                    -40.0,
                    0.0,
                    tokio_util::sync::CancellationToken::new(),
                )
                .await
            });
        assert!(matches!(result, Err(FfmpegError::Io(_))));
    }

    /// End-to-end: synthesize a 4s clip — 1s of tone, 2s of pure
    /// silence, 1s of tone — via lavfi `aevalsrc` and run
    /// `generate_silences`. The middle silence should land. Skipped
    /// when ffmpeg isn't on the box.
    #[test]
    fn generate_silences_against_synthesized_audio() {
        let Ok(_bin) = ffmpeg_path() else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let asset = dir.path().join("clip.wav");

        // aevalsrc: pure-silence regions are exactly what silencedetect
        // is built for. We splice tone + silence + tone via concat.
        let status = std::process::Command::new(ffmpeg_path().unwrap())
            .arg("-loglevel")
            .arg("error")
            .arg("-y")
            .arg("-f")
            .arg("lavfi")
            .arg("-i")
            .arg("sine=frequency=440:duration=1")
            .arg("-f")
            .arg("lavfi")
            .arg("-i")
            .arg("anullsrc=duration=2:r=44100")
            .arg("-f")
            .arg("lavfi")
            .arg("-i")
            .arg("sine=frequency=440:duration=1")
            .arg("-filter_complex")
            .arg("[0][1][2]concat=n=3:v=0:a=1[a]")
            .arg("-map")
            .arg("[a]")
            .arg(&asset)
            .status()
            .expect("synth ffmpeg spawn");
        assert!(status.success(), "synth wav exit: {status}");

        let ranges = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                generate_silences(
                    &asset,
                    -40.0,
                    0.6,
                    tokio_util::sync::CancellationToken::new(),
                )
                .await
            })
            .expect("generate_silences");

        // We expect exactly one silence range covering ~[1.0, 3.0].
        assert!(
            !ranges.is_empty(),
            "expected at least one silence range, got 0",
        );
        let r = ranges[0];
        // ffmpeg's silencedetect rounds to milliseconds; allow ±200ms
        // slack on each edge for the tone-fade margin.
        assert!(
            (0.8..=1.2).contains(&r.start_s),
            "silence_start {} out of expected [0.8, 1.2]",
            r.start_s,
        );
        assert!(
            (2.8..=3.2).contains(&r.end_s),
            "silence_end {} out of expected [2.8, 3.2]",
            r.end_s,
        );
    }

    #[test]
    fn parse_scene_scores_extracts_decimal_values() {
        let stderr = "\
[Parsed_metadata_2 @ 0x111] frame:0    pts:0       pts_time:0
[Parsed_metadata_2 @ 0x111] lavfi.scene_score=0.000000
[Parsed_metadata_2 @ 0x111] frame:1    pts:25      pts_time:1
[Parsed_metadata_2 @ 0x111] lavfi.scene_score=0.123456
[Parsed_metadata_2 @ 0x111] frame:2    pts:50      pts_time:2
[Parsed_metadata_2 @ 0x111] lavfi.scene_score=0.789012
";
        let signal = parse_scene_scores(stderr);
        assert_eq!(signal.len(), 3);
        assert!((signal[0] - 0.0).abs() < 1e-6);
        assert!((signal[1] - 0.123456).abs() < 1e-5);
        assert!((signal[2] - 0.789012).abs() < 1e-5);
    }

    #[test]
    fn parse_scene_scores_clamps_out_of_range_values() {
        // ffmpeg can technically emit values slightly above 1.0 in
        // edge cases; clamp on parse so downstream consumers don't
        // have to defend.
        let stderr = "\
[X] lavfi.scene_score=1.5
[X] lavfi.scene_score=-0.1
[X] lavfi.scene_score=0.5
";
        let signal = parse_scene_scores(stderr);
        assert_eq!(signal, vec![1.0, 0.0, 0.5]);
    }

    #[test]
    fn parse_scene_scores_skips_nan_and_unparseable_lines() {
        let stderr = "\
[X] frame=12 fps=24
[X] lavfi.scene_score=
[X] lavfi.scene_score=not-a-number
[X] lavfi.scene_score=NaN
[X] lavfi.scene_score=0.42
";
        let signal = parse_scene_scores(stderr);
        // Only the 0.42 line parses cleanly; NaN is rejected by the
        // is_finite() check.
        assert_eq!(signal, vec![0.42]);
    }

    #[test]
    fn parse_scene_scores_empty_input() {
        assert!(parse_scene_scores("").is_empty());
        assert!(parse_scene_scores("frame=0\nframe=1\n").is_empty());
    }

    #[test]
    fn parse_black_frame_ranges_pairs_start_end_duration_lines() {
        let stderr = "\
[blackdetect @ 0x7fa] black_start:0 black_end:1.24 black_duration:1.24
[blackdetect @ 0x7fa] black_start:4.5 black_end:6 black_duration:1.5
";

        let ranges = parse_black_frame_ranges(stderr);

        assert_eq!(
            ranges,
            vec![
                BlackFrameRange {
                    start_s: 0.0,
                    end_s: 1.24,
                    duration_s: 1.24,
                },
                BlackFrameRange {
                    start_s: 4.5,
                    end_s: 6.0,
                    duration_s: 1.5,
                },
            ]
        );
    }

    #[test]
    fn generate_black_frames_rejects_invalid_thresholds() {
        let result = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                generate_black_frames(
                    Path::new("/nonexistent.mp4"),
                    -1.0,
                    0.5,
                    tokio_util::sync::CancellationToken::new(),
                )
                .await
            });

        assert!(matches!(result, Err(FfmpegError::Io(_))));
    }

    /// End-to-end: synthesize a 3s testsrc clip with motion + run
    /// `generate_motion_signal`. Should produce ~3 samples (1 per
    /// second) with non-zero scene scores. Skipped without ffmpeg.
    #[test]
    fn generate_motion_signal_against_testsrc() {
        let Ok(_bin) = ffmpeg_path() else {
            return;
        };

        let dir = tempfile::tempdir().unwrap();
        let asset = dir.path().join("testsrc.mp4");

        // testsrc has rotating motion → non-zero scene scores.
        let status = std::process::Command::new(ffmpeg_path().unwrap())
            .arg("-loglevel")
            .arg("error")
            .arg("-y")
            .arg("-f")
            .arg("lavfi")
            .arg("-i")
            .arg("testsrc=duration=3:size=320x240:rate=30")
            .arg("-c:v")
            .arg("libx264")
            .arg("-pix_fmt")
            .arg("yuv420p")
            .arg(&asset)
            .status()
            .expect("synth ffmpeg spawn");
        assert!(status.success(), "synth mp4 exit: {status}");

        let signal = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                generate_motion_signal(&asset, tokio_util::sync::CancellationToken::new()).await
            })
            .expect("generate_motion_signal");

        // 3 second clip @ fps=1 → 2-3 samples (first second often
        // emits frame 0 with no predecessor; the math is jittery
        // depending on ffmpeg version). Just confirm we got
        // something nonzero.
        assert!(
            !signal.is_empty(),
            "expected at least one motion sample, got 0",
        );
        // testsrc moves visibly → at least one sample should be
        // > 0.0. Allow some leeway since the very first frame's
        // score is undefined.
        let max_score = signal.iter().cloned().fold(0.0_f32, f32::max);
        assert!(
            max_score > 0.0,
            "expected non-zero motion magnitude, got {signal:?}",
        );
    }
}
