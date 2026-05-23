//! Frame-pixel caption rendered-output scorer.
//!
//! Parses Dialogue lines from ASS sidecars, computes per-event bounding boxes
//! against PlayRes + style margins + a safe-area profile, asks a
//! [`CaptionFrameSampler`] for a grayscale frame at the event midpoint, then
//! decides safe-area and occlusion outcomes. Production wiring lives in
//! [`crate::tools::verify_render`] which calls
//! [`score_caption_rendered_output`] before its sync gate-builder and
//! injects measured evidence into the render manifest's metadata view.

use std::path::{Path, PathBuf};

/// Failure modes that the scorer surfaces back to the caller.
#[derive(Debug, thiserror::Error)]
pub enum ScorerError {
    /// Frame sampling is impossible in this environment (e.g. ffmpeg missing).
    #[error("frame sampler unavailable: {0}")]
    SamplerUnavailable(&'static str),
    /// Render output file is missing or zero-sized.
    #[error("render output missing or empty")]
    RenderOutputMissing,
    /// Every sidecar failed to parse — gate must fall back to libass layout.
    #[error("all sidecars failed to parse")]
    SidecarParseFailed,
    /// Underlying IO error.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// One decoded grayscale frame with row-major 8-bit luma.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedFrame {
    /// Pixel width.
    pub width: u32,
    /// Pixel height.
    pub height: u32,
    /// Row-major luma plane, `width * height` bytes.
    pub luma: Vec<u8>,
}

/// Per-event scoring detail. Surfaced so the verify_render gate can log
/// individual event outcomes alongside the aggregate counters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptionEventFinding {
    /// Dialogue start in milliseconds.
    pub start_ms: u64,
    /// Dialogue end in milliseconds.
    pub end_ms: u64,
    /// Probe timestamp used for sampling (midpoint).
    pub probe_ms: u64,
    /// Bounding box `(x, y, width, height)` in output pixels.
    pub bbox: (u32, u32, u32, u32),
    /// True iff the bbox sits inside the safe-area inset.
    pub safe_area_pass: bool,
    /// True iff the inside-vs-halo luma variance check failed.
    pub occlusion_fail: bool,
    /// Static reason tag (`scored`, `sample_failed`, ...).
    pub reason: &'static str,
}

/// Aggregate per-render evidence consumed by the verify_render gate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptionRenderedOutputEvidence {
    /// Number of events that produced a measured outcome.
    pub probe_count: usize,
    /// Number of events whose bbox satisfied the safe-area inset.
    pub safe_area_pass_count: usize,
    /// Number of events that failed the occlusion check.
    pub occlusion_fail_count: usize,
    /// Per-event detail, in source order.
    pub per_event_findings: Vec<CaptionEventFinding>,
    /// Optional non-fatal flag forwarded to the gate (e.g. partial parse).
    pub fallback_reason: Option<&'static str>,
}

/// Abstraction over single-frame sampling. The production implementation
/// shells out to ffmpeg; tests inject a deterministic in-memory sampler.
#[async_trait::async_trait]
pub trait CaptionFrameSampler: Send + Sync {
    /// Sample a grayscale frame at `t_s` (seconds).
    async fn sample(&self, t_s: f64) -> Result<DecodedFrame, ScorerError>;
}

/// Score a render output's caption events against the supplied layout
/// sidecars. Each Dialogue line is sampled once at its visible midpoint.
pub async fn score_caption_rendered_output(
    render_output: &Path,
    layout_sidecars: &[PathBuf],
    video_dims: (u32, u32),
    safe_area_profile: &str,
    sampler: &dyn CaptionFrameSampler,
) -> Result<CaptionRenderedOutputEvidence, ScorerError> {
    let render_meta =
        std::fs::metadata(render_output).map_err(|_| ScorerError::RenderOutputMissing)?;
    if render_meta.len() == 0 {
        return Err(ScorerError::RenderOutputMissing);
    }

    let safe_area = safe_area_inset(safe_area_profile, video_dims);

    let mut parsed_any = false;
    let mut had_parse_failures = false;
    let mut events: Vec<DialogueEvent> = Vec::new();
    for path in layout_sidecars {
        let Ok(contents) = std::fs::read_to_string(path) else {
            had_parse_failures = true;
            continue;
        };
        let (file_events, file_partial, file_ok) = parse_sidecar_events(&contents, video_dims);
        if file_ok {
            parsed_any = true;
        }
        if file_partial {
            had_parse_failures = true;
        }
        events.extend(file_events);
    }

    if !parsed_any && events.is_empty() && !had_parse_failures {
        return Ok(CaptionRenderedOutputEvidence {
            probe_count: 0,
            safe_area_pass_count: 0,
            occlusion_fail_count: 0,
            per_event_findings: vec![],
            fallback_reason: Some("no_caption_events"),
        });
    }
    if !parsed_any {
        return Err(ScorerError::SidecarParseFailed);
    }
    if events.is_empty() {
        return Ok(CaptionRenderedOutputEvidence {
            probe_count: 0,
            safe_area_pass_count: 0,
            occlusion_fail_count: 0,
            per_event_findings: vec![],
            fallback_reason: Some("no_caption_events"),
        });
    }

    let mut findings = Vec::with_capacity(events.len());
    let mut probe_count = 0usize;
    let mut safe_area_pass_count = 0usize;
    let mut occlusion_fail_count = 0usize;

    for event in &events {
        let probe_ms = (event.start_ms + event.end_ms) / 2;
        let t_s = (probe_ms as f64) / 1000.0;
        let frame = match sampler.sample(t_s).await {
            Ok(f) => f,
            Err(_) => {
                findings.push(CaptionEventFinding {
                    start_ms: event.start_ms,
                    end_ms: event.end_ms,
                    probe_ms,
                    bbox: event.bbox,
                    safe_area_pass: false,
                    occlusion_fail: false,
                    reason: "sample_failed",
                });
                continue;
            }
        };

        let safe_pass = bbox_within_inset(event.bbox, video_dims, safe_area);
        let inside_var = luma_variance_in_rect(&frame, event.bbox);
        let halo_var = luma_variance_in_halo(&frame, event.bbox, 8);
        let occlusion_fail = inside_var < halo_var + 4.0;

        probe_count += 1;
        if safe_pass {
            safe_area_pass_count += 1;
        }
        if occlusion_fail {
            occlusion_fail_count += 1;
        }

        findings.push(CaptionEventFinding {
            start_ms: event.start_ms,
            end_ms: event.end_ms,
            probe_ms,
            bbox: event.bbox,
            safe_area_pass: safe_pass,
            occlusion_fail,
            reason: "scored",
        });
    }

    let fallback_reason = if had_parse_failures {
        Some("partial_scorer_evidence")
    } else {
        None
    };

    Ok(CaptionRenderedOutputEvidence {
        probe_count,
        safe_area_pass_count,
        occlusion_fail_count,
        per_event_findings: findings,
        fallback_reason,
    })
}

struct DialogueEvent {
    start_ms: u64,
    end_ms: u64,
    bbox: (u32, u32, u32, u32),
}

#[derive(Debug, Default, Clone)]
struct StyleRecord {
    fontsize: u32,
    alignment: u8,
    margin_l: u32,
    margin_r: u32,
    margin_v: u32,
}

fn safe_area_inset(profile: &str, dims: (u32, u32)) -> (u32, u32, u32, u32) {
    let (w, h) = dims;
    match profile {
        "mobile" => (w * 5 / 100, h * 10 / 100, w * 5 / 100, h * 10 / 100),
        _ => (w * 5 / 100, h * 5 / 100, w * 5 / 100, h * 5 / 100),
    }
}

fn bbox_within_inset(
    bbox: (u32, u32, u32, u32),
    dims: (u32, u32),
    inset: (u32, u32, u32, u32),
) -> bool {
    let (x, y, w, h) = bbox;
    let (l, t, r, b) = inset;
    let (fw, fh) = dims;
    x >= l && y >= t && x + w <= fw.saturating_sub(r) && y + h <= fh.saturating_sub(b)
}

fn luma_variance_in_rect(frame: &DecodedFrame, rect: (u32, u32, u32, u32)) -> f64 {
    let (x, y, w, h) = clamp_rect(rect, (frame.width, frame.height));
    if w == 0 || h == 0 {
        return 0.0;
    }
    let mut sum = 0u64;
    let mut sum_sq = 0u64;
    let mut count = 0u64;
    for yy in y..y + h {
        for xx in x..x + w {
            let v = frame.luma[(yy * frame.width + xx) as usize] as u64;
            sum += v;
            sum_sq += v * v;
            count += 1;
        }
    }
    if count == 0 {
        return 0.0;
    }
    let mean = sum as f64 / count as f64;
    (sum_sq as f64 / count as f64) - mean * mean
}

fn luma_variance_in_halo(frame: &DecodedFrame, rect: (u32, u32, u32, u32), halo: u32) -> f64 {
    let (x, y, w, h) = rect;
    let dims = (frame.width, frame.height);
    let outer = (
        x.saturating_sub(halo),
        y.saturating_sub(halo),
        w + 2 * halo,
        h + 2 * halo,
    );
    let (ox, oy, ow, oh) = clamp_rect(outer, dims);
    if ow == 0 || oh == 0 {
        return 0.0;
    }
    let mut sum = 0u64;
    let mut sum_sq = 0u64;
    let mut count = 0u64;
    for yy in oy..oy + oh {
        for xx in ox..ox + ow {
            let inside = xx >= x && xx < x + w && yy >= y && yy < y + h;
            if inside {
                continue;
            }
            let v = frame.luma[(yy * frame.width + xx) as usize] as u64;
            sum += v;
            sum_sq += v * v;
            count += 1;
        }
    }
    if count == 0 {
        return 0.0;
    }
    let mean = sum as f64 / count as f64;
    (sum_sq as f64 / count as f64) - mean * mean
}

fn clamp_rect(rect: (u32, u32, u32, u32), dims: (u32, u32)) -> (u32, u32, u32, u32) {
    let (x, y, w, h) = rect;
    let (fw, fh) = dims;
    let x = x.min(fw);
    let y = y.min(fh);
    let w = w.min(fw.saturating_sub(x));
    let h = h.min(fh.saturating_sub(y));
    (x, y, w, h)
}

fn parse_sidecar_events(
    contents: &str,
    video_dims: (u32, u32),
) -> (Vec<DialogueEvent>, bool, bool) {
    let playres = parse_playres(contents).unwrap_or(video_dims);
    let styles = parse_styles(contents);
    let mut events: Vec<DialogueEvent> = Vec::new();
    let mut any_bad = false;
    let mut any_good = false;
    for line in contents.lines() {
        let trimmed = line.trim_start();
        if !trimmed.starts_with("Dialogue:") {
            continue;
        }
        let rest = trimmed.trim_start_matches("Dialogue:").trim_start();
        let fields: Vec<&str> = rest.splitn(10, ',').collect();
        if fields.len() < 10 {
            any_bad = true;
            continue;
        }
        let (start_ms, end_ms) = match (
            parse_ass_time_ms(fields[1].trim()),
            parse_ass_time_ms(fields[2].trim()),
        ) {
            (Some(a), Some(b)) if b > a => (a, b),
            _ => {
                any_bad = true;
                continue;
            }
        };
        let style_name = fields[3].trim();
        let style = styles.get(style_name).cloned().unwrap_or_default();
        let margin_l_override: u32 = fields[5].trim().parse().unwrap_or(0);
        let margin_r_override: u32 = fields[6].trim().parse().unwrap_or(0);
        let margin_v_override: u32 = fields[7].trim().parse().unwrap_or(0);
        let margin_l = if margin_l_override > 0 {
            margin_l_override
        } else {
            style.margin_l
        };
        let margin_r = if margin_r_override > 0 {
            margin_r_override
        } else {
            style.margin_r
        };
        let margin_v = if margin_v_override > 0 {
            margin_v_override
        } else {
            style.margin_v
        };
        let text = fields[9];
        let line_count = 1 + text.matches("\\N").count() as u32;

        let bbox = compute_bbox(
            playres, video_dims, &style, margin_l, margin_r, margin_v, line_count,
        );
        events.push(DialogueEvent {
            start_ms,
            end_ms,
            bbox,
        });
        any_good = true;
    }
    (events, any_bad && any_good, any_good)
}

fn compute_bbox(
    playres: (u32, u32),
    video_dims: (u32, u32),
    style: &StyleRecord,
    margin_l: u32,
    margin_r: u32,
    margin_v: u32,
    line_count: u32,
) -> (u32, u32, u32, u32) {
    let (pw, ph) = playres;
    let (fw, fh) = video_dims;
    let sx = fw as f64 / pw as f64;
    let sy = fh as f64 / ph as f64;
    let line_height = ((style.fontsize as f64) * 1.2 * sy).round() as u32;
    let height = line_height.saturating_mul(line_count);
    let margin_l_px = ((margin_l as f64) * sx).round() as u32;
    let margin_r_px = ((margin_r as f64) * sx).round() as u32;
    let width = fw.saturating_sub(margin_l_px).saturating_sub(margin_r_px);
    let x = margin_l_px;
    let margin_v_px = ((margin_v as f64) * sy).round() as u32;
    let y = match style.alignment {
        7..=9 => margin_v_px,
        4..=6 => fh.saturating_sub(height) / 2,
        _ => fh.saturating_sub(height).saturating_sub(margin_v_px),
    };
    (x, y, width, height)
}

fn parse_playres(contents: &str) -> Option<(u32, u32)> {
    let mut x = None;
    let mut y = None;
    for line in contents.lines() {
        if let Some(rest) = line.strip_prefix("PlayResX:") {
            x = rest.trim().parse().ok();
        }
        if let Some(rest) = line.strip_prefix("PlayResY:") {
            y = rest.trim().parse().ok();
        }
    }
    match (x, y) {
        (Some(a), Some(b)) => Some((a, b)),
        _ => None,
    }
}

fn parse_styles(contents: &str) -> std::collections::HashMap<String, StyleRecord> {
    let mut out = std::collections::HashMap::new();
    for line in contents.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("Style:") {
            let fields: Vec<&str> = rest.split(',').map(str::trim).collect();
            if fields.len() < 23 {
                continue;
            }
            let name = fields[0].to_string();
            let fontsize = fields[2].parse().unwrap_or(0);
            let alignment = fields[18].parse().unwrap_or(2);
            let margin_l = fields[19].parse().unwrap_or(0);
            let margin_r = fields[20].parse().unwrap_or(0);
            let margin_v = fields[21].parse().unwrap_or(0);
            out.insert(
                name,
                StyleRecord {
                    fontsize,
                    alignment,
                    margin_l,
                    margin_r,
                    margin_v,
                },
            );
        }
    }
    out
}

fn parse_ass_time_ms(s: &str) -> Option<u64> {
    let mut parts = s.split(':');
    let h: u64 = parts.next()?.parse().ok()?;
    let m: u64 = parts.next()?.parse().ok()?;
    let sec = parts.next()?;
    let mut sec_parts = sec.split('.');
    let s_whole: u64 = sec_parts.next()?.parse().ok()?;
    let cs: u64 = sec_parts.next().unwrap_or("0").parse().ok()?;
    Some(((h * 3600 + m * 60 + s_whole) * 1000) + cs * 10)
}

/// Production [`CaptionFrameSampler`] backed by ffmpeg raw-grayscale frame
/// extraction. Each `sample` call shells out to `ffmpeg` via
/// [`awidat_render::extract_frame_raw_gray`].
pub struct FfmpegFrameSampler {
    render_output: PathBuf,
}

impl FfmpegFrameSampler {
    /// Build a sampler for the supplied render output.
    pub fn new(render_output: PathBuf) -> Self {
        Self { render_output }
    }
}

#[async_trait::async_trait]
impl CaptionFrameSampler for FfmpegFrameSampler {
    async fn sample(&self, t_s: f64) -> Result<DecodedFrame, ScorerError> {
        let (width, height, luma) =
            awidat_render::extract_frame_raw_gray(&self.render_output, t_s)
                .await
                .map_err(|_| ScorerError::SamplerUnavailable("ffmpeg_extract_failed"))?;
        Ok(DecodedFrame {
            width,
            height,
            luma,
        })
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    //! Test-only sampler implementations. Kept under `pub(crate)` so the
    //! verify_render integration tests can also drive the scorer with a
    //! deterministic frame source.

    use std::collections::HashMap;
    use std::sync::Mutex;

    use super::{CaptionFrameSampler, DecodedFrame, ScorerError};

    /// Stores `(t_ms / 10, DecodedFrame)` pairs and returns the closest one
    /// (±20 ms) on lookup. Probes round to 10 ms buckets to absorb the
    /// rounding implicit in `score_caption_rendered_output`'s midpoint math.
    pub struct InMemoryFrameSampler {
        frames: Mutex<HashMap<u64, DecodedFrame>>,
    }

    impl Default for InMemoryFrameSampler {
        fn default() -> Self {
            Self::new()
        }
    }

    impl InMemoryFrameSampler {
        pub fn new() -> Self {
            Self {
                frames: Mutex::new(HashMap::new()),
            }
        }

        pub fn insert(&self, t_s: f64, frame: DecodedFrame) {
            let key = (t_s * 100.0).round() as u64;
            self.frames.lock().unwrap().insert(key, frame);
        }
    }

    #[async_trait::async_trait]
    impl CaptionFrameSampler for InMemoryFrameSampler {
        async fn sample(&self, t_s: f64) -> Result<DecodedFrame, ScorerError> {
            let key = (t_s * 100.0).round() as u64;
            let guard = self.frames.lock().unwrap();
            for offset in 0..=2u64 {
                if let Some(frame) = guard.get(&(key + offset)) {
                    return Ok(frame.clone());
                }
                if offset > 0
                    && let Some(frame) = guard.get(&key.saturating_sub(offset))
                {
                    return Ok(frame.clone());
                }
            }
            Err(ScorerError::SamplerUnavailable("no_frame_for_timestamp"))
        }
    }

    /// Sampler that always reports `SamplerUnavailable`. Used by verify_render
    /// integration tests to exercise the libass-layout fallback path.
    #[allow(dead_code)] // wired up by the verify_render integration tests in a later slice step
    pub struct AlwaysUnavailableFrameSampler;

    #[async_trait::async_trait]
    impl CaptionFrameSampler for AlwaysUnavailableFrameSampler {
        async fn sample(&self, _t_s: f64) -> Result<DecodedFrame, ScorerError> {
            Err(ScorerError::SamplerUnavailable("test_unavailable"))
        }
    }

    /// Helper: build a frame whose only high-variance region is the supplied
    /// bbox (flat luma everywhere else). Models a caption rendered on a
    /// uniform background so the occlusion heuristic resolves to "visible".
    pub fn caption_on_flat_background_frame(
        width: u32,
        height: u32,
        bbox: (u32, u32, u32, u32),
    ) -> DecodedFrame {
        let mut luma = vec![80u8; (width * height) as usize];
        let (bx, by, bw, bh) = bbox;
        for y in by..(by + bh).min(height) {
            for x in bx..(bx + bw).min(width) {
                let v = if (x / 2 + y / 2) % 2 == 0 { 240 } else { 16 };
                luma[(y * width + x) as usize] = v;
            }
        }
        DecodedFrame {
            width,
            height,
            luma,
        }
    }

    /// Helper: build a constant-luma frame so the occlusion heuristic
    /// resolves to "captions masked".
    pub fn flat_frame(width: u32, height: u32, value: u8) -> DecodedFrame {
        DecodedFrame {
            width,
            height,
            luma: vec![value; (width * height) as usize],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::{InMemoryFrameSampler, caption_on_flat_background_frame, flat_frame};
    use super::*;

    fn sidecar_with_event(start_s: f64, end_s: f64, alignment: u8, margin_v: u32) -> String {
        format!(
            "[Script Info]\nPlayResX: 1920\nPlayResY: 1080\n\n\
[V4+ Styles]\nFormat: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\n\
Style: Default,Arial,40,&H00FFFFFF,&H000000FF,&H00000000,&H00000000,0,0,0,0,100,100,0,0,1,2,2,{alignment},100,100,{margin_v},1\n\n\
[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n\
Dialogue: 0,{a},{b},Default,,0,0,0,,hello world\n",
            a = ass_time(start_s),
            b = ass_time(end_s),
        )
    }

    fn ass_time(t: f64) -> String {
        let h = (t / 3600.0).floor() as u32;
        let m = ((t / 60.0) % 60.0).floor() as u32;
        let s = t % 60.0;
        format!("{h}:{m:02}:{s:05.2}")
    }

    fn write_tmp_sidecar(body: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("captions.ass");
        std::fs::write(&path, body).unwrap();
        (dir, path)
    }

    fn write_render_output(dir: &tempfile::TempDir) -> PathBuf {
        let path = dir.path().join("out.mp4");
        std::fs::write(&path, b"stub").unwrap();
        path
    }

    // Bounding boxes for the synthetic style (PlayRes 1920x1080, fontsize 40,
    // margin_l=margin_r=100, single line). These mirror what `compute_bbox`
    // produces internally so the frame fixtures can place high-variance
    // content exactly where the scorer will look.
    const PASS_BBOX: (u32, u32, u32, u32) = (100, 952, 1720, 48);
    const BOTTOM_EDGE_BBOX: (u32, u32, u32, u32) = (100, 1032, 1720, 48);

    #[tokio::test]
    async fn scorer_passes_when_event_within_safe_area_and_variance_high() {
        let (_tmp, sidecar) = write_tmp_sidecar(&sidecar_with_event(0.0, 2.0, 2, 80));
        let render_dir = tempfile::tempdir().unwrap();
        let render_output = write_render_output(&render_dir);
        let sampler = InMemoryFrameSampler::new();
        sampler.insert(1.0, caption_on_flat_background_frame(1920, 1080, PASS_BBOX));
        let result = score_caption_rendered_output(
            &render_output,
            &[sidecar],
            (1920, 1080),
            "default",
            &sampler,
        )
        .await
        .unwrap();
        assert_eq!(result.probe_count, 1);
        assert_eq!(result.safe_area_pass_count, 1);
        assert_eq!(result.occlusion_fail_count, 0);
        assert!(result.fallback_reason.is_none());
    }

    #[tokio::test]
    async fn scorer_fails_safe_area_when_event_outside_margin() {
        let (_tmp, sidecar) = write_tmp_sidecar(&sidecar_with_event(0.0, 2.0, 2, 0));
        let render_dir = tempfile::tempdir().unwrap();
        let render_output = write_render_output(&render_dir);
        let sampler = InMemoryFrameSampler::new();
        sampler.insert(
            1.0,
            caption_on_flat_background_frame(1920, 1080, BOTTOM_EDGE_BBOX),
        );
        let result = score_caption_rendered_output(
            &render_output,
            &[sidecar],
            (1920, 1080),
            "default",
            &sampler,
        )
        .await
        .unwrap();
        assert_eq!(result.probe_count, 1);
        assert_eq!(result.safe_area_pass_count, 0);
    }

    #[tokio::test]
    async fn scorer_fails_occlusion_when_inside_variance_low() {
        let (_tmp, sidecar) = write_tmp_sidecar(&sidecar_with_event(0.0, 2.0, 2, 80));
        let render_dir = tempfile::tempdir().unwrap();
        let render_output = write_render_output(&render_dir);
        let sampler = InMemoryFrameSampler::new();
        sampler.insert(1.0, flat_frame(1920, 1080, 128));
        let result = score_caption_rendered_output(
            &render_output,
            &[sidecar],
            (1920, 1080),
            "default",
            &sampler,
        )
        .await
        .unwrap();
        assert_eq!(result.probe_count, 1);
        assert_eq!(result.occlusion_fail_count, 1);
    }

    #[tokio::test]
    async fn scorer_returns_empty_evidence_when_no_events() {
        let body = "[Script Info]\nPlayResX: 1920\nPlayResY: 1080\n\n\
[V4+ Styles]\nFormat: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\n\
Style: Default,Arial,40,&H00FFFFFF,&H000000FF,&H00000000,&H00000000,0,0,0,0,100,100,0,0,1,2,2,2,80,80,40,1\n\n\
[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n";
        let (_tmp, sidecar) = write_tmp_sidecar(body);
        let render_dir = tempfile::tempdir().unwrap();
        let render_output = write_render_output(&render_dir);
        let sampler = InMemoryFrameSampler::new();
        let result = score_caption_rendered_output(
            &render_output,
            &[sidecar],
            (1920, 1080),
            "default",
            &sampler,
        )
        .await
        .unwrap();
        assert_eq!(result.probe_count, 0);
        assert_eq!(result.fallback_reason, Some("no_caption_events"));
    }

    #[tokio::test]
    async fn scorer_reports_partial_when_some_events_unparseable() {
        let body = format!(
            "[Script Info]\nPlayResX: 1920\nPlayResY: 1080\n\n\
[V4+ Styles]\nFormat: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\n\
Style: Default,Arial,40,&H00FFFFFF,&H000000FF,&H00000000,&H00000000,0,0,0,0,100,100,0,0,1,2,2,2,80,80,40,1\n\n\
[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n\
Dialogue: 0,0:00:00.00,0:00:02.00,Default,,0,0,0,,hello world\n\
Dialogue: not-a-valid-line\n",
        );
        let (_tmp, sidecar) = write_tmp_sidecar(&body);
        let render_dir = tempfile::tempdir().unwrap();
        let render_output = write_render_output(&render_dir);
        let sampler = InMemoryFrameSampler::new();
        sampler.insert(1.0, caption_on_flat_background_frame(1920, 1080, PASS_BBOX));
        let result = score_caption_rendered_output(
            &render_output,
            &[sidecar],
            (1920, 1080),
            "default",
            &sampler,
        )
        .await
        .unwrap();
        assert_eq!(result.probe_count, 1);
        assert_eq!(result.fallback_reason, Some("partial_scorer_evidence"));
    }
}
