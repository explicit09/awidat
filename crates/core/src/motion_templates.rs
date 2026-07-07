//! Motion-template spec types and the pure expander that lowers a
//! high-level template spec into `MotionSceneLayer`s.
//!
//! Templates are agent-facing shorthands ("lower third", "kinetic text
//! word cascade", ...) that expand into the same `MotionScene`/
//! `MotionSceneLayer` primitives `plan_motion_scene` produces by hand.
//! Layer ids are deterministic (`lower-third-bar`, `kinetic-word-<i>`,
//! ...) so replanning the same scene id is idempotent through
//! `apply_edl`'s replace-by-id contract instead of accumulating
//! duplicate layers.
//!
//! Only [`MotionTemplateSpec::LowerThird`] is expanded so far; the
//! remaining variants are wired into the enum ahead of their expanders
//! landing in later tasks.

use std::collections::BTreeMap;

use montage_proto::professional::{MotionSceneLayer, MotionSceneLayerKind};

/// High-level motion-template request. Each variant expands into one or
/// more [`MotionSceneLayer`]s via [`expand_template`].
#[derive(Debug, Clone, PartialEq)]
pub enum MotionTemplateSpec {
    /// Lower-third name/role card.
    LowerThird {
        /// Name displayed in the primary line.
        name: String,
        /// Optional role/title displayed below the name.
        role: Option<String>,
    },
    /// Word-by-word kinetic text cascade.
    KineticText {
        /// Words in display order, each with its own timing.
        words: Vec<KineticWord>,
        /// Screen anchor for the text block.
        anchor: TextAnchor,
    },
    /// Highlight box drawn around a region of interest.
    HighlightBox {
        /// Left offset as a fraction of canvas width.
        x: f64,
        /// Top offset as a fraction of canvas height.
        y: f64,
        /// Box width as a fraction of canvas width.
        width: f64,
        /// Box height as a fraction of canvas height.
        height: f64,
        /// Whether the box pulses to draw attention.
        pulse: bool,
    },
    /// Horizontal progress bar animating between two values.
    ProgressBar {
        /// Starting fraction in 0..=1.
        from: f64,
        /// Ending fraction in 0..=1.
        to: f64,
        /// Vertical position as a fraction of canvas height.
        y: f64,
        /// Optional bar color; defaults to the template's own choice.
        color: Option<String>,
    },
}

/// One word in a [`MotionTemplateSpec::KineticText`] cascade.
#[derive(Debug, Clone, PartialEq)]
pub struct KineticWord {
    /// Word text.
    pub text: String,
    /// Time the word appears, in scene-local seconds.
    pub at_s: f64,
    /// How long the word holds on screen, in seconds.
    pub hold_s: f64,
}

/// Screen anchor for a [`MotionTemplateSpec::KineticText`] block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextAnchor {
    /// Lower-left corner.
    LowerLeft,
    /// Frame center.
    Center,
    /// Lower-center, horizontally centered near the bottom.
    LowerCenter,
}

/// Result of expanding a [`MotionTemplateSpec`] into concrete layers.
#[derive(Debug, Clone, PartialEq)]
pub struct TemplateExpansion {
    /// Expanded motion-scene layers, in draw order.
    pub layers: Vec<MotionSceneLayer>,
    /// Short rationale for review.
    pub rationale: String,
}

/// Expand a [`MotionTemplateSpec`] into concrete `MotionSceneLayer`s.
///
/// `scene_duration_s` and `fps` bound and time the expansion; layers
/// must fit within the scene duration for `MotionScene::validate()` to
/// accept them. Layer ids are deterministic per template so replanning
/// the same scene id replaces layers idempotently.
pub fn expand_template(
    spec: &MotionTemplateSpec,
    scene_duration_s: f64,
    fps: f64,
) -> Result<TemplateExpansion, String> {
    if !scene_duration_s.is_finite() || scene_duration_s <= 0.0 {
        return Err(format!(
            "expand_template: scene_duration_s must be positive, got {scene_duration_s}"
        ));
    }
    if !fps.is_finite() || fps <= 0.0 {
        return Err(format!("expand_template: fps must be positive, got {fps}"));
    }
    match spec {
        MotionTemplateSpec::LowerThird { name, role } => {
            expand_lower_third(name, role.as_deref(), scene_duration_s)
        }
        MotionTemplateSpec::KineticText { .. } => {
            Err("expand_template: KineticText is not implemented yet (Task 2+)".to_string())
        }
        MotionTemplateSpec::HighlightBox { .. } => {
            Err("expand_template: HighlightBox is not implemented yet (Task 2+)".to_string())
        }
        MotionTemplateSpec::ProgressBar { .. } => {
            Err("expand_template: ProgressBar is not implemented yet (Task 2+)".to_string())
        }
    }
}

/// Slide-in/out duration for lower-third layers, in seconds. Mirrors
/// the scene-default fade window (see
/// [`montage_proto::professional::MOTION_SCENE_DEFAULT_FADE_S`]) so
/// the motion reads at a familiar pace.
const LOWER_THIRD_SLIDE_S: f64 = 0.55;

/// Per-layer entrance stagger for the lower-third card: bar, then
/// name (+0.08s), then role (+0.16s).
const LOWER_THIRD_STAGGER_S: f64 = 0.08;

/// Bar geometry, mirroring `plan_motion_scene`'s panel/callout
/// conventions: `x`/`y` are the Solid/Shape top-left corner as a
/// fraction of the canvas (see `background_panel_layer`).
const BAR_X: f64 = 0.04;
const BAR_Y: f64 = 0.82;
const BAR_WIDTH: f64 = 0.40;
const BAR_HEIGHT: f64 = 0.10;

/// Text layer geometry: `x`/`y` are the box center (see
/// `plan_motion_scene::headline_layer`).
const NAME_X: f64 = 0.06 + (0.36 / 2.0);
const NAME_Y: f64 = 0.85;
const NAME_WIDTH: f64 = 0.36;
const NAME_HEIGHT: f64 = 0.06;

const ROLE_Y: f64 = NAME_Y + 0.055;
const ROLE_HEIGHT: f64 = 0.045;

/// Lower-third: a solid accent bar that slides in from off-canvas
/// left, a name text layer staggered `LOWER_THIRD_STAGGER_S` behind
/// it, and (when `role` is present) a role text layer staggered
/// `2 * LOWER_THIRD_STAGGER_S` behind the bar. All three slide back
/// out in the final [`LOWER_THIRD_SLIDE_S`] of their own layer
/// window.
fn expand_lower_third(
    name: &str,
    role: Option<&str>,
    scene_duration_s: f64,
) -> Result<TemplateExpansion, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("expand_template: LowerThird name must not be empty".to_string());
    }
    let role = role.map(str::trim).filter(|role| !role.is_empty());

    let bar_from_s = 0.0;
    let bar_duration_s = scene_duration_s;
    let name_from_s = LOWER_THIRD_STAGGER_S.min(scene_duration_s);
    let name_duration_s = (scene_duration_s - name_from_s).max(0.01);
    let role_from_s = (2.0 * LOWER_THIRD_STAGGER_S).min(scene_duration_s);
    let role_duration_s = (scene_duration_s - role_from_s).max(0.01);

    let bar = MotionSceneLayer {
        id: "lower-third-bar".into(),
        kind: MotionSceneLayerKind::Solid,
        from_s: bar_from_s,
        duration_s: bar_duration_s,
        z_index: 10,
        params: BTreeMap::from([
            ("x".into(), serde_json::json!(BAR_X)),
            ("y".into(), serde_json::json!(BAR_Y)),
            ("width".into(), serde_json::json!(BAR_WIDTH)),
            ("height".into(), serde_json::json!(BAR_HEIGHT)),
            ("color".into(), serde_json::json!("#111111")),
            ("opacity".into(), serde_json::json!(0.82)),
            (
                "animations".into(),
                serde_json::json!([slide_x_animation(BAR_X, BAR_WIDTH, bar_duration_s)]),
            ),
        ]),
    };

    let name_layer = MotionSceneLayer {
        id: "lower-third-name".into(),
        kind: MotionSceneLayerKind::Text,
        from_s: name_from_s,
        duration_s: name_duration_s,
        z_index: 11,
        params: BTreeMap::from([
            ("text".into(), serde_json::json!(name)),
            ("font_size".into(), serde_json::json!(36)),
            ("font_weight".into(), serde_json::json!("bold")),
            ("align".into(), serde_json::json!("left")),
            ("x".into(), serde_json::json!(NAME_X)),
            ("y".into(), serde_json::json!(NAME_Y)),
            ("width".into(), serde_json::json!(NAME_WIDTH)),
            ("height".into(), serde_json::json!(NAME_HEIGHT)),
            (
                "animations".into(),
                serde_json::json!([
                    slide_x_animation(NAME_X, NAME_WIDTH, name_duration_s),
                    fade_opacity_animation(name_duration_s),
                ]),
            ),
        ]),
    };

    let mut layers = vec![bar, name_layer];

    if let Some(role) = role {
        let role_layer = MotionSceneLayer {
            id: "lower-third-role".into(),
            kind: MotionSceneLayerKind::Text,
            from_s: role_from_s,
            duration_s: role_duration_s,
            z_index: 11,
            params: BTreeMap::from([
                ("text".into(), serde_json::json!(role)),
                ("font_size".into(), serde_json::json!(24)),
                ("font_weight".into(), serde_json::json!("normal")),
                ("align".into(), serde_json::json!("left")),
                ("x".into(), serde_json::json!(NAME_X)),
                ("y".into(), serde_json::json!(ROLE_Y)),
                ("width".into(), serde_json::json!(NAME_WIDTH)),
                ("height".into(), serde_json::json!(ROLE_HEIGHT)),
                (
                    "animations".into(),
                    serde_json::json!([
                        slide_x_animation(NAME_X, NAME_WIDTH, role_duration_s),
                        fade_opacity_animation(role_duration_s),
                    ]),
                ),
            ]),
        };
        layers.push(role_layer);
    }

    let rationale = match role {
        Some(role) => format!("Lower third for {name}, role '{role}'."),
        None => format!("Lower third for {name}."),
    };

    Ok(TemplateExpansion { layers, rationale })
}

/// `overlay.x` slide-in/slide-out keyframes for a layer whose resting
/// left edge is `final_x` and whose width is `width`. The off-canvas
/// start position clears the frame entirely (`final_x - width`) so
/// the layer is fully hidden before it enters. When the layer window
/// is too short to fit both a slide-in and slide-out without
/// overlapping, the two slides are compressed to share the midpoint
/// instead of reversing order.
fn slide_x_animation(
    final_x: f64,
    width: f64,
    duration_s: f64,
) -> montage_proto::professional::MotionSceneLayerAnimation {
    use montage_proto::professional::{Easing, Keyframe};

    let off_canvas_x = final_x - width;
    let slide_s = LOWER_THIRD_SLIDE_S.min(duration_s / 2.0);

    // `KeyframeInterpolation::Linear` + a named `Easing` curve (not
    // `Bezier` interpolation, which expects explicit control-point
    // handles) — matches `plan_emphasis::keyframe`'s convention for
    // eased motion without authored Bezier handles.
    let ease_out_cubic = |time_s: f64, value: f64| Keyframe {
        easing: Easing::EaseOutCubic,
        ..Keyframe::linear(time_s, value)
    };

    montage_proto::professional::MotionSceneLayerAnimation {
        parameter: "overlay.x".into(),
        keyframes: vec![
            ease_out_cubic(0.0, off_canvas_x),
            Keyframe::linear(slide_s, final_x),
            Keyframe::linear(duration_s - slide_s, final_x),
            ease_out_cubic(duration_s, off_canvas_x),
        ],
        pre_extrapolation: Default::default(),
        post_extrapolation: Default::default(),
        motion_path: None,
    }
}

/// `overlay.opacity` fade-in/fade-out keyframes over the same
/// [`LOWER_THIRD_SLIDE_S`] window as the slide, so the card fades and
/// slides in together rather than popping visible mid-slide.
fn fade_opacity_animation(
    duration_s: f64,
) -> montage_proto::professional::MotionSceneLayerAnimation {
    use montage_proto::professional::Keyframe;

    let slide_s = LOWER_THIRD_SLIDE_S.min(duration_s / 2.0);

    montage_proto::professional::MotionSceneLayerAnimation {
        parameter: "overlay.opacity".into(),
        keyframes: vec![
            Keyframe::linear(0.0, 0.0),
            Keyframe::linear(slide_s, 1.0),
            Keyframe::linear(duration_s - slide_s, 1.0),
            Keyframe::linear(duration_s, 0.0),
        ],
        pre_extrapolation: Default::default(),
        post_extrapolation: Default::default(),
        motion_path: None,
    }
}

#[cfg(test)]
mod tests {
    use montage_proto::professional::MotionScene;

    use super::*;

    fn wrap_in_scene(layers: Vec<MotionSceneLayer>, duration_s: f64, fps: f64) -> MotionScene {
        MotionScene {
            id: "test-scene".into(),
            start_s: 0.0,
            duration_s,
            fps,
            width: 1920,
            height: 1080,
            layers,
            rationale: None,
        }
    }

    #[test]
    fn lower_third_expands_to_valid_layers() {
        let spec = MotionTemplateSpec::LowerThird {
            name: "Ada Lovelace".to_string(),
            role: Some("Mathematician".to_string()),
        };

        let expansion = expand_template(&spec, 4.0, 30.0).expect("lower third should expand");

        assert!(
            expansion.layers.len() >= 2,
            "expected at least 2 layers, got {}",
            expansion.layers.len()
        );

        let ids: Vec<&str> = expansion.layers.iter().map(|l| l.id.as_str()).collect();
        assert!(ids.contains(&"lower-third-bar"));
        assert!(ids.contains(&"lower-third-name"));

        let bar = expansion
            .layers
            .iter()
            .find(|l| l.id == "lower-third-bar")
            .unwrap();
        assert_eq!(bar.kind, MotionSceneLayerKind::Solid);

        let name_layer = expansion
            .layers
            .iter()
            .find(|l| l.id == "lower-third-name")
            .unwrap();
        assert_eq!(name_layer.kind, MotionSceneLayerKind::Text);

        let scene = wrap_in_scene(expansion.layers, 4.0, 30.0);
        let diagnostics = scene.validate();
        assert!(
            diagnostics.is_empty(),
            "expected a valid scene, got diagnostics: {diagnostics:?}"
        );
    }

    #[test]
    fn lower_third_without_role_omits_role_layer() {
        let spec = MotionTemplateSpec::LowerThird {
            name: "Ada Lovelace".to_string(),
            role: None,
        };

        let expansion = expand_template(&spec, 4.0, 30.0).expect("lower third should expand");

        assert_eq!(
            expansion.layers.len(),
            2,
            "expected exactly bar + name layers without a role, got {:?}",
            expansion
                .layers
                .iter()
                .map(|l| l.id.as_str())
                .collect::<Vec<_>>()
        );
        assert!(
            expansion.layers.iter().all(|l| l.id != "lower-third-role"),
            "role layer should not be emitted when role is None"
        );

        let scene = wrap_in_scene(expansion.layers, 4.0, 30.0);
        let diagnostics = scene.validate();
        assert!(
            diagnostics.is_empty(),
            "expected a valid scene, got diagnostics: {diagnostics:?}"
        );
    }

    #[test]
    fn lower_third_rejects_empty_name() {
        let spec = MotionTemplateSpec::LowerThird {
            name: "   ".to_string(),
            role: None,
        };
        let result = expand_template(&spec, 4.0, 30.0);
        assert!(result.is_err());
    }

    #[test]
    fn expand_template_rejects_non_positive_duration() {
        let spec = MotionTemplateSpec::LowerThird {
            name: "Ada".to_string(),
            role: None,
        };
        let result = expand_template(&spec, 0.0, 30.0);
        assert!(result.is_err());
    }

    #[test]
    fn kinetic_text_is_not_yet_implemented() {
        let spec = MotionTemplateSpec::KineticText {
            words: vec![KineticWord {
                text: "hello".to_string(),
                at_s: 0.0,
                hold_s: 0.5,
            }],
            anchor: TextAnchor::Center,
        };
        let result = expand_template(&spec, 4.0, 30.0);
        assert!(result.is_err());
    }
}
