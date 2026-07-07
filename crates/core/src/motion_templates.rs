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
    /// How long the word holds on screen, in seconds, after its pop-in.
    /// `0.0` means "run to the end of the scene" (the word never gets
    /// its own exit before the scene does); a positive value caps the
    /// layer's `duration_s` at `at_s + hold_s`, clamped to the scene
    /// duration, so the word can exit before the scene ends. Must be
    /// non-negative.
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
        MotionTemplateSpec::KineticText { words, anchor } => {
            expand_kinetic_text(words, *anchor, scene_duration_s)
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

/// Pop-in duration for each kinetic-text word: `overlay.opacity` fades
/// 0→1 and `overlay.scale` springs 1.15→1.0 over this window.
const KINETIC_POP_IN_S: f64 = 0.12;

/// Overshoot scale a word pops in from before settling to 1.0.
const KINETIC_POP_IN_SCALE: f64 = 1.15;

/// Spring parameters for the pop-in scale settle (per task brief).
const KINETIC_SPRING: montage_proto::professional::SpringParameters =
    montage_proto::professional::SpringParameters {
        mass: 1.0,
        stiffness: 180.0,
        damping: 12.0,
    };

/// Font size for kinetic-text words, in reference px (mirrors
/// `plan_motion_scene`'s headline sizing).
const KINETIC_FONT_SIZE: u32 = 48;

/// Reference canvas width kinetic-text layout is computed against
/// (matches the 1080p reference `fit_text_to_width_px` and
/// `plan_motion_scene` already assume).
const KINETIC_CANVAS_WIDTH_PX: f64 = 1920.0;

/// Horizontal gap between adjacent kinetic-text words, as a fraction
/// of canvas width.
const KINETIC_WORD_GAP: f64 = 0.012;

/// Text layer height, as a fraction of canvas height (single line).
const KINETIC_WORD_HEIGHT: f64 = 0.08;

/// Word-by-word kinetic-text cascade: one Text layer per word,
/// `kinetic-word-<i>`, appearing at its own `at_s`. A word with
/// `hold_s == 0.0` holds through the rest of the scene (the
/// scene-default exit fade handles the out); a word with a positive
/// `hold_s` exits after `at_s + hold_s` (clamped to the scene
/// duration) instead. Words lay out on a single line with cumulative
/// x offsets estimated via [`montage_render::fit_text_to_width_px`]
/// character metrics, anchored per `anchor`.
fn expand_kinetic_text(
    words: &[KineticWord],
    anchor: TextAnchor,
    scene_duration_s: f64,
) -> Result<TemplateExpansion, String> {
    if words.is_empty() {
        return Err("expand_template: KineticText words must not be empty".to_string());
    }
    for (i, word) in words.iter().enumerate() {
        if word.text.trim().is_empty() {
            return Err(format!(
                "expand_template: KineticText word {i} text must not be empty"
            ));
        }
        if !word.at_s.is_finite() || word.at_s < 0.0 {
            return Err(format!(
                "expand_template: KineticText word {i} at_s must be non-negative, got {}",
                word.at_s
            ));
        }
        if word.at_s >= scene_duration_s {
            return Err(format!(
                "expand_template: KineticText word {i} at_s ({}) must be before scene_duration_s ({scene_duration_s})",
                word.at_s
            ));
        }
        if !word.hold_s.is_finite() || word.hold_s < 0.0 {
            return Err(format!(
                "expand_template: KineticText word {i} hold_s must be non-negative, got {}",
                word.hold_s
            ));
        }
    }

    // Per-word pixel width (unwrapped: a huge max width guarantees
    // `fit_text_to_width_px` never wraps or shrinks a single word) and
    // its normalized-to-canvas-width equivalent, laid out left to
    // right with a fixed gap between words.
    let word_widths_px: Vec<f64> = words
        .iter()
        .map(|word| {
            let (wrapped, font_size) =
                montage_render::fit_text_to_width_px(word.text.trim(), KINETIC_FONT_SIZE, f64::MAX);
            estimate_text_width_px(&wrapped, font_size)
        })
        .collect();
    let word_widths_frac: Vec<f64> = word_widths_px
        .iter()
        .map(|width_px| width_px / KINETIC_CANVAS_WIDTH_PX)
        .collect();

    let total_width_frac: f64 = word_widths_frac.iter().sum::<f64>()
        + KINETIC_WORD_GAP * (word_widths_frac.len().saturating_sub(1) as f64);

    let (start_x, y) = match anchor {
        TextAnchor::LowerLeft => (0.06, 0.82),
        TextAnchor::Center => (((1.0 - total_width_frac) / 2.0).max(0.02), 0.46),
        TextAnchor::LowerCenter => (((1.0 - total_width_frac) / 2.0).max(0.02), 0.82),
    };

    let mut layers = Vec::with_capacity(words.len());
    let mut cursor_x = start_x;
    for (i, word) in words.iter().enumerate() {
        let width_frac = word_widths_frac[i];
        let from_s = word.at_s;
        let run_to_scene_end_s = (scene_duration_s - from_s).max(0.01);
        let duration_s = if word.hold_s > 0.0 {
            word.hold_s.min(run_to_scene_end_s)
        } else {
            run_to_scene_end_s
        };

        let layer = MotionSceneLayer {
            id: format!("kinetic-word-{i}"),
            kind: MotionSceneLayerKind::Text,
            from_s,
            duration_s,
            z_index: 12,
            params: BTreeMap::from([
                ("text".into(), serde_json::json!(word.text.trim())),
                ("font_size".into(), serde_json::json!(KINETIC_FONT_SIZE)),
                ("font_weight".into(), serde_json::json!("bold")),
                ("align".into(), serde_json::json!("left")),
                ("x".into(), serde_json::json!(cursor_x)),
                ("y".into(), serde_json::json!(y)),
                ("width".into(), serde_json::json!(width_frac)),
                ("height".into(), serde_json::json!(KINETIC_WORD_HEIGHT)),
                (
                    "animations".into(),
                    serde_json::json!([pop_in_opacity_animation(), pop_in_scale_animation(),]),
                ),
            ]),
        };
        layers.push(layer);

        cursor_x += width_frac + KINETIC_WORD_GAP;
    }

    let rationale = format!(
        "Kinetic text: {} word{} cascading from {:.2}s.",
        words.len(),
        if words.len() == 1 { "" } else { "s" },
        words[0].at_s
    );

    Ok(TemplateExpansion { layers, rationale })
}

/// Rough per-character pixel-width estimate consistent with the
/// render's average-glyph-width heuristic
/// (`title_wrap_max_chars`'s `font_size * 0.55`), scaled by the
/// wrapped word's grapheme count. `fit_text_to_width_px` only exposes
/// wrapped text + a (possibly shrunk) font size, not a pixel width, so
/// this mirrors that internal constant rather than re-deriving it.
fn estimate_text_width_px(wrapped_text: &str, font_size: u32) -> f64 {
    const AVERAGE_GLYPH_WIDTH_FACTOR: f64 = 0.55;
    let longest_line_chars = wrapped_text
        .split('\n')
        .map(|line| line.chars().count())
        .max()
        .unwrap_or(0);
    longest_line_chars as f64 * f64::from(font_size) * AVERAGE_GLYPH_WIDTH_FACTOR
}

/// `overlay.opacity` pop-in keyframes: 0→1 over
/// [`KINETIC_POP_IN_S`]. `motion_animations_with_scene_fade` appends
/// the scene-default exit fade since this ends at a visible value
/// before the layer's duration — the desired behavior for a word that
/// should fade with the scene's exit.
fn pop_in_opacity_animation() -> montage_proto::professional::MotionSceneLayerAnimation {
    use montage_proto::professional::Keyframe;

    montage_proto::professional::MotionSceneLayerAnimation {
        parameter: "overlay.opacity".into(),
        keyframes: vec![
            Keyframe::linear(0.0, 0.0),
            Keyframe::linear(KINETIC_POP_IN_S, 1.0),
        ],
        pre_extrapolation: Default::default(),
        post_extrapolation: Default::default(),
        motion_path: None,
    }
}

/// `overlay.scale` pop-in keyframes: starts overshot at
/// [`KINETIC_POP_IN_SCALE`] and springs to its 1.0 settle target over
/// [`KINETIC_POP_IN_S`] using [`KINETIC_SPRING`] parameters. Both
/// keyframe evaluators (desktop `evaluateAnimationValue` and
/// `montage_render::animation::evaluate_keyframes_with_extrapolation`)
/// read `interpolation`/`spring` off the segment-*start* keyframe to
/// drive the segment ending at the next keyframe, so the spring params
/// live on the overshoot start keyframe (`time_s` 0.0); the end
/// keyframe (`time_s` [`KINETIC_POP_IN_S`]) is the plain settle target.
fn pop_in_scale_animation() -> montage_proto::professional::MotionSceneLayerAnimation {
    use montage_proto::professional::{Keyframe, KeyframeInterpolation};

    let overshoot_start = Keyframe {
        spring: Some(KINETIC_SPRING),
        interpolation: KeyframeInterpolation::Spring,
        ..Keyframe::linear(0.0, KINETIC_POP_IN_SCALE)
    };

    montage_proto::professional::MotionSceneLayerAnimation {
        parameter: "overlay.scale".into(),
        keyframes: vec![overshoot_start, Keyframe::linear(KINETIC_POP_IN_S, 1.0)],
        pre_extrapolation: Default::default(),
        post_extrapolation: Default::default(),
        motion_path: None,
    }
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
    fn kinetic_text_expands_one_layer_per_word() {
        let spec = MotionTemplateSpec::KineticText {
            words: vec![
                KineticWord {
                    text: "hello".to_string(),
                    at_s: 0.0,
                    hold_s: 0.5,
                },
                KineticWord {
                    text: "world".to_string(),
                    at_s: 0.3,
                    hold_s: 0.5,
                },
            ],
            anchor: TextAnchor::Center,
        };

        let expansion = expand_template(&spec, 4.0, 30.0).expect("kinetic text should expand");

        assert_eq!(expansion.layers.len(), 2);
        assert_eq!(expansion.layers[0].id, "kinetic-word-0");
        assert_eq!(expansion.layers[1].id, "kinetic-word-1");
        assert!(
            expansion
                .layers
                .iter()
                .all(|layer| layer.kind == MotionSceneLayerKind::Text)
        );

        let scene = wrap_in_scene(expansion.layers, 4.0, 30.0);
        let diagnostics = scene.validate();
        assert!(
            diagnostics.is_empty(),
            "expected a valid scene, got diagnostics: {diagnostics:?}"
        );
    }

    #[test]
    fn kinetic_text_from_s_matches_at_s_and_runs_to_scene_end() {
        let spec = MotionTemplateSpec::KineticText {
            words: vec![
                KineticWord {
                    text: "hello".to_string(),
                    at_s: 0.0,
                    hold_s: 0.0,
                },
                KineticWord {
                    text: "world".to_string(),
                    at_s: 0.3,
                    hold_s: 0.0,
                },
            ],
            anchor: TextAnchor::Center,
        };

        let expansion = expand_template(&spec, 4.0, 30.0).expect("kinetic text should expand");

        assert_eq!(expansion.layers[0].from_s, 0.0);
        assert_eq!(expansion.layers[1].from_s, 0.3);
        assert_eq!(expansion.layers[0].duration_s, 4.0);
        assert_eq!(expansion.layers[1].duration_s, 4.0 - 0.3);
    }

    #[test]
    fn kinetic_text_positive_hold_s_caps_duration_before_scene_end() {
        let spec = MotionTemplateSpec::KineticText {
            words: vec![
                KineticWord {
                    text: "hello".to_string(),
                    at_s: 0.0,
                    hold_s: 0.5,
                },
                KineticWord {
                    text: "world".to_string(),
                    at_s: 0.3,
                    // hold_s would run past the scene end from at_s, so
                    // it must clamp to the remaining scene duration.
                    hold_s: 10.0,
                },
            ],
            anchor: TextAnchor::Center,
        };

        let expansion = expand_template(&spec, 4.0, 30.0).expect("kinetic text should expand");

        assert_eq!(
            expansion.layers[0].duration_s, 0.5,
            "positive hold_s should cap duration_s instead of running to scene end"
        );
        assert_eq!(
            expansion.layers[1].duration_s,
            4.0 - 0.3,
            "hold_s exceeding the remaining scene duration should clamp to the scene end"
        );

        let scene = wrap_in_scene(expansion.layers, 4.0, 30.0);
        let diagnostics = scene.validate();
        assert!(
            diagnostics.is_empty(),
            "expected a valid scene, got diagnostics: {diagnostics:?}"
        );
    }

    #[test]
    fn kinetic_text_rejects_negative_hold_s() {
        let spec = MotionTemplateSpec::KineticText {
            words: vec![KineticWord {
                text: "hello".to_string(),
                at_s: 0.0,
                hold_s: -0.1,
            }],
            anchor: TextAnchor::Center,
        };
        let result = expand_template(&spec, 4.0, 30.0);
        assert!(result.is_err());
    }

    #[test]
    fn kinetic_text_per_word_from_s_strictly_increasing() {
        let spec = MotionTemplateSpec::KineticText {
            words: vec![
                KineticWord {
                    text: "one".to_string(),
                    at_s: 0.0,
                    hold_s: 0.5,
                },
                KineticWord {
                    text: "two".to_string(),
                    at_s: 0.2,
                    hold_s: 0.5,
                },
                KineticWord {
                    text: "three".to_string(),
                    at_s: 0.4,
                    hold_s: 0.5,
                },
            ],
            anchor: TextAnchor::LowerLeft,
        };

        let expansion = expand_template(&spec, 4.0, 30.0).expect("kinetic text should expand");

        let from_times: Vec<f64> = expansion.layers.iter().map(|l| l.from_s).collect();
        for window in from_times.windows(2) {
            assert!(
                window[1] > window[0],
                "expected strictly increasing from_s, got {from_times:?}"
            );
        }
    }

    #[test]
    fn kinetic_text_word_boxes_do_not_overlap() {
        let spec = MotionTemplateSpec::KineticText {
            words: vec![
                KineticWord {
                    text: "one".to_string(),
                    at_s: 0.0,
                    hold_s: 0.5,
                },
                KineticWord {
                    text: "two".to_string(),
                    at_s: 0.2,
                    hold_s: 0.5,
                },
                KineticWord {
                    text: "three".to_string(),
                    at_s: 0.4,
                    hold_s: 0.5,
                },
            ],
            anchor: TextAnchor::LowerLeft,
        };

        let expansion = expand_template(&spec, 4.0, 30.0).expect("kinetic text should expand");

        let boxes: Vec<(f64, f64)> = expansion
            .layers
            .iter()
            .map(|layer| {
                let x = layer
                    .params
                    .get("x")
                    .and_then(serde_json::Value::as_f64)
                    .unwrap();
                let width = layer
                    .params
                    .get("width")
                    .and_then(serde_json::Value::as_f64)
                    .unwrap();
                (x, width)
            })
            .collect();

        for window in boxes.windows(2) {
            let (x0, width0) = window[0];
            let (x1, _width1) = window[1];
            assert!(
                x0 + width0 <= x1,
                "expected word boxes to not overlap: word 0 ends at {}, word 1 starts at {}",
                x0 + width0,
                x1
            );
        }
    }

    #[test]
    fn kinetic_text_rejects_empty_words() {
        let spec = MotionTemplateSpec::KineticText {
            words: vec![],
            anchor: TextAnchor::Center,
        };
        let result = expand_template(&spec, 4.0, 30.0);
        assert!(result.is_err());
    }

    #[test]
    fn kinetic_text_rejects_at_s_beyond_scene_duration() {
        let spec = MotionTemplateSpec::KineticText {
            words: vec![KineticWord {
                text: "hello".to_string(),
                at_s: 5.0,
                hold_s: 0.5,
            }],
            anchor: TextAnchor::Center,
        };
        let result = expand_template(&spec, 4.0, 30.0);
        assert!(result.is_err());
    }

    #[test]
    fn kinetic_text_pop_in_uses_spring_scale_and_opacity_fade() {
        let spec = MotionTemplateSpec::KineticText {
            words: vec![KineticWord {
                text: "hello".to_string(),
                at_s: 0.0,
                hold_s: 0.5,
            }],
            anchor: TextAnchor::Center,
        };

        let expansion = expand_template(&spec, 4.0, 30.0).expect("kinetic text should expand");
        let word = &expansion.layers[0];
        let animations = word.motion_animations();

        let opacity = animations
            .iter()
            .find(|a| a.parameter == "overlay.opacity")
            .expect("word has an overlay.opacity animation");
        assert_eq!(opacity.keyframes.first().unwrap().value, 0.0);
        assert_eq!(opacity.keyframes.first().unwrap().time_s, 0.0);
        let pop_in_end = opacity
            .keyframes
            .iter()
            .find(|kf| kf.time_s > 0.0)
            .expect("opacity has a pop-in end keyframe");
        assert_eq!(pop_in_end.time_s, 0.12);
        assert_eq!(pop_in_end.value, 1.0);

        let scale = animations
            .iter()
            .find(|a| a.parameter == "overlay.scale")
            .expect("word has an overlay.scale animation");

        // Both keyframe evaluators (desktop and render) read
        // interpolation/spring off the segment-start keyframe, so the
        // spring must be on keyframes[0] (time_s 0.0) for it to fire at
        // all; putting it on the end keyframe silently renders linear.
        let overshoot_start = scale
            .keyframes
            .first()
            .expect("scale has an overshoot start keyframe");
        assert_eq!(overshoot_start.time_s, 0.0);
        assert_eq!(overshoot_start.value, 1.15);
        assert_eq!(
            overshoot_start.interpolation,
            montage_proto::professional::KeyframeInterpolation::Spring
        );
        let spring = overshoot_start
            .spring
            .expect("overshoot start keyframe carries spring params");
        assert_eq!(spring.mass, 1.0);
        assert_eq!(spring.stiffness, 180.0);
        assert_eq!(spring.damping, 12.0);

        let settle = scale.keyframes.get(1).expect("scale has a settle keyframe");
        assert_eq!(settle.time_s, 0.12);
        assert_eq!(settle.value, 1.0);
        assert_eq!(
            settle.interpolation,
            montage_proto::professional::KeyframeInterpolation::Linear,
            "settle keyframe should be the plain target, not carry its own interpolation"
        );
    }

    #[test]
    fn kinetic_text_on_screen_copy_matches_words_text() {
        let spec = MotionTemplateSpec::KineticText {
            words: vec![
                KineticWord {
                    text: "hello".to_string(),
                    at_s: 0.0,
                    hold_s: 0.5,
                },
                KineticWord {
                    text: "world".to_string(),
                    at_s: 0.3,
                    hold_s: 0.5,
                },
            ],
            anchor: TextAnchor::Center,
        };

        let expansion = expand_template(&spec, 4.0, 30.0).expect("kinetic text should expand");

        let texts: Vec<&str> = expansion
            .layers
            .iter()
            .map(|layer| {
                layer
                    .params
                    .get("text")
                    .and_then(serde_json::Value::as_str)
                    .unwrap()
            })
            .collect();
        assert_eq!(texts, vec!["hello", "world"]);
    }
}
