//! `color_scopes` tool — compute histogram, waveform, RGB parade, and
//! vectorscope evidence from a single video frame.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::process::Command;

use crate::FunctionCallError;
use crate::tool_schema::Tool as ToolSchema;
use crate::tool::{ToolContext, ToolHandler, ToolInvocation, ToolOutput};

const DEFAULT_BINS: usize = 64;
const DEFAULT_MAX_WIDTH: usize = 320;

/// The `color_scopes` tool.
pub struct ColorScopesTool;

#[derive(Debug, Deserialize)]
struct ColorScopesArgs {
    /// Project-relative asset id, OR an absolute path under the project.
    asset: String,
    /// Time in seconds to sample.
    t_s: f64,
    /// Scope resolution per axis.
    #[serde(default)]
    bins: Option<usize>,
    /// Maximum extracted frame width before scope computation.
    #[serde(default)]
    max_width: Option<usize>,
}

/// RGB frame input for scope computation.
#[derive(Debug, Clone)]
pub struct ColorScopeInput {
    /// Frame width in pixels.
    pub width: usize,
    /// Frame height in pixels.
    pub height: usize,
    /// Number of scope bins per axis.
    pub bins: usize,
    /// Packed RGB bytes, row-major, three bytes per pixel.
    pub rgb: Vec<u8>,
}

/// Per-channel RGB histograms.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RgbHistogram {
    /// Red counts by intensity bin.
    pub red: Vec<u32>,
    /// Green counts by intensity bin.
    pub green: Vec<u32>,
    /// Blue counts by intensity bin.
    pub blue: Vec<u32>,
}

/// Per-channel RGB parade. First dimension is horizontal position bucket,
/// second dimension is intensity bucket.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RgbParade {
    /// Red counts by x/intensity bin.
    pub red: Vec<Vec<u32>>,
    /// Green counts by x/intensity bin.
    pub green: Vec<Vec<u32>>,
    /// Blue counts by x/intensity bin.
    pub blue: Vec<Vec<u32>>,
}

/// Complete color scope snapshot for one frame.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ColorScopeSnapshot {
    /// Sampled frame width in pixels.
    pub width: usize,
    /// Sampled frame height in pixels.
    pub height: usize,
    /// Number of bins per scope axis.
    pub bins: usize,
    /// Number of pixels sampled.
    pub sample_count: u32,
    /// Luma counts by intensity bin.
    pub luma_histogram: Vec<u32>,
    /// RGB histograms by intensity bin.
    pub rgb_histogram: RgbHistogram,
    /// Luma waveform by x/intensity bin.
    pub waveform: Vec<Vec<u32>>,
    /// RGB parade by x/intensity bin.
    pub rgb_parade: RgbParade,
    /// Cb/Cr vectorscope by vertical/horizontal bin.
    pub vectorscope: Vec<Vec<u32>>,
}

#[async_trait]
impl ToolHandler for ColorScopesTool {
    fn name(&self) -> &'static str {
        "color_scopes"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "color_scopes".into(),
            description: DESCRIPTION.into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "asset": {
                        "type": "string",
                        "description": "Project-relative asset path, e.g. \"raw/ep-014.mp4\"."
                    },
                    "t_s": {
                        "type": "number",
                        "minimum": 0.0,
                        "description": "Time in seconds to sample."
                    },
                    "bins": {
                        "type": "integer",
                        "minimum": 8,
                        "maximum": 256,
                        "description": "Scope resolution per axis. Defaults to 64."
                    },
                    "max_width": {
                        "type": "integer",
                        "minimum": 32,
                        "maximum": 1920,
                        "description": "Maximum extracted frame width before scope computation. Defaults to 320."
                    }
                },
                "required": ["asset", "t_s"]
            }),
            ..Default::default()
        }
    }

    fn is_mutating(&self, _invocation: &ToolInvocation) -> bool {
        false
    }

    async fn handle(
        &self,
        invocation: ToolInvocation,
        ctx: ToolContext,
    ) -> Result<ToolOutput, FunctionCallError> {
        let args: ColorScopesArgs = serde_json::from_value(invocation.args).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "color_scopes: invalid args ({e}). Required: {{ \"asset\": <str>, \"t_s\": <number> }}."
            ))
        })?;
        if !args.t_s.is_finite() || args.t_s < 0.0 {
            return Err(FunctionCallError::RespondToModel(format!(
                "color_scopes: t_s ({}) must be finite and >= 0",
                args.t_s
            )));
        }
        let bins = args.bins.unwrap_or(DEFAULT_BINS);
        if !(8..=256).contains(&bins) {
            return Err(FunctionCallError::RespondToModel(format!(
                "color_scopes: bins ({bins}) must be between 8 and 256"
            )));
        }
        let max_width = args.max_width.unwrap_or(DEFAULT_MAX_WIDTH);
        if !(32..=1920).contains(&max_width) {
            return Err(FunctionCallError::RespondToModel(format!(
                "color_scopes: max_width ({max_width}) must be between 32 and 1920"
            )));
        }

        let asset_path = resolve_asset_path(&ctx.project_root, &args.asset)?;
        let frame = extract_rgb_frame(&asset_path, args.t_s, bins, max_width).await?;
        let scopes = compute_color_scopes(&frame).map_err(|e| {
            FunctionCallError::RespondToModel(format!("color_scopes: computation failed: {e}"))
        })?;
        let body = serde_json::json!({
            "asset": args.asset,
            "t_s": args.t_s,
            "scopes": scopes,
        });
        let json = serde_json::to_string_pretty(&body).map_err(|e| {
            FunctionCallError::RespondToModel(format!("color_scopes: serialize failed: {e}"))
        })?;
        Ok(ToolOutput::text(json))
    }
}

/// Default scope resolution per axis when the caller doesn't override.
pub const DEFAULT_SCOPE_BINS: usize = DEFAULT_BINS;
/// Default extracted-frame width cap when the caller doesn't override.
pub const DEFAULT_SCOPE_MAX_WIDTH: usize = DEFAULT_MAX_WIDTH;

/// Extract one frame from a local video asset at `t_s` and compute
/// the full scope snapshot for it. Same pipeline the `color_scopes`
/// agent tool uses internally — exposed so the desktop UI can drive
/// scope rendering without going through the tool-handler indirection.
///
/// `asset_path` must be an absolute path that exists on disk. `bins`
/// and `max_width` mirror the tool's validation ranges; values
/// outside the supported range return `Err` rather than clamping
/// silently. Returns the scope snapshot on success or a stringified
/// failure (ffmpeg/ffprobe error, dimension mismatch, etc.).
pub async fn extract_and_compute_color_scopes(
    asset_path: &Path,
    t_s: f64,
    bins: usize,
    max_width: usize,
) -> Result<ColorScopeSnapshot, String> {
    if !t_s.is_finite() || t_s < 0.0 {
        return Err(format!("t_s ({t_s}) must be finite and >= 0"));
    }
    if !(8..=256).contains(&bins) {
        return Err(format!("bins ({bins}) must be between 8 and 256"));
    }
    if !(32..=1920).contains(&max_width) {
        return Err(format!(
            "max_width ({max_width}) must be between 32 and 1920"
        ));
    }
    let frame = extract_rgb_frame(asset_path, t_s, bins, max_width)
        .await
        .map_err(|e| match e {
            FunctionCallError::RespondToModel(msg) => msg,
            other => format!("{other:?}"),
        })?;
    compute_color_scopes(&frame)
}

/// Compute color scopes from a packed RGB frame.
pub fn compute_color_scopes(input: &ColorScopeInput) -> Result<ColorScopeSnapshot, String> {
    if input.width == 0 || input.height == 0 {
        return Err("width and height must be positive".into());
    }
    if input.bins == 0 {
        return Err("bins must be positive".into());
    }
    let pixel_count = input
        .width
        .checked_mul(input.height)
        .ok_or_else(|| "frame dimensions overflow".to_string())?;
    let expected_len = pixel_count
        .checked_mul(3)
        .ok_or_else(|| "RGB byte count overflow".to_string())?;
    if input.rgb.len() != expected_len {
        return Err(format!(
            "expected {expected_len} RGB bytes for {}x{} frame, got {}",
            input.width,
            input.height,
            input.rgb.len()
        ));
    }

    let bins = input.bins;
    let mut luma_histogram = vec![0_u32; bins];
    let mut rgb_histogram = RgbHistogram {
        red: vec![0; bins],
        green: vec![0; bins],
        blue: vec![0; bins],
    };
    let mut waveform = grid(bins);
    let mut rgb_parade = RgbParade {
        red: grid(bins),
        green: grid(bins),
        blue: grid(bins),
    };
    let mut vectorscope = grid(bins);

    for (idx, px) in input.rgb.chunks_exact(3).enumerate() {
        let x = idx % input.width;
        let r = px[0];
        let g = px[1];
        let b = px[2];
        let r_bin = channel_bin(r, bins);
        let g_bin = channel_bin(g, bins);
        let b_bin = channel_bin(b, bins);
        let luma = luma_u8(r, g, b);
        let luma_bin = channel_bin(luma, bins);
        let x_bin = coordinate_bin(x, input.width, bins);

        luma_histogram[luma_bin] += 1;
        rgb_histogram.red[r_bin] += 1;
        rgb_histogram.green[g_bin] += 1;
        rgb_histogram.blue[b_bin] += 1;
        waveform[x_bin][invert_bin(luma_bin, bins)] += 1;
        rgb_parade.red[x_bin][invert_bin(r_bin, bins)] += 1;
        rgb_parade.green[x_bin][invert_bin(g_bin, bins)] += 1;
        rgb_parade.blue[x_bin][invert_bin(b_bin, bins)] += 1;

        let (cb, cr) = chroma_coordinates(r, g, b);
        let cb_bin = float_bin(cb, bins);
        let cr_bin = invert_bin(float_bin(cr, bins), bins);
        vectorscope[cr_bin][cb_bin] += 1;
    }

    let sample_count = u32::try_from(pixel_count).map_err(|_| {
        format!("sample count {pixel_count} exceeds maximum serializable scope count")
    })?;
    Ok(ColorScopeSnapshot {
        width: input.width,
        height: input.height,
        bins,
        sample_count,
        luma_histogram,
        rgb_histogram,
        waveform,
        rgb_parade,
        vectorscope,
    })
}

fn grid(bins: usize) -> Vec<Vec<u32>> {
    vec![vec![0; bins]; bins]
}

fn channel_bin(value: u8, bins: usize) -> usize {
    ((usize::from(value) * bins) / 256).min(bins - 1)
}

fn coordinate_bin(value: usize, extent: usize, bins: usize) -> usize {
    ((value * bins) / extent).min(bins - 1)
}

fn float_bin(value: f64, bins: usize) -> usize {
    (((value.clamp(0.0, 255.0) * bins as f64) / 256.0).floor() as usize).min(bins - 1)
}

fn invert_bin(bin: usize, bins: usize) -> usize {
    bins - 1 - bin
}

fn luma_u8(r: u8, g: u8, b: u8) -> u8 {
    (0.2126 * f64::from(r) + 0.7152 * f64::from(g) + 0.0722 * f64::from(b))
        .round()
        .clamp(0.0, 255.0) as u8
}

fn chroma_coordinates(r: u8, g: u8, b: u8) -> (f64, f64) {
    let r = f64::from(r);
    let g = f64::from(g);
    let b = f64::from(b);
    let cb = 128.0 - 0.114_572 * r - 0.385_428 * g + 0.5 * b;
    let cr = 128.0 + 0.5 * r - 0.454_153 * g - 0.045_847 * b;
    (cb, cr)
}

async fn extract_rgb_frame(
    asset_path: &Path,
    t_s: f64,
    bins: usize,
    max_width: usize,
) -> Result<ColorScopeInput, FunctionCallError> {
    let (source_width, source_height) = probe_video_dimensions(asset_path).await?;
    let (width, height) = scaled_dimensions(source_width, source_height, max_width);
    let ffmpeg = awidat_render::ffmpeg_path().map_err(|e| {
        FunctionCallError::RespondToModel(format!("color_scopes: ffmpeg unavailable: {e}"))
    })?;
    let output = Command::new(ffmpeg)
        .arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-ss")
        .arg(format!("{t_s:.6}"))
        .arg("-i")
        .arg(asset_path)
        .arg("-frames:v")
        .arg("1")
        .arg("-vf")
        .arg(format!("scale={width}:{height}"))
        .arg("-pix_fmt")
        .arg("rgb24")
        .arg("-f")
        .arg("rawvideo")
        .arg("-")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "color_scopes: failed to spawn ffmpeg for {}: {e}",
                asset_path.display()
            ))
        })?;
    if !output.status.success() {
        return Err(FunctionCallError::RespondToModel(format!(
            "color_scopes: ffmpeg failed for {}: {}",
            asset_path.display(),
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok(ColorScopeInput {
        width,
        height,
        bins,
        rgb: output.stdout,
    })
}

async fn probe_video_dimensions(asset_path: &Path) -> Result<(usize, usize), FunctionCallError> {
    let ffprobe = awidat_render::ffprobe_path().map_err(|e| {
        FunctionCallError::RespondToModel(format!("color_scopes: ffprobe unavailable: {e}"))
    })?;
    let output = Command::new(ffprobe)
        .arg("-v")
        .arg("error")
        .arg("-select_streams")
        .arg("v:0")
        .arg("-show_entries")
        .arg("stream=width,height")
        .arg("-of")
        .arg("csv=p=0:s=x")
        .arg(asset_path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "color_scopes: failed to spawn ffprobe for {}: {e}",
                asset_path.display()
            ))
        })?;
    if !output.status.success() {
        return Err(FunctionCallError::RespondToModel(format!(
            "color_scopes: ffprobe failed for {}: {}",
            asset_path.display(),
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout.trim();
    let (width, height) = line.split_once('x').ok_or_else(|| {
        FunctionCallError::RespondToModel(format!(
            "color_scopes: ffprobe did not return WIDTHxHEIGHT for {}",
            asset_path.display()
        ))
    })?;
    let width = width.parse::<usize>().map_err(|e| {
        FunctionCallError::RespondToModel(format!("color_scopes: invalid probed width: {e}"))
    })?;
    let height = height.parse::<usize>().map_err(|e| {
        FunctionCallError::RespondToModel(format!("color_scopes: invalid probed height: {e}"))
    })?;
    if width == 0 || height == 0 {
        return Err(FunctionCallError::RespondToModel(
            "color_scopes: probed video dimensions must be positive".into(),
        ));
    }
    Ok((width, height))
}

fn scaled_dimensions(width: usize, height: usize, max_width: usize) -> (usize, usize) {
    if width <= max_width {
        return (width, height);
    }
    let scaled_height = ((height as f64 * max_width as f64) / width as f64)
        .round()
        .max(1.0) as usize;
    (max_width, scaled_height)
}

fn resolve_asset_path(project_root: &Path, asset: &str) -> Result<PathBuf, FunctionCallError> {
    let p = Path::new(asset);
    if asset.contains("..") {
        return Err(FunctionCallError::RespondToModel(format!(
            "color_scopes: asset '{asset}' must not contain '..' segments"
        )));
    }
    if p.is_absolute() {
        let canonical_root = project_root
            .canonicalize()
            .unwrap_or(project_root.to_path_buf());
        let canonical_asset = p.canonicalize().unwrap_or(p.to_path_buf());
        if !canonical_asset.starts_with(&canonical_root) {
            return Err(FunctionCallError::RespondToModel(format!(
                "color_scopes: absolute asset path '{asset}' must live under the project root"
            )));
        }
        Ok(canonical_asset)
    } else {
        Ok(project_root.join(p))
    }
}

const DESCRIPTION: &str = "\
Computes color scope evidence from one video frame: luma histogram, \
RGB histogram, luma waveform, RGB parade, and Cb/Cr vectorscope. Use \
this before or after applying color effects when you need objective \
frame-level exposure, channel balance, and chroma distribution evidence.";
