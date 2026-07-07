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
        MotionTemplateSpec::KineticText { .. } => Err(
            "expand_template: KineticText is not implemented yet (Task 2+)".to_string(),
        ),
        MotionTemplateSpec::HighlightBox { .. } => Err(
            "expand_template: HighlightBox is not implemented yet (Task 2+)".to_string(),
        ),
        MotionTemplateSpec::ProgressBar { .. } => Err(
            "expand_template: ProgressBar is not implemented yet (Task 2+)".to_string(),
        ),
    }
}

/// Lower-third stub: a solid accent bar plus a name text layer, both
/// spanning the full scene duration. The role line (when present) is
/// left for a follow-up task; for now it is folded into the rationale
/// so callers know it was accepted but not yet rendered.
fn expand_lower_third(
    name: &str,
    role: Option<&str>,
    scene_duration_s: f64,
) -> Result<TemplateExpansion, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("expand_template: LowerThird name must not be empty".to_string());
    }

    let bar = MotionSceneLayer {
        id: "lower-third-bar".into(),
        kind: MotionSceneLayerKind::Solid,
        from_s: 0.0,
        duration_s: scene_duration_s,
        z_index: 10,
        params: BTreeMap::from([
            ("x".into(), serde_json::json!(0.04)),
            ("y".into(), serde_json::json!(0.82)),
            ("width".into(), serde_json::json!(0.40)),
            ("height".into(), serde_json::json!(0.10)),
            ("color".into(), serde_json::json!("#111111")),
            ("opacity".into(), serde_json::json!(0.82)),
        ]),
    };

    let name_layer = MotionSceneLayer {
        id: "lower-third-name".into(),
        kind: MotionSceneLayerKind::Text,
        from_s: 0.0,
        duration_s: scene_duration_s,
        z_index: 11,
        params: BTreeMap::from([
            ("text".into(), serde_json::json!(name)),
            ("font_size".into(), serde_json::json!(36)),
            ("font_weight".into(), serde_json::json!("bold")),
            ("align".into(), serde_json::json!("left")),
            ("x".into(), serde_json::json!(0.06)),
            ("y".into(), serde_json::json!(0.85)),
            ("width".into(), serde_json::json!(0.36)),
            ("height".into(), serde_json::json!(0.06)),
        ]),
    };

    let rationale = match role {
        Some(role) if !role.trim().is_empty() => format!(
            "Lower third for {name}; role '{}' captured but not yet rendered (role line lands in a follow-up task).",
            role.trim()
        ),
        _ => format!("Lower third for {name}."),
    };

    Ok(TemplateExpansion {
        layers: vec![bar, name_layer],
        rationale,
    })
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

        let expansion =
            expand_template(&spec, 4.0, 30.0).expect("lower third should expand");

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
