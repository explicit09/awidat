//! Byte-for-byte golden tests for motion-template expansions.
//!
//! Goldens live in `tests/fixtures/motion-templates/<name>.json` as
//! `serde_json::to_string_pretty` output. Run with `UPDATE_GOLDENS=1`
//! to (re)write the fixture from the current expansion; without the
//! env var, a missing fixture fails with a bootstrap hint and a
//! mismatch fails with the two contents inline for diffing.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::path::{Path, PathBuf};

use montage_core::motion_templates::{MotionTemplateSpec, expand_template};

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
