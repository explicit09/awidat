//! `plan_color_grade` — emit a validated, render-ready color-grade EDL
//! from a clip + intent, analyzing frames on-demand (no Python, no
//! pre-computed color sidecar).
//!
//! Slice 1 encodes the universal color-craft law: **correct first, then
//! optionally a creative look**.
//!
//! - Correction (default on): sample frames via the existing
//!   `color_scopes` ffmpeg machinery, measure stats, derive a single
//!   `Set Color Correction` op in canonical order.
//! - Look (optional): only via an EXISTING project `.cube` path. Emits an
//!   `montage.color_pipeline` Set Effect at reduced strength; auto-inserts
//!   the bundled shaper LUT before the look when the clip is camera-Log.
//!
//! There is NO catalog-id / mood → cube generation here, and NO
//! placeholder-path emission. That is a later slice.

use std::path::Path;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::color_analysis::{
    CorrectionRecommendation, FrameStats, aggregate_stats, recommended_correction,
    stats_from_snapshot,
};
use crate::montage_mcp::context::McpToolCtx;
use crate::montage_mcp::tools::clip_anchor::{normalize_project_rel, resolve_clip_anchor};
use crate::montage_mcp::tools::color_scopes::{
    self, ColorScopeInput, compute_color_scopes, extract_rgb_frame,
};
use crate::montage_mcp::tools::plan_color_grade_edl::{
    LookPlan, assemble_edl, correction_block, correction_params, is_camera_log_space, look_block,
    look_params, shaper_stem_for_space,
};
use montage_effects::{COLOR_CORRECTION, COLOR_PIPELINE, KNOWN_COLOR_SPACES, normalize_params};

const DEFAULT_SPACE: &str = "rec709_g24";
const DEFAULT_STEM: &str = "color-grade-plan";
const DEFAULT_LOOK_STRENGTH: f64 = 0.5;
/// Scope bins / extraction width mirror `color_scopes` defaults.
const BINS: usize = 64;
const MAX_WIDTH: usize = 320;
/// Hard cap on the number of frames sampled per call.
const MAX_SAMPLES: usize = 30;

/// Arguments to `plan_color_grade`.
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct PlanColorGradeArgs {
    /// Project-relative asset path. Normalized (leading `./` stripped,
    /// re-joined with `/`) for asset resolution and as the EDL anchor
    /// fallback when the asset is referenced by exactly one clip.
    pub clip: String,
    /// Clip name/uuid from `view_timeline`, used verbatim as the EDL
    /// anchor. Required when the same media appears in more than one
    /// timeline clip (an asset path alone is then ambiguous). When
    /// `None`, the anchor is resolved by counting clips that reference
    /// the asset.
    #[serde(default)]
    pub clip_anchor: Option<String>,
    /// Emit a `Set Color Correction` op. Default true.
    #[serde(default = "default_true")]
    pub correct: bool,
    /// Color space the decoded pixels live in. Must be a known space.
    #[serde(default = "default_space")]
    pub clip_input_space: String,
    /// Project-relative path to an EXISTING `.cube` creative look LUT.
    #[serde(default)]
    pub look_lut: Option<String>,
    /// Color space the `look_lut` was authored against. Default
    /// `rec709_g24`.
    #[serde(default)]
    pub lut_input_space: Option<String>,
    /// Look blend strength in `0.0..=1.0`. Default 0.5.
    #[serde(default)]
    pub look_strength: Option<f64>,
    /// Explicit sample times in seconds. Empty → derived from duration.
    #[serde(default)]
    pub t_samples: Vec<f64>,
    /// Filename stem under `renders/` for the plan artifacts.
    #[serde(default = "default_stem")]
    pub output_stem: String,
}

fn default_true() -> bool {
    true
}

fn default_space() -> String {
    DEFAULT_SPACE.into()
}

fn default_stem() -> String {
    DEFAULT_STEM.into()
}

/// Validate a stem is a plain filename under `renders/`.
fn validate_stem(field: &str, value: &str) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.is_empty()
        || trimmed.contains('/')
        || trimmed.contains('\\')
        || trimmed.contains("..")
        || trimmed == "."
    {
        return Err(format!(
            "plan_color_grade: {field} must be a plain filename stem under renders/, got {value:?}"
        ));
    }
    Ok(trimmed.to_string())
}

/// Choose sample times in source/media seconds. Explicit `t_samples` win
/// (capped, taken verbatim — already absolute source times). Otherwise
/// derive fractional samples WITHIN the clip's window
/// `[offset_s, offset_s + duration_s]` so a trimmed clip is sampled inside
/// its trim rather than over the whole media file.
fn sample_times(explicit: &[f64], duration_s: f64, offset_s: f64) -> Vec<f64> {
    if !explicit.is_empty() {
        return explicit
            .iter()
            .copied()
            .filter(|t| t.is_finite() && *t >= 0.0)
            .take(MAX_SAMPLES)
            .collect();
    }
    let base = if offset_s.is_finite() && offset_s >= 0.0 {
        offset_s
    } else {
        0.0
    };
    let fractions = [0.1, 0.25, 0.5, 0.75, 0.9];
    if !(duration_s.is_finite() && duration_s > 0.0) {
        return vec![base];
    }
    fractions
        .iter()
        .map(|f| base + (f * duration_s).max(0.0))
        .take(MAX_SAMPLES)
        .collect()
}

/// Probe a clip's duration in seconds via ffprobe. Returns `None` on any
/// failure so the caller can fall back to a single sample.
async fn probe_duration(asset_path: &Path) -> Option<f64> {
    let ffprobe = montage_render::ffprobe_path().ok()?;
    let output = tokio::process::Command::new(ffprobe)
        .arg("-v")
        .arg("error")
        .arg("-show_entries")
        .arg("format=duration")
        .arg("-of")
        .arg("default=noprint_wrappers=1:nokey=1")
        .arg(asset_path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|d| d.is_finite() && *d > 0.0)
}

/// Sample frames at the given times and return per-frame stats. Fails loud
/// when no frame could be extracted at all.
async fn collect_frame_stats(asset_path: &Path, times: &[f64]) -> Result<Vec<FrameStats>, String> {
    let mut stats = Vec::new();
    let mut last_err: Option<String> = None;
    for &t in times {
        match extract_and_score(asset_path, t).await {
            Ok(frame_stats) => stats.push(frame_stats),
            Err(e) => last_err = Some(e),
        }
    }
    if stats.is_empty() {
        return Err(format!(
            "plan_color_grade: could not sample any frame from {}: {}",
            asset_path.display(),
            last_err.unwrap_or_else(|| "no usable sample times".into())
        ));
    }
    Ok(stats)
}

async fn extract_and_score(asset_path: &Path, t_s: f64) -> Result<FrameStats, String> {
    let frame: ColorScopeInput = extract_rgb_frame(asset_path, t_s, BINS, MAX_WIDTH).await?;
    let snapshot = compute_color_scopes(&frame)
        .map_err(|e| format!("plan_color_grade: scope computation failed: {e}"))?;
    Ok(stats_from_snapshot(&snapshot))
}

/// Run `plan_color_grade`. Resolves the asset, samples frames, derives a
/// correction, optionally builds a look, writes `renders/<stem>.edl` and
/// `renders/<stem>.json`, and returns the JSON summary as a string.
pub async fn run(args: PlanColorGradeArgs, ctx: McpToolCtx) -> Result<String, String> {
    if !KNOWN_COLOR_SPACES.contains(&args.clip_input_space.as_str()) {
        return Err(format!(
            "plan_color_grade: clip_input_space {:?} is not a known color space",
            args.clip_input_space
        ));
    }
    let lut_input_space = args
        .lut_input_space
        .clone()
        .unwrap_or_else(|| DEFAULT_SPACE.to_string());
    if args.look_lut.is_some() && !KNOWN_COLOR_SPACES.contains(&lut_input_space.as_str()) {
        return Err(format!(
            "plan_color_grade: lut_input_space {lut_input_space:?} is not a known color space"
        ));
    }
    // Fail loud on a non-finite look_strength: clamp(NaN) is NaN, which the
    // param map silently drops, yielding a full-strength look.
    if let Some(strength) = args.look_strength
        && !strength.is_finite()
    {
        return Err("plan_color_grade: look_strength must be finite".into());
    }
    // Normalize the clip to the canonical project-relative shape the
    // timeline target_url / apply_edl anchor uses (strip a leading
    // `./`, reject `..`/absolute/backslashes). A non-canonical spelling
    // like `./raw/foo.mp4` would otherwise be emitted raw and never
    // match `raw/foo.mp4` on the timeline.
    let clip_rel = normalize_project_rel(&args.clip)
        .map_err(|e| format!("plan_color_grade: clip {:?}: {e}", args.clip))?;
    let output_stem = validate_stem("output_stem", &args.output_stem)?;

    // Normalize + validate the look LUT shape AND contents up front so a
    // missing, non-canonical, or non-cube path fails loud before any
    // frame sampling. `look_lut` is rebound to its normalized form so
    // the emitted anchor matches what apply_edl will accept.
    let look_lut = match &args.look_lut {
        Some(raw) => {
            let normalized = normalize_lut_path(raw)?;
            validate_lut_contents(&ctx.project_root, &normalized)?;
            Some(normalized)
        }
        None => None,
    };

    // Pick the EDL anchor: an explicit clip_anchor (a NAME, used
    // verbatim, but validated against the timeline) wins; otherwise
    // require the normalized asset to be referenced by exactly one VIDEO
    // timeline clip. Carries the matched clip's source range so we sample
    // within the trim, not over the whole media file.
    let resolved = resolve_clip_anchor(&ctx.project_root, &clip_rel, args.clip_anchor.as_deref())
        .map_err(|e| format!("plan_color_grade: {e}"))?;
    let anchor = resolved.anchor;

    let asset_path = color_scopes::resolve_asset_path(&ctx.project_root, &clip_rel)?;

    let mut limitations: Vec<String> = Vec::new();

    // Sample frames + aggregate stats. When the targeted clip is a trim of
    // a longer media file, sample WITHIN `[start, start + duration]` so we
    // analyze the frames the grade actually applies to; otherwise fall
    // back to the whole asset's probed duration starting at 0.
    let (sample_offset, sample_span) = match resolved.source_range {
        Some((start, dur)) if start.is_finite() && start >= 0.0 && dur.is_finite() && dur > 0.0 => {
            (start, dur)
        }
        _ => (0.0, probe_duration(&asset_path).await.unwrap_or(0.0)),
    };
    let times = sample_times(&args.t_samples, sample_span, sample_offset);
    let frame_stats = collect_frame_stats(&asset_path, &times).await?;
    let aggregate = aggregate_stats(&frame_stats);

    // Build EDL blocks in canonical order: correct first, then look.
    let mut blocks: Vec<String> = Vec::new();

    let correction = if args.correct {
        let rec = recommended_correction(&aggregate);
        let params = correction_params(&rec);
        normalize_params(COLOR_CORRECTION, &params)
            .map_err(|e| format!("plan_color_grade: correction failed registry validation: {e}"))?;
        blocks.push(correction_block(&anchor, &rec));
        Some(rec)
    } else {
        None
    };

    let look = if let Some(look_lut) = &look_lut {
        let strength = args
            .look_strength
            .unwrap_or(DEFAULT_LOOK_STRENGTH)
            .clamp(0.0, 1.0);
        let shaper_lut = resolve_look_shaper(
            &ctx.project_root,
            &args.clip_input_space,
            &lut_input_space,
            &mut limitations,
        )
        .await?;
        let plan = LookPlan {
            clip_input_space: args.clip_input_space.clone(),
            lut_input_space: lut_input_space.clone(),
            look_lut: look_lut.clone(),
            look_strength: strength,
            shaper_lut,
        };
        let params = look_params(&plan);
        normalize_params(COLOR_PIPELINE, &params)
            .map_err(|e| format!("plan_color_grade: look failed registry validation: {e}"))?;
        blocks.push(look_block(&anchor, &plan)?);
        Some(plan)
    } else {
        None
    };

    if blocks.is_empty() {
        return Err(
            "plan_color_grade: nothing to plan — correction disabled and no look_lut given".into(),
        );
    }

    let edl = assemble_edl(&blocks);

    // Write artifacts.
    let renders = ctx.project_root.join("renders");
    tokio::fs::create_dir_all(&renders).await.map_err(|e| {
        format!(
            "plan_color_grade: failed to create {}: {e}",
            renders.display()
        )
    })?;
    let edl_path = renders.join(format!("{output_stem}.edl"));
    let json_path = renders.join(format!("{output_stem}.json"));

    tokio::fs::write(&edl_path, &edl).await.map_err(|e| {
        format!(
            "plan_color_grade: failed to write {}: {e}",
            edl_path.display()
        )
    })?;

    let summary = build_summary(
        &clip_rel,
        &anchor,
        &edl_path,
        &edl,
        &aggregate,
        correction.as_ref(),
        look.as_ref(),
        &limitations,
        frame_stats.len(),
    );
    let summary_str = serde_json::to_string_pretty(&summary)
        .map_err(|e| format!("plan_color_grade: failed to serialize summary: {e}"))?;
    tokio::fs::write(&json_path, &summary_str)
        .await
        .map_err(|e| {
            format!(
                "plan_color_grade: failed to write {}: {e}",
                json_path.display()
            )
        })?;

    Ok(summary_str)
}

/// Resolve (and materialize) the Log→Rec.709 shaper for a look.
///
/// Returns `Ok(Some(project_relative_csp))` only when:
/// - the clip is a camera-Log encoding that HAS a bundled shaper, AND
/// - the look LUT is authored in `rec709_g24` (the shaper's output space).
///
/// In that case the bundled `.csp` is copied from the skills bundle into
/// `<project>/luts/shapers/<stem>.csp` so the renderer's
/// `project_root.join(raw)` resolves it. Returns `Ok(None)` (and records a
/// `limitations` entry where appropriate) otherwise, so the look is never
/// emitted with an unresolvable or mismatched shaper path.
async fn resolve_look_shaper(
    project_root: &Path,
    clip_input_space: &str,
    lut_input_space: &str,
    limitations: &mut Vec<String>,
) -> Result<Option<String>, String> {
    // Non-Log clip: no shaper needed.
    if !is_camera_log_space(clip_input_space) {
        return Ok(None);
    }
    // Log clip but the look isn't Rec.709-authored: the bundled shaper maps
    // Log→rec709_g24, so inserting it would feed Rec.709 pixels to a
    // non-Rec709 look. Don't auto-insert; record the mismatch.
    if lut_input_space != DEFAULT_SPACE {
        limitations.push(format!(
            "clip_input_space {clip_input_space:?} is Log but lut_input_space is {lut_input_space:?} (not rec709_g24); the bundled shaper maps Log→rec709_g24, so it was NOT auto-inserted — supply a matching shaper_lut/input_transform_lut manually"
        ));
        return Ok(None);
    }
    // Log clip without any bundled shaper: don't pretend it was normalized.
    let Some(stem) = shaper_stem_for_space(clip_input_space) else {
        limitations.push(format!(
            "no bundled shaper for {clip_input_space}; a Rec.709-authored LUT would be applied to Log pixels — supply a shaper_lut/input_transform_lut manually"
        ));
        return Ok(None);
    };
    // Copy the bundled shaper into the project so the renderer can resolve
    // it via project_root.join(raw).
    let Some(skills_root) = montage_config::defaults::skills_root() else {
        limitations.push(format!(
            "bundled shaper {stem}.csp could not be located (no skills root); look applied WITHOUT a shaper"
        ));
        return Ok(None);
    };
    let src = skills_root
        .join("color-corrector")
        .join("shapers")
        .join(format!("{stem}.csp"));
    if !src.is_file() {
        limitations.push(format!(
            "bundled shaper {stem}.csp is not installed at {}; look applied WITHOUT a shaper",
            src.display()
        ));
        return Ok(None);
    }
    let rel = format!("luts/shapers/{stem}.csp");
    let dest = project_root.join(&rel);
    // Don't clobber a possibly-customized project shaper: if the
    // destination already exists, reuse it and emit the same
    // project-relative path. Only copy the bundled `.csp` when absent.
    if !dest.exists() {
        let dest_dir = project_root.join("luts").join("shapers");
        tokio::fs::create_dir_all(&dest_dir).await.map_err(|e| {
            format!(
                "plan_color_grade: failed to create {}: {e}",
                dest_dir.display()
            )
        })?;
        tokio::fs::copy(&src, &dest).await.map_err(|e| {
            format!(
                "plan_color_grade: failed to copy shaper {} -> {}: {e}",
                src.display(),
                dest.display()
            )
        })?;
    }
    Ok(Some(rel))
}

/// Verify a look LUT exists and parses as a 3D `.cube`, mirroring the
/// renderer's `apply_lut` validation. Fails loud, naming the file.
fn validate_lut_contents(project_root: &Path, look_lut: &str) -> Result<(), String> {
    let full = project_root.join(look_lut);
    let bytes = std::fs::read(&full)
        .map_err(|e| format!("plan_color_grade: look_lut {look_lut:?} could not be read: {e}"))?;
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| format!("plan_color_grade: look_lut {look_lut:?} is not valid UTF-8"))?;
    montage_lut::parse_cube_3d(text).map(|_| ()).map_err(|e| {
        format!("plan_color_grade: look_lut {look_lut:?} is not a valid 3D .cube: {e}")
    })
}

/// Normalize a look LUT path to the canonical project-relative shape
/// `apply_edl` accepts for color-pipeline LUTs: strip a leading `./`,
/// reject `..`/absolute/backslashes and any non-`Component::Normal`
/// part, re-join with `/`. Also enforce the `.cube` extension. Returns
/// the normalized path so the emitted op matches what apply_edl reads.
fn normalize_lut_path(path: &str) -> Result<String, String> {
    let normalized = normalize_project_rel(path)
        .map_err(|e| format!("plan_color_grade: look_lut {path:?}: {e}"))?;
    if !normalized.to_ascii_lowercase().ends_with(".cube") {
        return Err(format!(
            "plan_color_grade: look_lut {path:?} must be a .cube file"
        ));
    }
    Ok(normalized)
}

#[allow(clippy::too_many_arguments)]
fn build_summary(
    clip_rel: &str,
    anchor: &str,
    edl_path: &Path,
    edl: &str,
    aggregate: &FrameStats,
    correction: Option<&CorrectionRecommendation>,
    look: Option<&LookPlan>,
    limitations: &[String],
    frames_sampled: usize,
) -> Value {
    let correction_json = correction.map(|rec| {
        serde_json::json!({
            "exposure_ev": rec.exposure_ev,
            "contrast": rec.contrast,
            "saturation": rec.saturation,
            "temperature": rec.temperature,
            "tint": rec.tint,
            "shadows": rec.shadows,
            "highlights": rec.highlights,
        })
    });
    let look_json = look.map(|plan| {
        serde_json::json!({
            "clip_input_space": plan.clip_input_space,
            "lut_input_space": plan.lut_input_space,
            "look_lut": plan.look_lut,
            "look_strength": plan.look_strength,
            "shaper_lut": plan.shaper_lut,
        })
    });
    serde_json::json!({
        "clip": clip_rel,
        "clip_anchor": anchor,
        "edl_path": edl_path.display().to_string(),
        "edl": edl,
        "frames_sampled": frames_sampled,
        "stats": {
            "brightness": aggregate.brightness,
            "contrast": aggregate.contrast,
            "luma_p05": aggregate.luma_p05,
            "luma_p50": aggregate.luma_p50,
            "luma_p95": aggregate.luma_p95,
            "mean_r": aggregate.mean_r,
            "mean_g": aggregate.mean_g,
            "mean_b": aggregate.mean_b,
            "overexposed_fraction": aggregate.overexposed_fraction,
            "underexposed_fraction": aggregate.underexposed_fraction,
        },
        "correction": correction_json,
        "look": look_json,
        "rationale": "correct first (exposure, contrast, white balance), then a creative look at reduced strength",
        "limitations": limitations,
    })
}

pub const DESCRIPTION: &str = "\
Plan a render-ready color grade for one clip: sample its frames, measure \
exposure/contrast/white-balance, and emit a validated EDL that corrects \
first and then (optionally, only with an existing project .cube) applies a \
creative look at reduced strength. Writes renders/<stem>.edl and \
renders/<stem>.json. After this tool, call apply_edl with the returned \
`edl` text, inspect vedit_diff, then render the timeline.";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edl::op::EdlOp;
    use crate::edl::parser::parse;
    use std::path::PathBuf;

    // EDL emission, parameter-map, and shaper-resolution behavior is
    // covered in the sibling `plan_color_grade_edl` module's tests. The
    // tests here exercise the run flow, argument validation, and sample
    // scheduling owned by this module.

    /// Serializes tests that mutate the process-global `MONTAGE_SKILLS_ROOT`.
    /// Under the parallel test harness, one test clearing/replacing the var
    /// mid-run races another that just set it, surfacing as intermittent
    /// "no shaper" failures (finding 3364995226). Every test that sets or
    /// removes the var holds this lock for the duration of its mutation and
    /// the `run()` call. Crate-shared so READERS in other modules (e.g.
    /// plan_look_regions resolving bundled scripts via skills_root())
    /// serialize against the mutations too.
    use crate::test_env::SKILLS_ROOT_ENV_LOCK as ENV_LOCK;

    #[test]
    fn normalize_lut_path_rejects_absolute_and_traversal() {
        assert!(normalize_lut_path("/etc/passwd.cube").is_err());
        assert!(normalize_lut_path("../secrets.cube").is_err());
        assert!(normalize_lut_path("luts/show.png").is_err());
        assert_eq!(
            normalize_lut_path("luts/show.cube").unwrap(),
            "luts/show.cube"
        );
    }

    #[test]
    fn normalize_lut_path_rejects_backslash_and_traversal() {
        // These read fine on disk but apply_edl rejects them, so the
        // planner must reject them too (finding 3364775592).
        assert!(normalize_lut_path("luts\\show.cube").is_err());
        assert!(normalize_lut_path("luts/../show.cube").is_err());
    }

    #[test]
    fn normalize_lut_path_collapses_interior_dot() {
        // An interior `./` is collapsed by path normalization (same as
        // apply_edl's component check), so it canonicalizes rather than
        // erroring.
        assert_eq!(
            normalize_lut_path("luts/./show.cube").unwrap(),
            "luts/show.cube"
        );
    }

    #[test]
    fn normalize_lut_path_strips_leading_dot_slash() {
        assert_eq!(
            normalize_lut_path("./luts/show.cube").unwrap(),
            "luts/show.cube"
        );
    }

    #[test]
    fn sample_times_derives_fractions_from_duration() {
        let times = sample_times(&[], 10.0, 0.0);
        assert_eq!(times, vec![1.0, 2.5, 5.0, 7.5, 9.0]);
    }

    /// Finding 3364897030: a trimmed clip (offset>0, span<media) samples
    /// fractions WITHIN `[offset, offset + span]`, not from 0.
    #[test]
    fn sample_times_offsets_fractions_into_clip_window() {
        let times = sample_times(&[], 10.0, 2.0);
        assert_eq!(times, vec![3.0, 4.5, 7.0, 9.5, 11.0]);
        // Every sample is inside the clip's source window.
        assert!(times.iter().all(|&t| (2.0..=12.0).contains(&t)));
    }

    #[test]
    fn sample_times_honors_explicit_and_caps() {
        let explicit: Vec<f64> = (0..50).map(|i| i as f64).collect();
        let times = sample_times(&explicit, 100.0, 0.0);
        assert_eq!(times.len(), MAX_SAMPLES);
    }

    // --- end-to-end run() with tiny generated clips ---

    fn ffmpeg_available() -> bool {
        montage_render::ffmpeg_path().is_ok()
    }

    async fn make_clip(dir: &Path, name: &str, color: &str) -> PathBuf {
        let path = dir.join(name);
        let ffmpeg = montage_render::ffmpeg_path().unwrap();
        let status = tokio::process::Command::new(ffmpeg)
            .arg("-hide_banner")
            .arg("-loglevel")
            .arg("error")
            .arg("-y")
            .arg("-f")
            .arg("lavfi")
            .arg("-i")
            .arg(format!("color=c={color}:s=320x180:d=1"))
            .arg("-frames:v")
            .arg("12")
            .arg(&path)
            .status()
            .await
            .unwrap();
        assert!(status.success(), "ffmpeg failed to make {name}");
        path
    }

    fn ctx_for(root: &Path) -> McpToolCtx {
        McpToolCtx {
            project_root: root.to_path_buf(),
        }
    }

    /// Write a minimal `project.otio.json` with one clip per asset path
    /// so `resolve_clip_anchor` can count timeline references.
    fn write_timeline(root: &Path, assets: &[&str]) {
        use montage_proto::otio::{
            Clip, ExternalReference, MediaReference, StackChild, Timeline, Track, TrackChild,
            TrackKind,
        };
        let mut tl = Timeline::empty("test");
        let mut track = Track::empty("V1", TrackKind::Video);
        for (i, asset) in assets.iter().enumerate() {
            let mut clip = Clip::empty(format!("clip-{i}"));
            clip.media_reference = MediaReference::External(ExternalReference::new(*asset));
            track.children.push(TrackChild::Clip(clip));
        }
        tl.tracks.children.push(StackChild::Track(track));
        let json = serde_json::to_vec_pretty(&tl).unwrap();
        std::fs::write(root.join("project.otio.json"), json).unwrap();
    }

    /// Write a `project.otio.json` with one VIDEO clip per `(name, asset)`
    /// pair, so an explicit `clip_anchor` can be matched by clip name.
    fn write_named_timeline(root: &Path, clips: &[(&str, &str)]) {
        use montage_proto::otio::{
            Clip, ExternalReference, MediaReference, StackChild, Timeline, Track, TrackChild,
            TrackKind,
        };
        let mut tl = Timeline::empty("test");
        let mut track = Track::empty("V1", TrackKind::Video);
        for (name, asset) in clips {
            let mut clip = Clip::empty(*name);
            clip.media_reference = MediaReference::External(ExternalReference::new(*asset));
            track.children.push(TrackChild::Clip(clip));
        }
        tl.tracks.children.push(StackChild::Track(track));
        let json = serde_json::to_vec_pretty(&tl).unwrap();
        std::fs::write(root.join("project.otio.json"), json).unwrap();
    }

    /// Write a `project.otio.json` with a single VIDEO clip referencing
    /// `asset`, trimmed to source range `[start_s, start_s + dur_s]`.
    fn write_trimmed_timeline(root: &Path, asset: &str, start_s: f64, dur_s: f64) {
        use montage_proto::otio::{
            Clip, ExternalReference, MediaReference, RationalTime, StackChild, TimeRange, Timeline,
            Track, TrackChild, TrackKind,
        };
        let mut tl = Timeline::empty("test");
        let mut track = Track::empty("V1", TrackKind::Video);
        let mut clip = Clip::empty("trimmed");
        clip.media_reference = MediaReference::External(ExternalReference::new(asset));
        clip.source_range = Some(TimeRange::new(
            RationalTime::new(start_s, 1.0),
            RationalTime::new(dur_s, 1.0),
        ));
        track.children.push(TrackChild::Clip(clip));
        tl.tracks.children.push(StackChild::Track(track));
        let json = serde_json::to_vec_pretty(&tl).unwrap();
        std::fs::write(root.join("project.otio.json"), json).unwrap();
    }

    /// A minimal but VALID 2×2×2 identity `.cube` (eight corner rows),
    /// so `validate_lut_contents` accepts it.
    const VALID_CUBE: &str = "\
# identity 2x2x2
LUT_3D_SIZE 2
0.0 0.0 0.0
1.0 0.0 0.0
0.0 1.0 0.0
1.0 1.0 0.0
0.0 0.0 1.0
1.0 0.0 1.0
0.0 1.0 1.0
1.0 1.0 1.0
";

    fn write_valid_cube(root: &Path, rel: &str) {
        let full = root.join(rel);
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(full, VALID_CUBE).unwrap();
    }

    #[tokio::test]
    async fn run_emits_correction_edl_end_to_end() {
        if !ffmpeg_available() {
            eprintln!("skipping: ffmpeg unavailable");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("raw")).unwrap();
        let _clip = make_clip(&root.join("raw"), "dark.mp4", "0x2E2E2E").await;
        write_timeline(root, &["raw/dark.mp4"]);

        let args = PlanColorGradeArgs {
            clip: "raw/dark.mp4".into(),
            clip_anchor: None,
            correct: true,
            clip_input_space: "rec709_g24".into(),
            look_lut: None,
            lut_input_space: None,
            look_strength: None,
            t_samples: vec![0.0],
            output_stem: "color-grade-plan".into(),
        };
        let out = run(args, ctx_for(root)).await.unwrap();
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert!(
            parsed
                .get("correction")
                .map(|v| !v.is_null())
                .unwrap_or(false)
        );
        assert!(parsed.get("look").map(|v| v.is_null()).unwrap_or(false));
        assert!(parsed.get("stats").is_some());

        let edl_path = root.join("renders/color-grade-plan.edl");
        assert!(edl_path.is_file());
        let edl_text = std::fs::read_to_string(&edl_path).unwrap();
        let env = parse(&edl_text).unwrap();
        assert_eq!(env.len(), 1);
        assert!(matches!(env.ops[0], EdlOp::SetColorCorrection { .. }));
    }

    #[tokio::test]
    async fn run_with_correct_false_emits_no_correction() {
        if !ffmpeg_available() {
            eprintln!("skipping: ffmpeg unavailable");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("raw")).unwrap();
        write_valid_cube(root, "luts/show.cube");
        make_clip(&root.join("raw"), "mid.mp4", "gray").await;
        write_timeline(root, &["raw/mid.mp4"]);

        let args = PlanColorGradeArgs {
            clip: "raw/mid.mp4".into(),
            clip_anchor: None,
            correct: false,
            clip_input_space: "rec709_g24".into(),
            look_lut: Some("luts/show.cube".into()),
            lut_input_space: None,
            look_strength: Some(0.5),
            t_samples: vec![0.0],
            output_stem: "no-correct".into(),
        };
        let out = run(args, ctx_for(root)).await.unwrap();
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert!(
            parsed
                .get("correction")
                .map(|v| v.is_null())
                .unwrap_or(false)
        );
        assert!(parsed.get("look").map(|v| !v.is_null()).unwrap_or(false));

        // The summary carries the full EDL envelope text for apply_edl
        // (which takes EDL text, not a path).
        let edl_in_summary = parsed.get("edl").and_then(|v| v.as_str()).unwrap();
        let edl_text = std::fs::read_to_string(root.join("renders/no-correct.edl")).unwrap();
        assert_eq!(edl_in_summary, edl_text);
        let env = parse(&edl_text).unwrap();
        assert_eq!(env.len(), 1);
        assert!(matches!(env.ops[0], EdlOp::SetEffect { .. }));
    }

    #[tokio::test]
    async fn run_without_look_lut_emits_no_set_effect() {
        if !ffmpeg_available() {
            eprintln!("skipping: ffmpeg unavailable");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("raw")).unwrap();
        make_clip(&root.join("raw"), "mid.mp4", "gray").await;
        write_timeline(root, &["raw/mid.mp4"]);

        let args = PlanColorGradeArgs {
            clip: "raw/mid.mp4".into(),
            clip_anchor: None,
            correct: true,
            clip_input_space: "rec709_g24".into(),
            look_lut: None,
            lut_input_space: None,
            look_strength: None,
            t_samples: vec![0.0],
            output_stem: "correct-only".into(),
        };
        run(args, ctx_for(root)).await.unwrap();
        let edl_text = std::fs::read_to_string(root.join("renders/correct-only.edl")).unwrap();
        let env = parse(&edl_text).unwrap();
        assert!(
            env.ops
                .iter()
                .all(|op| !matches!(op, EdlOp::SetEffect { .. }))
        );
    }

    /// Base args with a project-relative clip and no look. Tests override
    /// individual fields.
    fn base_args(clip: &str, stem: &str) -> PlanColorGradeArgs {
        PlanColorGradeArgs {
            clip: clip.into(),
            clip_anchor: None,
            correct: true,
            clip_input_space: "rec709_g24".into(),
            look_lut: None,
            lut_input_space: None,
            look_strength: None,
            t_samples: vec![0.0],
            output_stem: stem.into(),
        }
    }

    // --- clip-anchor / path normalization (findings 3364775588/592/595) ---

    /// Finding 3364775588: a `./raw/x.mp4` clip must normalize and
    /// anchor to `raw/x.mp4` so it matches the timeline target_url.
    #[tokio::test]
    async fn run_normalizes_dot_slash_clip_and_anchor() {
        if !ffmpeg_available() {
            eprintln!("skipping: ffmpeg unavailable");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("raw")).unwrap();
        make_clip(&root.join("raw"), "x.mp4", "gray").await;
        write_timeline(root, &["raw/x.mp4"]);

        let mut args = base_args("./raw/x.mp4", "dot-slash");
        args.t_samples = vec![0.0];
        let out = run(args, ctx_for(root)).await.unwrap();
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(
            parsed.get("clip").and_then(|v| v.as_str()),
            Some("raw/x.mp4")
        );
        // The clip PATH normalizes (`./raw/x.mp4` → `raw/x.mp4`), but the
        // emitted anchor is the matched clip's id ("clip-0"), so apply_edl
        // resolves it on the first pass.
        assert_eq!(
            parsed.get("clip_anchor").and_then(|v| v.as_str()),
            Some("clip-0")
        );
        let edl = parsed.get("edl").and_then(|v| v.as_str()).unwrap();
        assert!(edl.contains("clip_uuid=clip-0"), "edl was: {edl}");
        assert!(!edl.contains("./raw/x.mp4"), "edl leaked ./ form: {edl}");
    }

    /// Finding 3364775588: a backslash / `..` / absolute clip must be
    /// rejected with a clear error before any sampling.
    #[tokio::test]
    async fn run_rejects_non_normalizable_clip() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        for bad in ["raw\\x.mp4", "../x.mp4", "raw/../x.mp4"] {
            let args = base_args(bad, "bad-clip");
            let err = run(args, ctx_for(root)).await.unwrap_err();
            assert!(
                err.contains("plan_color_grade: clip"),
                "unexpected error for {bad:?}: {err}"
            );
        }
    }

    /// Finding 3364775592: a `look_lut` apply_edl would reject (a
    /// backslash or `..`) must fail at plan time, even though it reads
    /// fine on disk.
    #[tokio::test]
    async fn run_rejects_non_normalizable_look_lut() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // Write the cube at the canonical location so a shape-only check
        // wouldn't catch the non-canonical spelling.
        write_valid_cube(root, "luts/show.cube");
        for bad in ["luts\\show.cube", "luts/../luts/show.cube"] {
            let mut args = base_args("raw/clip.mp4", "bad-lut-path");
            args.correct = false;
            args.look_lut = Some(bad.into());
            let err = run(args, ctx_for(root)).await.unwrap_err();
            assert!(
                err.contains("look_lut"),
                "unexpected error for {bad:?}: {err}"
            );
        }
    }

    /// Finding 3364775595 / 3364897033: an explicit `clip_anchor` that
    /// names a real video clip referencing the asset is used verbatim as
    /// the EDL anchor (it's a name, not a path — not normalized), and is
    /// validated against the timeline.
    #[tokio::test]
    async fn run_uses_explicit_clip_anchor_verbatim() {
        if !ffmpeg_available() {
            eprintln!("skipping: ffmpeg unavailable");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("raw")).unwrap();
        make_clip(&root.join("raw"), "x.mp4", "gray").await;
        // Asset appears in TWO clips, one of them named "Interview Take 2":
        // without clip_anchor this would be ambiguous, but the explicit
        // name resolves it (and is validated to reference the asset).
        write_named_timeline(
            root,
            &[
                ("Interview Take 1", "raw/x.mp4"),
                ("Interview Take 2", "raw/x.mp4"),
            ],
        );

        let mut args = base_args("raw/x.mp4", "explicit-anchor");
        args.clip_anchor = Some("Interview Take 2".into());
        let out = run(args, ctx_for(root)).await.unwrap();
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(
            parsed.get("clip_anchor").and_then(|v| v.as_str()),
            Some("Interview Take 2")
        );
        let edl = parsed.get("edl").and_then(|v| v.as_str()).unwrap();
        assert!(edl.contains("clip_uuid=Interview Take 2"), "edl was: {edl}");
    }

    /// Finding 3364897033: an explicit `clip_anchor` that does NOT name a
    /// video clip referencing the asset is rejected at plan time.
    #[tokio::test]
    async fn run_rejects_explicit_clip_anchor_not_referencing_asset() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // The named clip references a DIFFERENT asset than the requested one.
        write_named_timeline(root, &[("Take A", "raw/other.mp4")]);
        let mut args = base_args("raw/x.mp4", "bad-anchor");
        args.clip_anchor = Some("Take A".into());
        let err = run(args, ctx_for(root)).await.unwrap_err();
        assert!(
            err.contains("does not match any video clip referencing")
                || err.contains("resolves to a clip referencing"),
            "unexpected error: {err}"
        );
    }

    /// An asset referenced by exactly one clip anchors to that clip's own
    /// id (its name "clip-0" here, or its uuid when present) — NOT the asset
    /// path — so apply_edl resolves it on the first pass instead of the
    /// audio-counting asset fallback (round-5 findings 3366562448/50).
    #[tokio::test]
    async fn run_single_clip_anchors_to_clip_id() {
        if !ffmpeg_available() {
            eprintln!("skipping: ffmpeg unavailable");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("raw")).unwrap();
        make_clip(&root.join("raw"), "x.mp4", "gray").await;
        write_timeline(root, &["raw/x.mp4", "raw/other.mp4"]);

        let args = base_args("raw/x.mp4", "single-clip");
        let out = run(args, ctx_for(root)).await.unwrap();
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(
            parsed.get("clip_anchor").and_then(|v| v.as_str()),
            Some("clip-0")
        );
        // And the emitted EDL anchors by that clip id, not the asset path.
        let edl = parsed.get("edl").and_then(|v| v.as_str()).unwrap_or("");
        assert!(
            edl.contains("clip_uuid=clip-0"),
            "EDL should anchor by clip id: {edl}"
        );
    }

    /// Finding 3364775595: an asset referenced by two clips with no
    /// `clip_anchor` must fail loud at plan time, naming the count.
    #[tokio::test]
    async fn run_ambiguous_asset_without_anchor_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write_timeline(root, &["raw/clip.mp4", "raw/clip.mp4"]);
        let args = base_args("raw/clip.mp4", "ambiguous");
        let err = run(args, ctx_for(root)).await.unwrap_err();
        assert!(
            err.contains("appears in 2 clips") && err.contains("clip_anchor"),
            "unexpected error: {err}"
        );
    }

    /// Finding 3364775595: an asset referenced by zero clips errors.
    #[tokio::test]
    async fn run_unreferenced_asset_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write_timeline(root, &["raw/other.mp4"]);
        let args = base_args("raw/missing.mp4", "unreferenced");
        let err = run(args, ctx_for(root)).await.unwrap_err();
        assert!(
            err.contains("no timeline clip references"),
            "unexpected error: {err}"
        );
    }

    // --- argument validation that fails before any frame sampling ---

    #[tokio::test]
    async fn run_rejects_non_finite_look_strength() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let mut args = base_args("raw/clip.mp4", "nan");
        args.look_lut = Some("luts/show.cube".into());
        args.look_strength = Some(f64::NAN);
        let err = run(args, ctx_for(root)).await.unwrap_err();
        assert!(
            err.contains("look_strength must be finite"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn run_rejects_absolute_clip() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let args = base_args("/tmp/abs.mp4", "abs");
        let err = run(args, ctx_for(root)).await.unwrap_err();
        assert!(
            err.contains("must be a project-relative path"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn run_rejects_missing_look_lut() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let mut args = base_args("raw/clip.mp4", "missing-lut");
        args.look_lut = Some("luts/nope.cube".into());
        let err = run(args, ctx_for(root)).await.unwrap_err();
        assert!(err.contains("could not be read"), "unexpected error: {err}");
    }

    #[tokio::test]
    async fn run_rejects_invalid_look_lut_contents() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // Declares 3D size 2 but has no data rows → not a valid 3D cube.
        std::fs::create_dir_all(root.join("luts")).unwrap();
        std::fs::write(root.join("luts/bad.cube"), "# bad\nLUT_3D_SIZE 2\n").unwrap();
        let mut args = base_args("raw/clip.mp4", "bad-lut");
        args.look_lut = Some("luts/bad.cube".into());
        let err = run(args, ctx_for(root)).await.unwrap_err();
        assert!(
            err.contains("is not a valid 3D .cube"),
            "unexpected error: {err}"
        );
    }

    // --- shaper resolution / limitations (need ffmpeg to reach the look) ---

    #[tokio::test]
    async fn run_copies_bundled_shaper_into_project_for_log_clip() {
        if !ffmpeg_available() {
            eprintln!("skipping: ffmpeg unavailable");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("raw")).unwrap();
        write_valid_cube(root, "luts/show.cube");
        make_clip(&root.join("raw"), "log.mp4", "gray").await;
        write_timeline(root, &["raw/log.mp4"]);

        // Fake skills bundle with a shaper for arri_logc4.
        let skills = tempfile::tempdir().unwrap();
        let shaper_dir = skills.path().join("color-corrector").join("shapers");
        std::fs::create_dir_all(&shaper_dir).unwrap();
        std::fs::write(
            shaper_dir.join("arri_logc4_to_rec709_g24.csp"),
            "fake shaper contents\n",
        )
        .unwrap();
        // Serialize env mutation + run() against other shaper tests.
        let _env = ENV_LOCK.lock().await;
        // SAFETY: test process; set then restore.
        unsafe {
            std::env::set_var("MONTAGE_SKILLS_ROOT", skills.path());
        }

        let mut args = base_args("raw/log.mp4", "log-shaper");
        args.correct = false;
        args.clip_input_space = "arri_logc4".into();
        args.look_lut = Some("luts/show.cube".into());
        let out = run(args, ctx_for(root)).await.unwrap();

        unsafe {
            std::env::remove_var("MONTAGE_SKILLS_ROOT");
        }

        let parsed: Value = serde_json::from_str(&out).unwrap();
        let shaper = parsed
            .get("look")
            .and_then(|l| l.get("shaper_lut"))
            .and_then(|v| v.as_str())
            .unwrap();
        assert_eq!(shaper, "luts/shapers/arri_logc4_to_rec709_g24.csp");
        assert!(
            root.join("luts/shapers/arri_logc4_to_rec709_g24.csp")
                .is_file(),
            "shaper was not copied into the project"
        );
    }

    /// Finding 3364897035: an existing project shaper at the destination
    /// must NOT be overwritten by the bundled copy.
    #[tokio::test]
    async fn run_does_not_overwrite_existing_project_shaper() {
        if !ffmpeg_available() {
            eprintln!("skipping: ffmpeg unavailable");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("raw")).unwrap();
        write_valid_cube(root, "luts/show.cube");
        make_clip(&root.join("raw"), "log.mp4", "gray").await;
        write_timeline(root, &["raw/log.mp4"]);

        // A customized project shaper already lives at the destination.
        let dest_dir = root.join("luts").join("shapers");
        std::fs::create_dir_all(&dest_dir).unwrap();
        let dest = dest_dir.join("arri_logc4_to_rec709_g24.csp");
        const SENTINEL: &str = "CUSTOMIZED PROJECT SHAPER — DO NOT CLOBBER\n";
        std::fs::write(&dest, SENTINEL).unwrap();

        // Bundled shaper has DIFFERENT contents; a clobber would change them.
        let skills = tempfile::tempdir().unwrap();
        let shaper_dir = skills.path().join("color-corrector").join("shapers");
        std::fs::create_dir_all(&shaper_dir).unwrap();
        std::fs::write(
            shaper_dir.join("arri_logc4_to_rec709_g24.csp"),
            "bundled shaper contents\n",
        )
        .unwrap();
        let _env = ENV_LOCK.lock().await;
        unsafe {
            std::env::set_var("MONTAGE_SKILLS_ROOT", skills.path());
        }

        let mut args = base_args("raw/log.mp4", "keep-shaper");
        args.correct = false;
        args.clip_input_space = "arri_logc4".into();
        args.look_lut = Some("luts/show.cube".into());
        let out = run(args, ctx_for(root)).await.unwrap();

        unsafe {
            std::env::remove_var("MONTAGE_SKILLS_ROOT");
        }

        // The emitted path is still the project-relative shaper...
        let parsed: Value = serde_json::from_str(&out).unwrap();
        let shaper = parsed
            .get("look")
            .and_then(|l| l.get("shaper_lut"))
            .and_then(|v| v.as_str())
            .unwrap();
        assert_eq!(shaper, "luts/shapers/arri_logc4_to_rec709_g24.csp");
        // ...but the existing file's contents are untouched.
        let on_disk = std::fs::read_to_string(&dest).unwrap();
        assert_eq!(on_disk, SENTINEL, "existing project shaper was overwritten");
    }

    /// Finding 3364897030: a trimmed clip (source_range start>0,
    /// duration<media) samples WITHIN the trim. With no explicit
    /// `t_samples`, the derived sample times must fall in the clip's
    /// source window, and `run` must succeed.
    #[tokio::test]
    async fn run_samples_within_trimmed_clip_source_range() {
        if !ffmpeg_available() {
            eprintln!("skipping: ffmpeg unavailable");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("raw")).unwrap();
        // 5-second media; clip trimmed to source [2.0, 3.0] (ends at 5.0).
        let ffmpeg = montage_render::ffmpeg_path().unwrap();
        let status = tokio::process::Command::new(ffmpeg)
            .arg("-hide_banner")
            .arg("-loglevel")
            .arg("error")
            .arg("-y")
            .arg("-f")
            .arg("lavfi")
            .arg("-i")
            .arg("color=c=gray:s=320x180:d=5")
            .arg(root.join("raw").join("long.mp4"))
            .status()
            .await
            .unwrap();
        assert!(status.success(), "ffmpeg failed to make long.mp4");
        write_trimmed_timeline(root, "raw/long.mp4", 2.0, 3.0);

        // No explicit t_samples → derived from the clip's source window.
        let mut args = base_args("raw/long.mp4", "trimmed");
        args.t_samples = vec![];
        let out = run(args, ctx_for(root)).await.unwrap();
        let parsed: Value = serde_json::from_str(&out).unwrap();
        // Sampling succeeded within the (valid) trim window.
        let frames = parsed
            .get("frames_sampled")
            .and_then(|v| v.as_u64())
            .unwrap();
        assert!(frames > 0, "expected frames sampled within the trim");

        // The derived sample times for this window are all inside
        // [2.0, 5.0] (start..start+duration).
        let times = sample_times(&[], 3.0, 2.0);
        assert!(
            times.iter().all(|&t| (2.0..=5.0).contains(&t)),
            "sample times escaped the trim: {times:?}"
        );
    }

    #[tokio::test]
    async fn run_records_limitation_for_log_space_without_bundled_shaper() {
        if !ffmpeg_available() {
            eprintln!("skipping: ffmpeg unavailable");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("raw")).unwrap();
        write_valid_cube(root, "luts/show.cube");
        make_clip(&root.join("raw"), "log.mp4", "gray").await;
        write_timeline(root, &["raw/log.mp4"]);

        let mut args = base_args("raw/log.mp4", "no-shaper");
        args.correct = false;
        args.clip_input_space = "canon_log3".into();
        args.look_lut = Some("luts/show.cube".into());
        // This test asserts the "no bundled shaper" outcome, so it must run
        // with MONTAGE_SKILLS_ROOT unset and isolated from setter tests
        // racing it (finding 3364995226).
        let out = {
            let _env = ENV_LOCK.lock().await;
            unsafe {
                std::env::remove_var("MONTAGE_SKILLS_ROOT");
            }
            run(args, ctx_for(root)).await.unwrap()
        };
        let parsed: Value = serde_json::from_str(&out).unwrap();

        let limitations = parsed
            .get("limitations")
            .and_then(|v| v.as_array())
            .unwrap();
        assert!(
            limitations.iter().any(|l| l
                .as_str()
                .is_some_and(|s| s.contains("no bundled shaper for canon_log3"))),
            "missing limitation, got {limitations:?}"
        );
        // No shaper key in the emitted look.
        assert!(
            parsed
                .get("look")
                .and_then(|l| l.get("shaper_lut"))
                .map(|v| v.is_null())
                .unwrap_or(true)
        );
    }

    #[tokio::test]
    async fn run_records_limitation_for_log_clip_with_non_rec709_lut_space() {
        if !ffmpeg_available() {
            eprintln!("skipping: ffmpeg unavailable");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("raw")).unwrap();
        write_valid_cube(root, "luts/show.cube");
        make_clip(&root.join("raw"), "log.mp4", "gray").await;
        write_timeline(root, &["raw/log.mp4"]);

        // Bundled shaper exists, but the look is dci_p3-authored, so the
        // rec709 shaper must NOT be auto-inserted.
        let skills = tempfile::tempdir().unwrap();
        let shaper_dir = skills.path().join("color-corrector").join("shapers");
        std::fs::create_dir_all(&shaper_dir).unwrap();
        std::fs::write(
            shaper_dir.join("arri_logc4_to_rec709_g24.csp"),
            "fake shaper\n",
        )
        .unwrap();
        let _env = ENV_LOCK.lock().await;
        unsafe {
            std::env::set_var("MONTAGE_SKILLS_ROOT", skills.path());
        }

        let mut args = base_args("raw/log.mp4", "mismatch");
        args.correct = false;
        args.clip_input_space = "arri_logc4".into();
        args.lut_input_space = Some("dci_p3".into());
        args.look_lut = Some("luts/show.cube".into());
        let out = run(args, ctx_for(root)).await.unwrap();

        unsafe {
            std::env::remove_var("MONTAGE_SKILLS_ROOT");
        }

        let parsed: Value = serde_json::from_str(&out).unwrap();
        let limitations = parsed
            .get("limitations")
            .and_then(|v| v.as_array())
            .unwrap();
        assert!(
            limitations
                .iter()
                .any(|l| l.as_str().is_some_and(|s| s.contains("not rec709_g24"))),
            "missing mismatch limitation, got {limitations:?}"
        );
        assert!(
            parsed
                .get("look")
                .and_then(|l| l.get("shaper_lut"))
                .map(|v| v.is_null())
                .unwrap_or(true),
            "shaper should not be auto-inserted on space mismatch"
        );
        // No shaper file was copied into the project.
        assert!(!root.join("luts/shapers").exists());
    }
}
