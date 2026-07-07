//! Byte-for-byte golden tests for motion-template expansions.
//!
//! Goldens live in `tests/fixtures/motion-templates/<name>.json` as
//! `serde_json::to_string_pretty` output. Run with `UPDATE_GOLDENS=1`
//! to (re)write the fixture from the current expansion; without the
//! env var, a missing fixture fails with a bootstrap hint and a
//! mismatch fails with the two contents inline for diffing.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::path::{Path, PathBuf};

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
        std::fs::write(&path, actual).expect("write golden fixture");
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
        actual, expected,
        "expansion for {fixture_name} no longer matches the committed golden.\n\
         If this change is intentional, regenerate with UPDATE_GOLDENS=1 and \
         review the diff before committing."
    );
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
