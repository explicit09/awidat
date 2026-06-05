//! `plan_speed_ramp` — turn a ramp intent + clip into a validated,
//! render-ready `Set Time Remap` EDL.
//!
//! The `plan_color_grade` analog for speed: an intent word
//! (`slow_mo_reveal | ramp_to_beat | punch_then | hold_freeze`) plus
//! optional factor/hold/beat controls becomes an eased, monotonic
//! source→timeline curve that satisfies the renderer's `read_time_remap`
//! contract, emitted as exactly ONE atomic `awidat.time_remap` op (never a
//! chain of per-segment `SetSpeed`).
//!
//! Slice 1 surfaces the motion-blur craft law as a recommendation STRING
//! in the JSON summary; an actual shutter-angle render control is deferred.

use std::path::Path;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::awidat_mcp::context::McpToolCtx;
use crate::awidat_mcp::tools::clip_anchor::{normalize_project_rel, resolve_clip_anchor};
use crate::awidat_mcp::tools::color_scopes;
use crate::awidat_mcp::tools::media_probe::probe_duration;
use crate::speed_ramp::{RampPoint, RampRequest, RampShape, plan_ramp_curve};
use awidat_effects::{TIME_REMAP, normalize_params};

const DEFAULT_STEM: &str = "speed-ramp-plan";

/// Arguments to `plan_speed_ramp`.
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct PlanSpeedRampArgs {
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
    /// Ramp intent: `slow_mo_reveal | ramp_to_beat | punch_then |
    /// hold_freeze`. Empty defaults to `slow_mo_reveal`.
    pub shape: String,
    /// Slowest sustained factor in the bell (e.g. 0.3 = 30%). Default 0.3.
    #[serde(default)]
    pub min_factor: Option<f64>,
    /// Fastest sustained factor (e.g. 2.0 = 200%). Default 2.0.
    #[serde(default)]
    pub max_factor: Option<f64>,
    /// Source seconds the held/slow (or freeze) section spans. Default 25%
    /// of the clip's source duration.
    #[serde(default)]
    pub hold_s: Option<f64>,
    /// For `ramp_to_beat`: source-local seconds to align the ramp peak to.
    #[serde(default)]
    pub beat_at_s: Option<f64>,
    /// Eased samples per transition. Default 24, bounded.
    #[serde(default)]
    pub samples: Option<usize>,
    /// Filename stem under `renders/` for the plan artifacts.
    #[serde(default = "default_stem")]
    pub output_stem: String,
}

fn default_stem() -> String {
    DEFAULT_STEM.into()
}

/// Parse a ramp-intent word into a [`RampShape`]. Empty → `SlowMoReveal`
/// (the documented default); any other unknown word fails loud.
fn parse_shape(raw: &str) -> Result<RampShape, String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "" | "slow_mo_reveal" => Ok(RampShape::SlowMoReveal),
        "ramp_to_beat" => Ok(RampShape::RampToBeat),
        "punch_then" => Ok(RampShape::PunchThen),
        "hold_freeze" => Ok(RampShape::HoldFreeze),
        other => Err(format!(
            "plan_speed_ramp: shape {other:?} is not a known ramp shape \
             (slow_mo_reveal | ramp_to_beat | punch_then | hold_freeze)"
        )),
    }
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
            "plan_speed_ramp: {field} must be a plain filename stem under renders/, got {value:?}"
        ));
    }
    Ok(trimmed.to_string())
}

/// Build a [`RampRequest`] from parsed args + probed duration. Optional
/// controls fall back to the corpus-derived defaults.
fn build_request(args: &PlanSpeedRampArgs, shape: RampShape, duration_s: f64) -> RampRequest {
    let mut req = RampRequest::with_defaults(shape, duration_s);
    if let Some(min) = args.min_factor {
        req.min_factor = min;
    }
    if let Some(max) = args.max_factor {
        req.max_factor = max;
    }
    if let Some(hold) = args.hold_s {
        req.hold_s = hold;
    }
    req.beat_at_s = args.beat_at_s;
    if let Some(samples) = args.samples {
        req.samples = samples;
    }
    req
}

/// Assemble the single-op `Set Time Remap` EDL the agent hands to
/// `apply_edl`.
fn assemble_edl(clip: &str, curve: &[Value]) -> Result<String, String> {
    let curve_json = serde_json::to_string(curve)
        .map_err(|e| format!("plan_speed_ramp: serialize curve: {e}"))?;
    Ok(format!(
        "*** Begin EDL\n*** Set Time Remap\n@@ anchor: clip_uuid={clip}\n+ curve_json: {curve_json}\n*** End EDL"
    ))
}

/// Serialize the typed curve points to JSON values (field names already
/// match the renderer's `TimeRemapPoint`).
fn curve_to_json(points: &[RampPoint]) -> Result<Vec<Value>, String> {
    points
        .iter()
        .map(|p| {
            serde_json::to_value(p).map_err(|e| format!("plan_speed_ramp: serialize point: {e}"))
        })
        .collect()
}

/// Surface the motion-blur craft law as a recommendation string (slice 1
/// has no shutter-angle render control yet).
fn motion_blur_recommendation(req: &RampRequest) -> String {
    if req.max_factor > 1.5 {
        format!(
            "ramp peak reaches {:.0}% (> 1.5×); apply ~180° shutter / motion blur to the fastest \
             segment for natural smear — not yet an awidat render control.",
            req.max_factor * 100.0
        )
    } else {
        "ramp peak is mild (<= 1.5×); motion blur is optional — not yet an awidat render control."
            .to_string()
    }
}

/// Run `plan_speed_ramp`. Parses the shape, resolves the asset, probes its
/// duration (fail loud if absent), plans an eased curve, validates it
/// through the registry, writes `renders/<stem>.edl` and
/// `renders/<stem>.json`, and returns the JSON summary as a string.
pub async fn run(args: PlanSpeedRampArgs, ctx: McpToolCtx) -> Result<String, String> {
    let shape = parse_shape(&args.shape)?;
    let output_stem = validate_stem("output_stem", &args.output_stem)?;

    // Normalize the clip to the canonical project-relative shape the
    // timeline target_url / apply_edl anchor uses (strip a leading
    // `./`, reject `..`/absolute/backslashes). A non-canonical spelling
    // like `./raw/foo.mp4` would otherwise be emitted raw and never
    // match `raw/foo.mp4` on the timeline.
    let clip_rel = normalize_project_rel(&args.clip)
        .map_err(|e| format!("plan_speed_ramp: clip {:?}: {e}", args.clip))?;

    // Pick the EDL anchor: an explicit clip_anchor (a NAME, used
    // verbatim, but validated against the timeline) wins; otherwise
    // require the normalized asset to be referenced by exactly one VIDEO
    // timeline clip. Carries the matched clip's source range so the ramp
    // is planned over the clip's trimmed window, not the whole media file.
    let resolved = resolve_clip_anchor(&ctx.project_root, &clip_rel, args.clip_anchor.as_deref())
        .map_err(|e| format!("plan_speed_ramp: {e}"))?;
    let anchor = resolved.anchor;

    let asset_path = color_scopes::resolve_asset_path(&ctx.project_root, &clip_rel)?;

    // Plan the ramp over the CLIP's source duration. When the targeted
    // clip is a trim of a longer media file, the ramp curve must span the
    // clip's actual `[start, start + dur]` window — its source axis is
    // clip-relative (the renderer's contract requires source_time_s=0 at
    // the origin), so we use the trim duration `dur` as the span. Only
    // when no source range is known do we fall back to probing the whole
    // media file's duration.
    let duration_s = match resolved.source_range {
        Some((start, dur)) if start.is_finite() && start >= 0.0 && dur.is_finite() && dur > 0.0 => {
            dur
        }
        _ => probe_duration(&asset_path).await.ok_or_else(|| {
            format!(
                "plan_speed_ramp: could not probe a duration for {}; a ramp needs a real source duration",
                asset_path.display()
            )
        })?,
    };

    let req = build_request(&args, shape, duration_s);
    let points = plan_ramp_curve(&req)?;
    let curve = curve_to_json(&points)?;

    // Registry gate: never emit a curve the renderer would reject.
    let mut params = serde_json::Map::new();
    params.insert("curve".into(), Value::Array(curve.clone()));
    normalize_params(TIME_REMAP, &params)
        .map_err(|e| format!("plan_speed_ramp: curve failed registry validation: {e}"))?;

    let edl = assemble_edl(&anchor, &curve)?;

    // Write artifacts.
    let renders = ctx.project_root.join("renders");
    tokio::fs::create_dir_all(&renders).await.map_err(|e| {
        format!(
            "plan_speed_ramp: failed to create {}: {e}",
            renders.display()
        )
    })?;
    let edl_path = renders.join(format!("{output_stem}.edl"));
    let json_path = renders.join(format!("{output_stem}.json"));

    tokio::fs::write(&edl_path, &edl).await.map_err(|e| {
        format!(
            "plan_speed_ramp: failed to write {}: {e}",
            edl_path.display()
        )
    })?;

    // Report the EFFECTIVE clamped controls actually planned with (the
    // raw request may have out-of-range factor/hold/samples that the math
    // core silently clamps), so the caller sees what was used.
    let effective = req.sanitized();
    let summary = build_summary(&clip_rel, &edl_path, &edl, shape, &effective, &curve);
    let summary_str = serde_json::to_string_pretty(&summary)
        .map_err(|e| format!("plan_speed_ramp: failed to serialize summary: {e}"))?;
    tokio::fs::write(&json_path, &summary_str)
        .await
        .map_err(|e| {
            format!(
                "plan_speed_ramp: failed to write {}: {e}",
                json_path.display()
            )
        })?;

    Ok(summary_str)
}

fn shape_label(shape: RampShape) -> &'static str {
    match shape {
        RampShape::SlowMoReveal => "slow_mo_reveal",
        RampShape::RampToBeat => "ramp_to_beat",
        RampShape::PunchThen => "punch_then",
        RampShape::HoldFreeze => "hold_freeze",
    }
}

fn build_summary(
    clip_rel: &str,
    edl_path: &Path,
    edl: &str,
    shape: RampShape,
    req: &RampRequest,
    curve: &[Value],
) -> Value {
    serde_json::json!({
        "clip": clip_rel,
        "edl_path": edl_path.display().to_string(),
        "edl": edl,
        "shape": shape_label(shape),
        "min_factor": req.min_factor,
        "max_factor": req.max_factor,
        "hold_s": req.hold_s,
        "beat_at_s": req.beat_at_s,
        "samples": req.samples,
        "source_duration_s": req.source_duration_s,
        "curve": curve,
        "motion_blur_recommendation": motion_blur_recommendation(req),
        "rationale": "eased source→timeline curve (smoothstep, not linear), monotonic and origin-anchored, emitted as a single atomic Set Time Remap op",
        "limitations": [
            "motion blur is a recommendation only — no per-clip shutter-angle render control yet",
            "ramp_to_beat aligns to the supplied beat_at_s; no audio beat detection is performed",
        ],
    })
}

pub const DESCRIPTION: &str = "\
Plan a render-ready speed ramp for one clip: turn an intent word \
(slow_mo_reveal | ramp_to_beat | punch_then | hold_freeze) plus optional \
factor/hold/beat controls into an eased, validated Set Time Remap curve \
(source→timeline, monotonic, starts at 0/0). Writes renders/<stem>.edl and \
renders/<stem>.json and surfaces a motion-blur recommendation. After this \
tool, call apply_edl with the returned `edl` text, inspect vedit_diff, then \
render the timeline.";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edl::op::EdlOp;
    use crate::edl::parser::parse;
    use std::path::PathBuf;

    fn ctx_for(root: &Path) -> McpToolCtx {
        McpToolCtx {
            project_root: root.to_path_buf(),
        }
    }

    // Step 10: unknown shape fails loud.
    #[test]
    fn parse_shape_rejects_unknown() {
        assert!(parse_shape("wat").is_err());
        assert!(matches!(parse_shape(""), Ok(RampShape::SlowMoReveal)));
        assert!(matches!(
            parse_shape("punch_then"),
            Ok(RampShape::PunchThen)
        ));
        assert!(matches!(
            parse_shape("HOLD_FREEZE"),
            Ok(RampShape::HoldFreeze)
        ));
    }

    #[test]
    fn validate_stem_rejects_paths() {
        assert!(validate_stem("output_stem", "../escape").is_err());
        assert!(validate_stem("output_stem", "sub/dir").is_err());
        assert!(validate_stem("output_stem", "ok-stem").is_ok());
    }

    fn ffmpeg_available() -> bool {
        awidat_render::ffmpeg_path().is_ok()
    }

    async fn make_clip(dir: &Path, name: &str, color: &str) -> PathBuf {
        let path = dir.join(name);
        let ffmpeg = awidat_render::ffmpeg_path().unwrap();
        let status = tokio::process::Command::new(ffmpeg)
            .arg("-hide_banner")
            .arg("-loglevel")
            .arg("error")
            .arg("-y")
            .arg("-f")
            .arg("lavfi")
            .arg("-i")
            .arg(format!("color=c={color}:s=320x180:d=2"))
            .arg("-frames:v")
            .arg("48")
            .arg(&path)
            .status()
            .await
            .unwrap();
        assert!(status.success(), "ffmpeg failed to make {name}");
        path
    }

    /// Write a minimal `project.otio.json` with one clip per asset path
    /// so `resolve_clip_anchor` can count timeline references.
    fn write_timeline(root: &Path, assets: &[&str]) {
        use awidat_proto::otio::{
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
        use awidat_proto::otio::{
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
        use awidat_proto::otio::{
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

    fn base_args(clip: &str, shape: &str, stem: &str) -> PlanSpeedRampArgs {
        PlanSpeedRampArgs {
            clip: clip.into(),
            clip_anchor: None,
            shape: shape.into(),
            min_factor: None,
            max_factor: None,
            hold_s: None,
            beat_at_s: None,
            samples: None,
            output_stem: stem.into(),
        }
    }

    // Step 10 (run flow): unknown shape fails loud at run().
    #[tokio::test]
    async fn run_unknown_shape_fails_loud() {
        let tmp = tempfile::tempdir().unwrap();
        let args = base_args("raw/x.mp4", "wat", "speed-ramp-plan");
        let err = run(args, ctx_for(tmp.path())).await.unwrap_err();
        assert!(err.contains("not a known ramp shape"), "got: {err}");
    }

    // Step 11: rejects path traversal / absolute clip.
    #[tokio::test]
    async fn run_rejects_path_traversal() {
        let tmp = tempfile::tempdir().unwrap();
        let args = base_args("../escape.mp4", "slow_mo_reveal", "speed-ramp-plan");
        assert!(run(args, ctx_for(tmp.path())).await.is_err());
    }

    // Step 12: end-to-end emits a parseable single SetTimeRemap EDL
    // satisfying the renderer's contract.
    #[tokio::test]
    async fn run_emits_time_remap_edl_end_to_end() {
        if !ffmpeg_available() {
            eprintln!("skipping: ffmpeg unavailable");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("raw")).unwrap();
        make_clip(&root.join("raw"), "shot.mp4", "0x404040").await;
        write_timeline(root, &["raw/shot.mp4"]);

        let args = base_args("raw/shot.mp4", "slow_mo_reveal", "speed-ramp-plan");
        let out = run(args, ctx_for(root)).await.unwrap();
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(
            parsed.get("shape").and_then(Value::as_str),
            Some("slow_mo_reveal")
        );

        let edl_path = root.join("renders/speed-ramp-plan.edl");
        assert!(edl_path.is_file());
        let edl_text = std::fs::read_to_string(&edl_path).unwrap();
        let env = parse(&edl_text).unwrap();
        assert_eq!(env.len(), 1, "expected exactly one op");
        match &env.ops[0] {
            EdlOp::SetTimeRemap { curve, .. } => {
                assert!(curve.len() >= 2, "curve too short");
                let first = curve[0].as_object().unwrap();
                let s0 = first.get("source_time_s").and_then(Value::as_f64).unwrap();
                let t0 = first
                    .get("timeline_time_s")
                    .and_then(Value::as_f64)
                    .unwrap();
                assert!(s0.abs() < 1e-9 && t0.abs() < 1e-9, "first point not 0/0");
                // Source strictly increasing, timeline non-decreasing.
                let pts: Vec<(f64, f64)> = curve
                    .iter()
                    .map(|v| {
                        let o = v.as_object().unwrap();
                        (
                            o.get("source_time_s").and_then(Value::as_f64).unwrap(),
                            o.get("timeline_time_s").and_then(Value::as_f64).unwrap(),
                        )
                    })
                    .collect();
                for w in pts.windows(2) {
                    assert!(w[1].0 > w[0].0, "source not strictly increasing");
                    assert!(w[1].1 >= w[0].1, "timeline decreased");
                }
            }
            other => panic!("want SetTimeRemap, got {other:?}"),
        }
    }

    // Step 13: JSON summary carries the motion-blur recommendation + curve.
    #[tokio::test]
    async fn run_json_summary_has_recommendation_and_curve() {
        if !ffmpeg_available() {
            eprintln!("skipping: ffmpeg unavailable");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("raw")).unwrap();
        make_clip(&root.join("raw"), "shot.mp4", "0x404040").await;
        write_timeline(root, &["raw/shot.mp4"]);

        let mut args = base_args("raw/shot.mp4", "punch_then", "punch-plan");
        args.max_factor = Some(2.5);
        let out = run(args, ctx_for(root)).await.unwrap();
        let parsed: Value = serde_json::from_str(&out).unwrap();

        let rec = parsed.get("motion_blur_recommendation");
        assert!(
            rec.map(|v| v.is_string() && !v.as_str().unwrap_or("").is_empty())
                .unwrap_or(false),
            "missing motion_blur_recommendation"
        );
        assert!(
            rec.and_then(Value::as_str).unwrap().contains("180"),
            "expected a shutter recommendation for a >1.5× peak"
        );
        let curve = parsed.get("curve").and_then(Value::as_array);
        assert!(
            curve.map(|c| c.len() >= 2).unwrap_or(false),
            "missing curve"
        );
    }

    // --- clip-anchor / path normalization (mirrors plan_color_grade) ---

    /// A `./raw/x.mp4` clip must normalize and anchor to `raw/x.mp4` so
    /// it matches the timeline target_url.
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

        let args = base_args("./raw/x.mp4", "slow_mo_reveal", "dot-slash");
        let out = run(args, ctx_for(root)).await.unwrap();
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(
            parsed.get("clip").and_then(Value::as_str),
            Some("raw/x.mp4")
        );
        // The clip PATH normalizes (`./raw/x.mp4` → `raw/x.mp4`), but the
        // emitted anchor is the matched clip's id ("clip-0"), so apply_edl
        // resolves it on the first pass rather than the asset fallback.
        let edl_text = std::fs::read_to_string(root.join("renders/dot-slash.edl")).unwrap();
        assert!(edl_text.contains("clip_uuid=clip-0"), "edl was: {edl_text}");
        assert!(
            !edl_text.contains("./raw/x.mp4"),
            "edl leaked ./ form: {edl_text}"
        );
    }

    /// A backslash / `..` / absolute clip must be rejected with a clear
    /// error before any anchoring or sampling.
    #[tokio::test]
    async fn run_rejects_non_normalizable_clip() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        for bad in ["raw\\x.mp4", "../x.mp4", "raw/../x.mp4", "/tmp/abs.mp4"] {
            let args = base_args(bad, "slow_mo_reveal", "bad-clip");
            let err = run(args, ctx_for(root)).await.unwrap_err();
            assert!(
                err.contains("plan_speed_ramp: clip"),
                "unexpected error for {bad:?}: {err}"
            );
        }
    }

    /// An explicit `clip_anchor` that names a real video clip referencing
    /// the asset is used verbatim as the EDL anchor (it's a name, not a
    /// path — not normalized), even when the asset appears in multiple
    /// timeline clips, and is validated against the timeline.
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
        // Asset appears in TWO clips, one named "Interview Take 2": without
        // clip_anchor this would be ambiguous, but the explicit name
        // resolves it (and is validated to reference the asset).
        write_named_timeline(
            root,
            &[
                ("Interview Take 1", "raw/x.mp4"),
                ("Interview Take 2", "raw/x.mp4"),
            ],
        );

        let mut args = base_args("raw/x.mp4", "slow_mo_reveal", "explicit-anchor");
        args.clip_anchor = Some("Interview Take 2".into());
        run(args, ctx_for(root)).await.unwrap();

        let edl_text = std::fs::read_to_string(root.join("renders/explicit-anchor.edl")).unwrap();
        assert!(
            edl_text.contains("clip_uuid=Interview Take 2"),
            "edl was: {edl_text}"
        );
    }

    /// An asset referenced by two clips with no `clip_anchor` must fail
    /// loud at plan time, naming the count.
    #[tokio::test]
    async fn run_ambiguous_asset_without_anchor_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write_timeline(root, &["raw/clip.mp4", "raw/clip.mp4"]);
        let args = base_args("raw/clip.mp4", "slow_mo_reveal", "ambiguous");
        let err = run(args, ctx_for(root)).await.unwrap_err();
        assert!(
            err.contains("appears in 2 clips") && err.contains("clip_anchor"),
            "unexpected error: {err}"
        );
    }

    /// An asset referenced by zero clips errors.
    #[tokio::test]
    async fn run_unreferenced_asset_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write_timeline(root, &["raw/other.mp4"]);
        let args = base_args("raw/missing.mp4", "slow_mo_reveal", "unreferenced");
        let err = run(args, ctx_for(root)).await.unwrap_err();
        assert!(
            err.contains("no timeline clip references"),
            "unexpected error: {err}"
        );
    }

    /// Finding 3364684041: a trimmed clip plans the ramp over the CLIP's
    /// source duration (the trim window), NOT the full media probe
    /// duration. The curve's total source span equals the clip duration.
    #[tokio::test]
    async fn run_ramp_spans_trimmed_clip_duration_not_full_media() {
        if !ffmpeg_available() {
            eprintln!("skipping: ffmpeg unavailable");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("raw")).unwrap();
        // 5-second media; clip trimmed to source [1.0, 3.0] (dur 2.0).
        let ffmpeg = awidat_render::ffmpeg_path().unwrap();
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
        write_trimmed_timeline(root, "raw/long.mp4", 1.0, 2.0);

        let args = base_args("raw/long.mp4", "slow_mo_reveal", "trimmed-ramp");
        let out = run(args, ctx_for(root)).await.unwrap();
        let parsed: Value = serde_json::from_str(&out).unwrap();

        // The planned source duration is the clip's trim (2.0), not 5.0.
        let dur = parsed
            .get("source_duration_s")
            .and_then(Value::as_f64)
            .unwrap();
        assert!(
            (dur - 2.0).abs() < 1e-6,
            "source_duration_s={dur}, want 2.0"
        );

        // The curve's source axis spans the clip window: clip-relative
        // (starts at 0), ending at ~2.0 — never reaching the full 5.0.
        let curve = parsed.get("curve").and_then(Value::as_array).unwrap();
        let last = curve.last().unwrap().as_object().unwrap();
        let last_source = last.get("source_time_s").and_then(Value::as_f64).unwrap();
        assert!(
            (last_source - 2.0).abs() < 1e-6,
            "curve source span {last_source} should equal the clip duration 2.0, not the full media 5.0"
        );
    }

    /// Finding 3364807592: the JSON summary carries the full EDL envelope
    /// text (alongside `edl_path`), equal to what was written to disk, so
    /// the caller can hand it straight to apply_edl.
    #[tokio::test]
    async fn run_summary_carries_edl_text() {
        if !ffmpeg_available() {
            eprintln!("skipping: ffmpeg unavailable");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("raw")).unwrap();
        make_clip(&root.join("raw"), "shot.mp4", "0x404040").await;
        write_timeline(root, &["raw/shot.mp4"]);

        let args = base_args("raw/shot.mp4", "slow_mo_reveal", "edl-text");
        let out = run(args, ctx_for(root)).await.unwrap();
        let parsed: Value = serde_json::from_str(&out).unwrap();

        let edl_in_summary = parsed.get("edl").and_then(Value::as_str).unwrap();
        let edl_on_disk = std::fs::read_to_string(root.join("renders/edl-text.edl")).unwrap();
        assert_eq!(edl_in_summary, edl_on_disk);
        assert!(
            edl_in_summary.contains("Set Time Remap"),
            "edl was: {edl_in_summary}"
        );
    }

    /// Finding 3364684038: out-of-range controls (max_factor > 8, negative
    /// hold_s, huge samples) are clamped internally; the summary reports
    /// the EFFECTIVE clamped values actually used.
    #[tokio::test]
    async fn run_summary_reports_clamped_controls() {
        if !ffmpeg_available() {
            eprintln!("skipping: ffmpeg unavailable");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("raw")).unwrap();
        make_clip(&root.join("raw"), "shot.mp4", "0x404040").await;
        write_timeline(root, &["raw/shot.mp4"]);

        let mut args = base_args("raw/shot.mp4", "slow_mo_reveal", "clamped");
        args.max_factor = Some(100.0); // clamps to 8.0
        args.min_factor = Some(0.0); // clamps to MIN_LEGAL_FACTOR (0.05)
        args.hold_s = Some(-5.0); // clamps to 0.0
        args.samples = Some(9999); // clamps to MAX_SAMPLES (64)
        let out = run(args, ctx_for(root)).await.unwrap();
        let parsed: Value = serde_json::from_str(&out).unwrap();

        assert_eq!(
            parsed.get("max_factor").and_then(Value::as_f64),
            Some(8.0),
            "max_factor not clamped in summary"
        );
        assert_eq!(
            parsed.get("min_factor").and_then(Value::as_f64),
            Some(0.05),
            "min_factor not clamped in summary"
        );
        assert_eq!(
            parsed.get("hold_s").and_then(Value::as_f64),
            Some(0.0),
            "hold_s not clamped in summary"
        );
        assert_eq!(
            parsed.get("samples").and_then(Value::as_u64),
            Some(64),
            "samples not clamped in summary"
        );
    }
}
