//! Byte-for-byte golden tests for motion-template expansions.
//!
//! Goldens live in `tests/fixtures/motion-templates/<name>.json` as
//! `serde_json::to_string_pretty` output. Run with `UPDATE_GOLDENS=1`
//! to (re)write the fixture from the current expansion; without the
//! env var, a missing fixture fails with a bootstrap hint and a
//! mismatch fails with the two contents inline for diffing.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::path::{Path, PathBuf};

use montage_core::motion_scene::{MotionScenePlanRequest, plan_motion_scene_request};
use montage_core::motion_templates::{
    KineticWord, MotionTemplateSpec, TextAnchor, expand_template,
};

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/motion-templates")
}

/// Compare `actual` (pretty JSON) against the committed fixture at
/// `fixture_name`. With `UPDATE_GOLDENS=1` set, writes/overwrites the
/// fixture instead of comparing.
fn assert_matches_golden(fixture_name: &str, actual: &str) {
    let path = fixtures_dir().join(fixture_name);
    if std::env::var("UPDATE_GOLDENS").is_ok() {
        std::fs::create_dir_all(fixtures_dir()).expect("create fixtures dir");
        std::fs::write(&path, with_trailing_newline(actual)).expect("write golden fixture");
        return;
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_else(|_| {
        panic!(
            "golden fixture missing: {}\n\
             Bootstrap it by running this test with UPDATE_GOLDENS=1, e.g.:\n\
             UPDATE_GOLDENS=1 ./scripts/loop-cargo.sh test -p montage-core --test motion_template_goldens\n\
             then review the generated fixture and commit it.",
            path.display()
        )
    });
    assert_eq!(
        with_trailing_newline(actual),
        expected,
        "expansion for {fixture_name} no longer matches the committed golden.\n\
         If this change is intentional, regenerate with UPDATE_GOLDENS=1 and \
         review the diff before committing."
    );
}

/// Golden files are written with a single trailing newline so they are
/// POSIX-clean and diff/editor-friendly. `serde_json::to_string_pretty`
/// output has no trailing newline, so both the writer and the byte
/// comparison append one here to keep file-on-disk and in-memory bytes
/// in agreement.
fn with_trailing_newline(text: &str) -> String {
    if text.ends_with('\n') {
        text.to_string()
    } else {
        format!("{text}\n")
    }
}

#[test]
fn lower_third_matches_golden() {
    let spec = MotionTemplateSpec::LowerThird {
        name: "Ada Lovelace".to_string(),
        role: Some("Mathematician".to_string()),
    };
    let expansion = expand_template(&spec, 5.0, 30.0).expect("lower third should expand");
    let actual =
        serde_json::to_string_pretty(&expansion.layers).expect("serialize expansion layers");
    assert_matches_golden("lower_third.json", &actual);
}

#[test]
fn lower_third_bar_slides_in_from_off_canvas_and_settles_at_final_x() {
    let spec = MotionTemplateSpec::LowerThird {
        name: "Ada Lovelace".to_string(),
        role: Some("Mathematician".to_string()),
    };
    let expansion = expand_template(&spec, 5.0, 30.0).expect("lower third should expand");

    let bar = expansion
        .layers
        .iter()
        .find(|layer| layer.id == "lower-third-bar")
        .expect("bar layer present");
    let final_x = bar
        .params
        .get("x")
        .and_then(serde_json::Value::as_f64)
        .expect("bar has a final x");

    let animations = bar.motion_animations();
    let slide = animations
        .iter()
        .find(|animation| animation.parameter == "overlay.x")
        .expect("bar has an overlay.x animation");

    // Keyframes are [slide-in start, slide-in end, slide-out start,
    // slide-out end]: the slide-in sub-segment is the first pair.
    assert!(
        slide.keyframes.len() >= 2,
        "expected at least a slide-in start/end pair, got {} keyframes",
        slide.keyframes.len()
    );
    let slide_in_start = &slide.keyframes[0];
    let slide_in_end = &slide.keyframes[1];

    assert!(
        slide_in_start.value < 0.0,
        "bar slide-in should start off-canvas (x < 0), got {}",
        slide_in_start.value
    );
    assert_eq!(
        slide_in_end.value, final_x,
        "bar slide-in should end at the final resting x"
    );
}

#[test]
fn lower_third_name_stagger_starts_after_bar_slide_in() {
    let spec = MotionTemplateSpec::LowerThird {
        name: "Ada Lovelace".to_string(),
        role: Some("Mathematician".to_string()),
    };
    let expansion = expand_template(&spec, 5.0, 30.0).expect("lower third should expand");

    let bar = expansion
        .layers
        .iter()
        .find(|layer| layer.id == "lower-third-bar")
        .expect("bar layer present");
    let name_layer = expansion
        .layers
        .iter()
        .find(|layer| layer.id == "lower-third-name")
        .expect("name layer present");

    let bar_slide_start = bar
        .motion_animations()
        .into_iter()
        .find(|animation| animation.parameter == "overlay.x")
        .and_then(|animation| animation.keyframes.first().map(|kf| kf.time_s))
        .expect("bar has a slide-in start time");

    // The name layer starts later in the scene (from_s stagger) and,
    // once converted to a shared scene timeline, its slide-in keyframe
    // fires strictly after the bar's slide-in keyframe.
    let name_slide_start_scene_s = name_layer.from_s
        + name_layer
            .motion_animations()
            .into_iter()
            .find(|animation| animation.parameter == "overlay.x")
            .and_then(|animation| animation.keyframes.first().map(|kf| kf.time_s))
            .expect("name layer has a slide-in start time");
    let bar_slide_start_scene_s = bar.from_s + bar_slide_start;

    assert!(
        name_slide_start_scene_s > bar_slide_start_scene_s,
        "name layer slide-in ({name_slide_start_scene_s}) should start after the bar's ({bar_slide_start_scene_s})"
    );
}

fn kinetic_text_spec() -> MotionTemplateSpec {
    MotionTemplateSpec::KineticText {
        words: vec![
            KineticWord {
                text: "Ship".to_string(),
                at_s: 0.0,
                hold_s: 0.4,
            },
            KineticWord {
                text: "it".to_string(),
                at_s: 0.25,
                hold_s: 0.4,
            },
            KineticWord {
                text: "today".to_string(),
                at_s: 0.5,
                hold_s: 0.4,
            },
        ],
        anchor: TextAnchor::LowerCenter,
    }
}

#[test]
fn kinetic_text_matches_golden() {
    let spec = kinetic_text_spec();
    let expansion = expand_template(&spec, 3.0, 30.0).expect("kinetic text should expand");
    let actual =
        serde_json::to_string_pretty(&expansion.layers).expect("serialize expansion layers");
    assert_matches_golden("kinetic_text.json", &actual);
}

#[test]
fn kinetic_text_from_s_strictly_increasing_per_word() {
    let spec = kinetic_text_spec();
    let expansion = expand_template(&spec, 3.0, 30.0).expect("kinetic text should expand");

    let from_times: Vec<f64> = expansion.layers.iter().map(|layer| layer.from_s).collect();
    for window in from_times.windows(2) {
        assert!(
            window[1] > window[0],
            "expected strictly increasing from_s across words, got {from_times:?}"
        );
    }
}

#[test]
fn kinetic_text_word_boxes_are_non_overlapping() {
    let spec = kinetic_text_spec();
    let expansion = expand_template(&spec, 3.0, 30.0).expect("kinetic text should expand");

    let boxes: Vec<(f64, f64)> = expansion
        .layers
        .iter()
        .map(|layer| {
            let x = layer
                .params
                .get("x")
                .and_then(serde_json::Value::as_f64)
                .expect("word layer has x");
            let width = layer
                .params
                .get("width")
                .and_then(serde_json::Value::as_f64)
                .expect("word layer has width");
            (x, width)
        })
        .collect();

    for window in boxes.windows(2) {
        let (x0, width0) = window[0];
        let (x1, _width1) = window[1];
        assert!(
            x0 + width0 <= x1,
            "expected word boxes to not overlap: word ends at {}, next word starts at {}",
            x0 + width0,
            x1
        );
    }
}

fn highlight_box_pulse_spec() -> MotionTemplateSpec {
    MotionTemplateSpec::HighlightBox {
        x: 0.2,
        y: 0.25,
        width: 0.35,
        height: 0.3,
        pulse: true,
    }
}

#[test]
fn highlight_box_matches_golden() {
    let spec = highlight_box_pulse_spec();
    let expansion = expand_template(&spec, 3.0, 30.0).expect("highlight box should expand");
    let actual =
        serde_json::to_string_pretty(&expansion.layers).expect("serialize expansion layers");
    assert_matches_golden("highlight_box.json", &actual);
}

#[test]
fn highlight_box_no_pulse_matches_golden() {
    let spec = MotionTemplateSpec::HighlightBox {
        x: 0.2,
        y: 0.25,
        width: 0.35,
        height: 0.3,
        pulse: false,
    };
    let expansion = expand_template(&spec, 3.0, 30.0).expect("highlight box should expand");
    let actual =
        serde_json::to_string_pretty(&expansion.layers).expect("serialize expansion layers");
    assert_matches_golden("highlight_box_no_pulse.json", &actual);
}

#[test]
fn highlight_box_rejects_out_of_range_fractions() {
    let spec = MotionTemplateSpec::HighlightBox {
        x: 1.2,
        y: 0.25,
        width: 0.35,
        height: 0.3,
        pulse: false,
    };
    let result = expand_template(&spec, 3.0, 30.0);
    assert!(result.is_err());
}

#[test]
fn highlight_box_rejects_non_positive_width_or_height() {
    let spec = MotionTemplateSpec::HighlightBox {
        x: 0.2,
        y: 0.25,
        width: 0.0,
        height: 0.3,
        pulse: false,
    };
    let result = expand_template(&spec, 3.0, 30.0);
    assert!(result.is_err());
}

#[test]
fn highlight_box_pulse_uses_deterministic_scale_keyframes() {
    let spec = highlight_box_pulse_spec();
    let expansion = expand_template(&spec, 3.0, 30.0).expect("highlight box should expand");
    let layer = &expansion.layers[0];
    let animations = layer.motion_animations();

    let scale = animations
        .iter()
        .find(|a| a.parameter == "overlay.scale")
        .expect("pulsing box has an overlay.scale animation");
    assert_eq!(
        scale.keyframes.len(),
        5,
        "expected 5 explicit pulse keyframes (1.0 -> 1.04 -> 1.0 -> 1.04 -> 1.0), got {}",
        scale.keyframes.len()
    );
    let values: Vec<f64> = scale.keyframes.iter().map(|kf| kf.value).collect();
    assert_eq!(values, vec![1.0, 1.04, 1.0, 1.04, 1.0]);
    for window in scale.keyframes.windows(2) {
        assert!(
            window[1].time_s > window[0].time_s,
            "pulse keyframe times must be strictly increasing, got {:?}",
            scale
                .keyframes
                .iter()
                .map(|kf| kf.time_s)
                .collect::<Vec<_>>()
        );
    }

    let opacity = animations
        .iter()
        .find(|a| a.parameter == "overlay.opacity")
        .expect("box has an overlay.opacity animation");
    assert_eq!(opacity.keyframes.first().unwrap().value, 0.0);
    let pop_in_end = opacity
        .keyframes
        .iter()
        .find(|kf| kf.time_s > 0.0)
        .expect("opacity has a pop-in end keyframe");
    assert_eq!(pop_in_end.value, 0.85);
    assert_eq!(pop_in_end.time_s, 0.15);
}

#[test]
fn highlight_box_no_pulse_has_no_scale_animation() {
    let spec = MotionTemplateSpec::HighlightBox {
        x: 0.2,
        y: 0.25,
        width: 0.35,
        height: 0.3,
        pulse: false,
    };
    let expansion = expand_template(&spec, 3.0, 30.0).expect("highlight box should expand");
    let animations = expansion.layers[0].motion_animations();
    assert!(
        animations.iter().all(|a| a.parameter != "overlay.scale"),
        "non-pulsing box should not have an overlay.scale animation"
    );
}

fn progress_bar_spec() -> MotionTemplateSpec {
    MotionTemplateSpec::ProgressBar {
        from: 0.1,
        to: 0.75,
        y: 0.9,
        color: None,
    }
}

#[test]
fn progress_bar_matches_golden() {
    let spec = progress_bar_spec();
    let expansion = expand_template(&spec, 3.0, 30.0).expect("progress bar should expand");
    let actual =
        serde_json::to_string_pretty(&expansion.layers).expect("serialize expansion layers");
    assert_matches_golden("progress_bar.json", &actual);
}

#[test]
fn progress_bar_custom_color_matches_golden() {
    let spec = MotionTemplateSpec::ProgressBar {
        from: 0.0,
        to: 0.6,
        y: 0.9,
        color: Some("#00FF00".to_string()),
    };
    let expansion = expand_template(&spec, 3.0, 30.0).expect("progress bar should expand");
    let actual =
        serde_json::to_string_pretty(&expansion.layers).expect("serialize expansion layers");
    assert_matches_golden("progress_bar_custom_color.json", &actual);
}

#[test]
fn progress_bar_rejects_from_not_less_than_to() {
    let spec = MotionTemplateSpec::ProgressBar {
        from: 0.75,
        to: 0.75,
        y: 0.9,
        color: None,
    };
    let result = expand_template(&spec, 3.0, 30.0);
    assert!(result.is_err());
}

#[test]
fn progress_bar_rejects_out_of_range_to() {
    let spec = MotionTemplateSpec::ProgressBar {
        from: 0.1,
        to: 1.2,
        y: 0.9,
        color: None,
    };
    let result = expand_template(&spec, 3.0, 30.0);
    assert!(result.is_err());
}

#[test]
fn progress_bar_is_left_anchored_with_static_width_and_linear_scale() {
    let spec = progress_bar_spec();
    let expansion = expand_template(&spec, 3.0, 30.0).expect("progress bar should expand");
    let layer = &expansion.layers[0];

    let anchor_x = layer
        .params
        .get("anchor_x")
        .and_then(serde_json::Value::as_f64)
        .expect("progress bar has anchor_x");
    assert_eq!(anchor_x, 0.0, "progress bar must be left-anchored");

    let width = layer
        .params
        .get("width")
        .and_then(serde_json::Value::as_f64)
        .expect("progress bar has width");
    assert_eq!(width, 0.75, "static width should equal `to`");

    let height = layer
        .params
        .get("height")
        .and_then(serde_json::Value::as_f64)
        .expect("progress bar has height");
    assert_eq!(height, 0.012, "progress bar should be thin");

    let animations = layer.motion_animations();
    let scale = animations
        .iter()
        .find(|a| a.parameter == "overlay.scale")
        .expect("progress bar has an overlay.scale animation");
    assert_eq!(
        scale.keyframes.len(),
        2,
        "linear scale is a two-keyframe ramp"
    );
    assert_eq!(scale.keyframes[0].time_s, 0.0);
    assert_eq!(scale.keyframes[0].value, 0.1 / 0.75);
    assert_eq!(scale.keyframes[1].value, 1.0);
}

// =====================================================================
// Stage-harness scene sync tests (Phase-2 Task 7)
//
// The Playwright stage harness (`apps/desktop/tests/stage-harness.mjs`)
// renders committed scene fixtures in
// `apps/desktop/public/fixtures/stage/scene-*.json` — the
// SNAPSHOT-LEVEL mirror of expander output, i.e. the `Preview*`
// overlay shapes `StageHarness.tsx` consumes. These tests re-derive
// each harness scene from `expand_template` plus the same field
// mapping the Tauri snapshot lowering applies, serialize, and
// byte-compare against the committed fixture so the harness scenes
// cannot silently drift from the expander. `UPDATE_GOLDENS=1`
// regenerates the fixtures.
//
// SOURCE OF TRUTH for the field mapping is the Tauri snapshot
// lowering in `apps/desktop/src-tauri/src/commands/timeline.rs`
// (`motion_scene_title_for_protocol`, `motion_scene_shape_for_protocol`,
// `motion_scene_layer_animations_for_protocol`,
// `motion_scene_preview_track`) composed with the desktop preview
// overlay construction in
// `apps/desktop/src/media/stage/motionScene.tsx`
// (`activeMotionSceneOverlays`) and `titles.tsx` (`titleOverlayBox`,
// `titlePosition`, `titleAlign`, `titleReveal`). The mapping code
// below is a TEST-ONLY mirror of that lowering; if it and
// `timeline.rs` disagree, `timeline.rs` wins and this file must be
// updated.
//
// TIME BASE: expander keyframe times are LAYER-LOCAL seconds
// (relative to the layer's own `from_s`), and the Stage preview
// evaluates overlay animations at `timelineTime - overlay.startS`
// (see `motionScene.tsx` / `titles.tsx` `evaluateAnimations` call
// sites), so keyframe times are copied VERBATIM — no conversion.
// Overlay windows are timeline seconds:
// `startS = scene.start_s + layer.from_s` and
// `endS = startS + layer.duration_s` (mirroring
// `motion_scene_preview_track`'s `track_start_s`); harness scenes
// anchor at `scene.start_s = 0`, so `startS = from_s`.
//
// Field mapping table (expander layer -> harness scene JSON):
//   Text layer   -> {"kind":"title"}:
//     params.text                        -> text (required)
//     params.position (default "center") -> position, then
//                                           titlePosition(): top|bottom else center
//     params.font_size (default 64)      -> fontSize
//     params.color (default "#FFFFFF")   -> color
//     params.font_weight|weight
//       (default "normal")               -> fontWeight ("bold" else "normal")
//     params.animation slide_in|slide_out
//       else "none"                      -> animation
//     params.reveal (default "none")     -> reveal (typewriter|word|line else none)
//     params.{x,y,width} + params.align  -> box {x, y, width, align}
//       (scene_normalized_param: |v| <= 1 passes through — template
//        params are normalized fractions, so passthrough; align via
//        titleAlign(): left|right else center)
//   Solid/Shape layer -> {"kind":"shape"}:
//     MotionSceneTransform::from_layer_params -> x, y, width, height,
//       opacity (clamped 0..=1), scale, anchorX, anchorY, rotationDeg
//     params.color (default "#FFFFFF")   -> color; shape is "rect"
//   Both kinds:
//     motion_animations_with_scene_fade(peak_opacity) filtered by
//       is_runtime_clip_parameter        -> animations[] (peak_opacity:
//       Text -> 1.0, else transform.opacity; this is what synthesizes
//       the default enter/exit opacity fade on the lower-third bar)
//     animation id                       -> "<sceneId>:<layerId>:<parameter>"
//       (mirrors the lowering's "{clip_uuid}:{parameter}")
//     keyframes: time_s -> timeS (layer-local, verbatim), value,
//       interpolation + easing (serde snake_case names — identical to
//       timeline.rs interpolation_name/easing_name), spring passthrough
//   Layer order: sorted by z_index, stable (sorted_motion_scene_layers)
// =====================================================================

use montage_proto::professional::{
    MotionSceneLayer, MotionSceneLayerKind, MotionSceneTransform, is_runtime_clip_parameter,
};

#[derive(serde::Serialize)]
struct HarnessSpring {
    mass: f64,
    stiffness: f64,
    damping: f64,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct HarnessKeyframe {
    time_s: f64,
    value: f64,
    interpolation: String,
    easing: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    spring: Option<HarnessSpring>,
}

#[derive(serde::Serialize)]
struct HarnessAnimation {
    id: String,
    parameter: String,
    keyframes: Vec<HarnessKeyframe>,
}

#[derive(serde::Serialize)]
struct HarnessTitleBox {
    x: f64,
    y: f64,
    width: Option<f64>,
    align: String,
}

#[derive(serde::Serialize)]
#[serde(tag = "kind")]
enum HarnessLayer {
    #[serde(rename = "title", rename_all = "camelCase")]
    Title {
        key: String,
        start_s: f64,
        end_s: f64,
        text: String,
        position: String,
        font_size: u32,
        color: String,
        font_weight: String,
        animation: String,
        reveal: String,
        #[serde(rename = "box")]
        title_box: HarnessTitleBox,
        animations: Vec<HarnessAnimation>,
    },
    #[serde(rename = "shape", rename_all = "camelCase")]
    Shape {
        key: String,
        start_s: f64,
        end_s: f64,
        shape: String,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
        color: String,
        opacity: f64,
        scale: f64,
        anchor_x: f64,
        anchor_y: f64,
        rotation_deg: f64,
        animations: Vec<HarnessAnimation>,
    },
}

#[derive(serde::Serialize)]
struct HarnessScene {
    layers: Vec<HarnessLayer>,
}

fn layer_str<'a>(layer: &'a MotionSceneLayer, key: &str) -> Option<&'a str> {
    layer.params.get(key).and_then(serde_json::Value::as_str)
}

fn layer_f64(layer: &MotionSceneLayer, key: &str) -> Option<f64> {
    layer.params.get(key).and_then(serde_json::Value::as_f64)
}

fn layer_u32(layer: &MotionSceneLayer, key: &str) -> Option<u32> {
    layer
        .params
        .get(key)
        .and_then(serde_json::Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
}

/// serde snake_case name of a proto enum value — identical strings to
/// `timeline.rs` `interpolation_name`/`easing_name`.
fn enum_name<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .expect("serialize proto enum")
        .as_str()
        .expect("proto enum serializes as a string")
        .to_string()
}

/// Mirror of `motion_scene_layer_animations_for_protocol` (timeline.rs):
/// scene-fade-augmented layer animations, runtime-parameter filtered,
/// keyframe times layer-local verbatim.
fn harness_animations(layer: &MotionSceneLayer, layer_key: &str) -> Vec<HarnessAnimation> {
    let peak_opacity = match layer.kind {
        MotionSceneLayerKind::Text => 1.0,
        _ => MotionSceneTransform::from_layer_params(&layer.params).opacity,
    };
    layer
        .motion_animations_with_scene_fade(peak_opacity)
        .into_iter()
        .filter(|animation| is_runtime_clip_parameter(&animation.parameter))
        .map(|animation| HarnessAnimation {
            id: format!("{layer_key}:{}", animation.parameter),
            keyframes: animation
                .keyframes
                .iter()
                .map(|kf| HarnessKeyframe {
                    time_s: kf.time_s,
                    value: kf.value,
                    interpolation: enum_name(&kf.interpolation),
                    easing: enum_name(&kf.easing),
                    spring: kf.spring.map(|spring| HarnessSpring {
                        mass: spring.mass,
                        stiffness: spring.stiffness,
                        damping: spring.damping,
                    }),
                })
                .collect(),
            parameter: animation.parameter,
        })
        .collect()
}

/// Mirror of `motion_scene_title_for_protocol` (timeline.rs) composed
/// with `activeMotionSceneOverlays`/`titleOverlayBox`/`titlePosition`/
/// `titleAlign`/`titleReveal` (desktop preview).
fn harness_title_layer(
    layer: &MotionSceneLayer,
    key: String,
    start_s: f64,
    end_s: f64,
) -> HarnessLayer {
    let animations = harness_animations(layer, &key);
    let position = layer_str(layer, "position").unwrap_or("center");
    let position = if position == "top" || position == "bottom" {
        position
    } else {
        "center"
    };
    let font_weight = layer_str(layer, "font_weight")
        .or_else(|| layer_str(layer, "weight"))
        .unwrap_or("normal");
    let animation = layer_str(layer, "animation")
        .filter(|name| matches!(*name, "slide_in" | "slide_out"))
        .unwrap_or("none");
    let reveal = layer_str(layer, "reveal")
        .filter(|name| matches!(*name, "typewriter" | "word" | "line"))
        .unwrap_or("none");
    let align = layer_str(layer, "align")
        .filter(|name| matches!(*name, "left" | "right"))
        .unwrap_or("center");
    // scene_normalized_param: template params are normalized fractions
    // (|v| <= 1), so they pass through undivided.
    HarnessLayer::Title {
        text: layer_str(layer, "text")
            .expect("text layer has text")
            .to_string(),
        position: position.to_string(),
        font_size: layer_u32(layer, "font_size").unwrap_or(64),
        color: layer_str(layer, "color").unwrap_or("#FFFFFF").to_string(),
        font_weight: if font_weight == "bold" {
            "bold"
        } else {
            "normal"
        }
        .to_string(),
        animation: animation.to_string(),
        reveal: reveal.to_string(),
        title_box: HarnessTitleBox {
            x: layer_f64(layer, "x").expect("text layer has x"),
            y: layer_f64(layer, "y").expect("text layer has y"),
            width: layer_f64(layer, "width").filter(|width| *width > 0.0),
            align: align.to_string(),
        },
        key,
        start_s,
        end_s,
        animations,
    }
}

/// Mirror of `motion_scene_shape_for_protocol` (timeline.rs) composed
/// with `activeMotionSceneOverlays`' shape branch (clampOpacity is a
/// no-op here — `MotionSceneTransform` already clamps to 0..=1).
fn harness_shape_layer(
    layer: &MotionSceneLayer,
    key: String,
    start_s: f64,
    end_s: f64,
) -> HarnessLayer {
    let animations = harness_animations(layer, &key);
    let transform = MotionSceneTransform::from_layer_params(&layer.params);
    HarnessLayer::Shape {
        shape: "rect".to_string(),
        x: transform.x,
        y: transform.y,
        width: transform.width,
        height: transform.height,
        color: layer_str(layer, "color").unwrap_or("#FFFFFF").to_string(),
        opacity: transform.opacity,
        scale: transform.scale,
        anchor_x: transform.anchor_x,
        anchor_y: transform.anchor_y,
        rotation_deg: transform.rotation_deg,
        key,
        start_s,
        end_s,
        animations,
    }
}

/// Lower an expansion's layers into the harness scene document.
/// Mirrors `motion_scene_preview_track`: layers sorted by z_index
/// (stable), `clip_uuid = "<sceneId>:<layerId>"`, window at
/// `scene.start_s (= 0 in the harness) + from_s`.
fn harness_scene(scene_id: &str, layers: &[MotionSceneLayer]) -> HarnessScene {
    let mut sorted: Vec<&MotionSceneLayer> = layers.iter().collect();
    sorted.sort_by_key(|layer| layer.z_index);
    let layers = sorted
        .into_iter()
        .map(|layer| {
            let key = format!("{scene_id}:{}", layer.id);
            let start_s = layer.from_s;
            let end_s = layer.from_s + layer.duration_s;
            match layer.kind {
                MotionSceneLayerKind::Text => harness_title_layer(layer, key, start_s, end_s),
                MotionSceneLayerKind::Solid | MotionSceneLayerKind::Shape => {
                    harness_shape_layer(layer, key, start_s, end_s)
                }
                other => panic!("harness scenes do not lower {other:?} layers"),
            }
        })
        .collect();
    HarnessScene { layers }
}

fn harness_fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../apps/desktop/public/fixtures/stage")
}

/// Compare `actual` (pretty JSON) against the committed harness scene
/// fixture. `UPDATE_GOLDENS=1` writes/overwrites the fixture instead.
fn assert_matches_harness_golden(fixture_name: &str, actual: &str) {
    let path = harness_fixture_dir().join(fixture_name);
    if std::env::var("UPDATE_GOLDENS").is_ok() {
        std::fs::write(&path, with_trailing_newline(actual)).expect("write harness scene fixture");
        return;
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_else(|_| {
        panic!(
            "harness scene fixture missing: {}\n\
             Bootstrap it by running this test with UPDATE_GOLDENS=1, e.g.:\n\
             UPDATE_GOLDENS=1 ./scripts/loop-cargo.sh test -p montage-core --test motion_template_goldens\n\
             then review the generated fixture and commit it.",
            path.display()
        )
    });
    assert_eq!(
        with_trailing_newline(actual),
        expected,
        "harness scene {fixture_name} no longer matches the expander + snapshot lowering.\n\
         If the expander change is intentional, regenerate with UPDATE_GOLDENS=1, re-bootstrap \n\
         the SSIM goldens (delete the fixture's apps/desktop/tests/fixtures/stage-golden/*.png \n\
         and rerun the harness), and review both diffs before committing."
    );
}

/// Lower-third harness scene: same spec as `lower_third_matches_golden`
/// (Ada Lovelace / Mathematician, 5s scene). Harness screenshots at
/// t=0.4 (bar+text slide-in mid-flight) and t=2.0 (settled).
///
/// Built via `plan_motion_scene_request` in template mode — the SAME
/// production entry point agents call — so the harness scene tracks
/// everything production emits, including `fit_scene_text_layers`
/// auto-shrinking font sizes to fit their boxes. Deriving it from the
/// raw `expand_template` output would silently skip that fitter step.
#[test]
fn harness_scene_lower_third_matches_expander() {
    let plan = plan_motion_scene_request(&MotionScenePlanRequest {
        request: "lower third for the guest".into(),
        scene_id: Some("lower-third".into()),
        duration_s: Some(5.0),
        fps: Some(30.0),
        template: Some("lower_third".into()),
        name: Some("Ada Lovelace".into()),
        role: Some("Mathematician".into()),
        ..MotionScenePlanRequest::default()
    })
    .expect("lower third should plan");
    let scene = harness_scene(&plan.scene.id, &plan.scene.layers);
    let actual = serde_json::to_string_pretty(&scene).expect("serialize harness scene");
    assert_matches_harness_golden("scene-lower-third.json", &actual);
}

/// Progress-bar harness scene: the cross-renderer export-parity gate's
/// scene of record (see `export_parity_gate.rs`). Deliberately
/// SHAPES-ONLY — no text layer — because the parity gate SSIM-compares
/// the ffmpeg export against the browser-rendered stage golden, and
/// font rasterization differences between CSS and drawtext would
/// swamp the comparison. The bar's `overlay.scale` ramp is exactly the
/// geometry-animation channel the render lowering ships via lavfi
/// solids (PR #103), so this scene pins preview and export to the same
/// pixels for that path. 3s duration matches the harness fixture clip.
///
/// Built via `plan_motion_scene_request` in template mode (the
/// production entry point), same as the other harness scenes.
#[test]
fn harness_scene_progress_bar_matches_expander() {
    let plan = plan_motion_scene_request(&MotionScenePlanRequest {
        request: "progress bar for the challenge".into(),
        scene_id: Some("progress-bar".into()),
        duration_s: Some(3.0),
        fps: Some(30.0),
        width: Some(1280),
        height: Some(720),
        template: Some("progress_bar".into()),
        progress: Some((0.2, 0.9, 0.86)),
        ..MotionScenePlanRequest::default()
    })
    .expect("progress bar should plan");
    let scene = harness_scene(&plan.scene.id, &plan.scene.layers);
    let actual = serde_json::to_string_pretty(&scene).expect("serialize harness scene");
    assert_matches_harness_golden("scene-progress-bar.json", &actual);
}

/// Kinetic-text harness scene: words staggered so the harness catches
/// distinct states at its two screenshot times — at t=0.5 "Ship" is at
/// full hold (its exit fade only starts at t=0.54) and "it" is
/// mid-pop-in ("today" not yet started, begins at t=0.9); at t=1.5 only
/// "today" remains (settled, holds to scene end).
///
/// Built via `plan_motion_scene_request` in template mode (the
/// production entry point) so the harness scene tracks the fitter and
/// the production defaults. `anchor` is passed explicitly as
/// `"lower_center"` — the value the committed SSIM goldens pin — so
/// that pinned behavior stays reachable now that `anchor` is a real
/// input (it was previously unreachable from production, which
/// hardcoded `Center`).
#[test]
fn harness_scene_kinetic_text_matches_expander() {
    let plan = plan_motion_scene_request(&MotionScenePlanRequest {
        request: "kinetic text of the key phrase".into(),
        scene_id: Some("kinetic-text".into()),
        duration_s: Some(4.0),
        fps: Some(30.0),
        template: Some("kinetic_text".into()),
        words: vec![
            ("Ship".to_string(), 0.0, 0.6),
            ("it".to_string(), 0.4, 0.6),
            ("today".to_string(), 0.9, 0.0),
        ],
        anchor: Some("lower_center".into()),
        ..MotionScenePlanRequest::default()
    })
    .expect("kinetic text should plan");
    let scene = harness_scene(&plan.scene.id, &plan.scene.layers);
    let actual = serde_json::to_string_pretty(&scene).expect("serialize harness scene");
    assert_matches_harness_golden("scene-kinetic-text.json", &actual);
}
