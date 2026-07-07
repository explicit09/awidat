//! Replays `crates/eval/fixtures/animation-vectors.json` through the Rust
//! keyframe evaluator (`montage_render::animation::evaluate_keyframes_with_modes`).
//!
//! The same JSON is replayed on the TypeScript side by
//! `apps/desktop/tests/animation-vectors.test.ts` through `evaluateAnimations`
//! (`apps/desktop/src/timeline/animation.ts`). Nothing else pins these two
//! evaluators together, so a change to either one that shifts a shared case's
//! output fails here or on the TS side instead of shipping silent
//! preview/export (WYSIWYG) drift.
//!
//! Vectors are generated (not hand-written) by
//! `apps/desktop/tests/generate-animation-vectors.mjs`, which calls the same
//! TS evaluator this test's expected values must match, at 1e-6 precision.
//! Rust replay tolerance is looser (1e-4) to allow for cross-language
//! floating point differences without masking real divergence.

use montage_proto::professional::{
    BezierHandles, Easing, ExtrapolationMode, Keyframe, KeyframeInterpolation, SpringParameters,
    TangentMode,
};
use serde::Deserialize;
use std::path::PathBuf;

const TOLERANCE: f64 = 1e-4;

#[derive(Deserialize)]
struct VectorFixture {
    cases: Vec<VectorCase>,
}

#[derive(Deserialize)]
struct VectorCase {
    name: String,
    #[allow(dead_code)]
    param: String,
    keyframes: Vec<RawKeyframe>,
    pre_extrapolation: Option<String>,
    post_extrapolation: Option<String>,
    samples: Vec<VectorSample>,
}

#[derive(Deserialize)]
struct VectorSample {
    t: f64,
    expected: f64,
}

/// Mirrors the JSON keyframe shape emitted by the TS generator. Field names
/// match `montage_proto::professional::Keyframe` directly except that
/// `interpolation` and `easing` are plain strings here (the TS side writes
/// its own string union, which is a superset-compatible spelling of the Rust
/// enums) so we map them explicitly instead of relying on serde to parse the
/// Rust enum's `#[serde(rename_all = "snake_case")]` representation, keeping
/// the mapping visible and easy to audit if either vocabulary drifts.
#[derive(Deserialize)]
struct RawKeyframe {
    time_s: f64,
    value: f64,
    interpolation: String,
    easing: String,
    #[serde(default)]
    bezier: Option<RawBezier>,
    #[serde(default)]
    tangent_mode: Option<String>,
    #[serde(default)]
    spring: Option<RawSpring>,
}

#[derive(Deserialize)]
struct RawBezier {
    out_x: f64,
    out_y: f64,
    in_x: f64,
    in_y: f64,
}

#[derive(Deserialize)]
struct RawSpring {
    mass: f64,
    stiffness: f64,
    damping: f64,
}

fn map_interpolation(name: &str) -> KeyframeInterpolation {
    match name {
        "hold" => KeyframeInterpolation::Hold,
        "step" => KeyframeInterpolation::Step,
        "linear" => KeyframeInterpolation::Linear,
        "bezier" => KeyframeInterpolation::Bezier,
        "spring" => KeyframeInterpolation::Spring,
        other => panic!("unknown interpolation in vector fixture: {other}"),
    }
}

fn map_easing(name: &str) -> Easing {
    match name {
        "linear" => Easing::Linear,
        "ease_in_sine" => Easing::EaseInSine,
        "ease_out_sine" => Easing::EaseOutSine,
        "ease_in_out_sine" => Easing::EaseInOutSine,
        "ease_in" => Easing::EaseIn,
        "ease_out" => Easing::EaseOut,
        "ease_in_out" => Easing::EaseInOut,
        "ease_in_cubic" => Easing::EaseInCubic,
        "ease_out_cubic" => Easing::EaseOutCubic,
        "ease_in_out_cubic" => Easing::EaseInOutCubic,
        "ease_in_quart" => Easing::EaseInQuart,
        "ease_out_quart" => Easing::EaseOutQuart,
        "ease_in_out_quart" => Easing::EaseInOutQuart,
        "ease_in_quint" => Easing::EaseInQuint,
        "ease_out_quint" => Easing::EaseOutQuint,
        "ease_in_out_quint" => Easing::EaseInOutQuint,
        "ease_in_expo" => Easing::EaseInExpo,
        "ease_out_expo" => Easing::EaseOutExpo,
        "ease_in_out_expo" => Easing::EaseInOutExpo,
        "ease_in_circ" => Easing::EaseInCirc,
        "ease_out_circ" => Easing::EaseOutCirc,
        "ease_in_out_circ" => Easing::EaseInOutCirc,
        "ease_in_back" => Easing::EaseInBack,
        "ease_out_back" => Easing::EaseOutBack,
        "ease_in_out_back" => Easing::EaseInOutBack,
        "ease_in_elastic" => Easing::EaseInElastic,
        "ease_out_elastic" => Easing::EaseOutElastic,
        "ease_in_out_elastic" => Easing::EaseInOutElastic,
        "ease_in_bounce" => Easing::EaseInBounce,
        "ease_out_bounce" => Easing::EaseOutBounce,
        "ease_in_out_bounce" => Easing::EaseInOutBounce,
        other => panic!("unknown easing in vector fixture: {other}"),
    }
}

fn map_tangent_mode(name: Option<&str>) -> TangentMode {
    match name {
        None | Some("auto") => TangentMode::Auto,
        Some("aligned") => TangentMode::Aligned,
        Some("broken") => TangentMode::Broken,
        Some("flat") => TangentMode::Flat,
        Some(other) => panic!("unknown tangent_mode in vector fixture: {other}"),
    }
}

fn map_extrapolation(name: Option<&str>) -> ExtrapolationMode {
    match name {
        None | Some("hold") => ExtrapolationMode::Hold,
        Some("linear") => ExtrapolationMode::Linear,
        Some(other) => panic!("unknown extrapolation in vector fixture: {other}"),
    }
}

fn map_keyframe(raw: &RawKeyframe) -> Keyframe {
    Keyframe {
        time_s: raw.time_s,
        value: raw.value,
        interpolation: map_interpolation(&raw.interpolation),
        easing: map_easing(&raw.easing),
        bezier: raw.bezier.as_ref().map(|b| BezierHandles {
            out_x: b.out_x,
            out_y: b.out_y,
            in_x: b.in_x,
            in_y: b.in_y,
        }),
        tangent_mode: map_tangent_mode(raw.tangent_mode.as_deref()),
        spring: raw.spring.as_ref().map(|s| SpringParameters {
            mass: s.mass,
            stiffness: s.stiffness,
            damping: s.damping,
        }),
    }
}

fn load_fixture() -> VectorFixture {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/animation-vectors.json");
    let json = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
    serde_json::from_str(&json)
        .unwrap_or_else(|error| panic!("failed to parse {}: {error}", path.display()))
}

#[test]
fn shared_animation_vectors_match_rust_evaluator() {
    let fixture = load_fixture();
    assert!(
        fixture.cases.len() >= 20,
        "expected at least 20 vector cases, found {}",
        fixture.cases.len()
    );

    let mut checked = 0usize;
    let mut failures = Vec::new();

    for case in &fixture.cases {
        let keyframes: Vec<Keyframe> = case.keyframes.iter().map(map_keyframe).collect();
        let pre = map_extrapolation(case.pre_extrapolation.as_deref());
        let post = map_extrapolation(case.post_extrapolation.as_deref());

        for sample in &case.samples {
            let actual = montage_render::animation::evaluate_keyframes_with_modes(
                &keyframes, sample.t, pre, post,
            );
            checked += 1;
            match actual {
                Some(actual) if (actual - sample.expected).abs() < TOLERANCE => {}
                Some(actual) => failures.push(format!(
                    "{} at t={}: TS expected {}, Rust got {} (diff {})",
                    case.name,
                    sample.t,
                    sample.expected,
                    actual,
                    (actual - sample.expected).abs()
                )),
                None => failures.push(format!(
                    "{} at t={}: TS expected {}, Rust produced None",
                    case.name, sample.t, sample.expected
                )),
            }
        }
    }

    assert!(
        failures.is_empty(),
        "TS/Rust animation evaluator divergence ({checked} samples checked):\n{}",
        failures.join("\n")
    );
}
