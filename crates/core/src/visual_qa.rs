//! Visual QA helpers for agent-authored timeline support.
//!
//! These checks are intentionally structural. They catch the common mistakes
//! that made MotionScenes look broken before an agent asks for rendered-frame
//! inspection: full-frame backing panels, off-frame layer boxes, bad timing,
//! and numbered text layers that do not sit inside their matching bars.

use std::collections::HashMap;

use montage_proto::otio::Timeline;
use montage_proto::professional::{MotionScene, MotionSceneLayer, MotionSceneLayerKind};
use serde::Serialize;

/// Severity for a visual QA issue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MotionSceneQaSeverity {
    Error,
    Warning,
}

/// Stable kind for a visual QA issue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MotionSceneQaIssueKind {
    InvalidScene,
    SceneOutsideTimeline,
    LayerOutsideScene,
    LayerOutOfFrame,
    FullFrameBacking,
    TextOutsideBacking,
}

/// One MotionScene QA issue.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MotionSceneQaIssue {
    pub severity: MotionSceneQaSeverity,
    pub kind: MotionSceneQaIssueKind,
    pub scene_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub layer_id: Option<String>,
    pub message: String,
}

/// Render sample plan for a scene.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MotionSceneFrameSample {
    pub scene_id: String,
    pub label: String,
    pub t_s: f64,
}

/// Structural QA report for MotionScenes.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct MotionSceneQaReport {
    pub scene_count: usize,
    pub issues: Vec<MotionSceneQaIssue>,
    pub frame_samples: Vec<MotionSceneFrameSample>,
}

/// Audit stored MotionScenes and return structural issues plus frame sample
/// times. `scene_id` narrows the report to one scene when present.
pub fn audit_motion_scenes(
    timeline: &Timeline,
    timeline_duration_s: Option<f64>,
    scene_id: Option<&str>,
) -> MotionSceneQaReport {
    // An imported/unversioned project may carry no montage metadata at all.
    // Don't early-return: the explicit scene-id miss check below must still
    // run so a typo'd or unpersisted `--scene-id` is reported, not silently
    // passed as an empty successful report.
    let scenes: Vec<&MotionScene> = timeline
        .metadata
        .montage
        .as_ref()
        .map(|metadata| {
            metadata
                .motion_scenes
                .iter()
                .filter(|scene| scene_id.is_none_or(|target| target == scene.id))
                .collect()
        })
        .unwrap_or_default();

    let mut report = MotionSceneQaReport {
        scene_count: scenes.len(),
        issues: Vec::new(),
        frame_samples: Vec::new(),
    };

    // An explicit scene-id that matches nothing must not silently pass QA — it
    // would let an agent mark a typo'd or unpersisted scene as audited.
    if let Some(target) = scene_id
        && scenes.is_empty()
    {
        report.issues.push(MotionSceneQaIssue {
            severity: MotionSceneQaSeverity::Error,
            kind: MotionSceneQaIssueKind::InvalidScene,
            scene_id: target.to_string(),
            layer_id: None,
            message: format!("scene-id '{target}' was not found among stored MotionScenes"),
        });
    }

    for scene in scenes {
        audit_scene(scene, timeline_duration_s, &mut report);
    }

    report
}

fn audit_scene(
    scene: &MotionScene,
    timeline_duration_s: Option<f64>,
    report: &mut MotionSceneQaReport,
) {
    for diagnostic in scene.validate() {
        report.issues.push(MotionSceneQaIssue {
            severity: MotionSceneQaSeverity::Error,
            kind: MotionSceneQaIssueKind::InvalidScene,
            scene_id: scene.id.clone(),
            layer_id: None,
            message: diagnostic.message,
        });
    }

    if let Some(timeline_duration_s) = timeline_duration_s {
        let end_s = scene.start_s + scene.duration_s;
        if end_s > timeline_duration_s + 1e-6 {
            report.issues.push(MotionSceneQaIssue {
                severity: MotionSceneQaSeverity::Error,
                kind: MotionSceneQaIssueKind::SceneOutsideTimeline,
                scene_id: scene.id.clone(),
                layer_id: None,
                message: format!(
                    "scene ends at {end_s:.3}s past timeline duration {timeline_duration_s:.3}s"
                ),
            });
        }
    }

    for sample in frame_samples_for_scene(scene) {
        report.frame_samples.push(sample);
    }

    let mut shape_by_suffix = HashMap::new();
    for layer in &scene.layers {
        audit_layer(scene, layer, report);
        if matches!(
            layer.kind,
            MotionSceneLayerKind::Shape | MotionSceneLayerKind::Solid
        ) && let Some(suffix) = numbered_suffix(&layer.id, "bar-")
        {
            shape_by_suffix.insert(suffix, layer);
        }
    }

    for layer in &scene.layers {
        if layer.kind != MotionSceneLayerKind::Text {
            continue;
        }
        let Some(suffix) = numbered_suffix(&layer.id, "text-") else {
            continue;
        };
        let Some(backing) = shape_by_suffix.get(&suffix) else {
            continue;
        };
        if let (Some(text_box), Some(backing_box)) = (
            normalized_box(scene, layer, BoxAnchor::Center),
            normalized_box(scene, backing, BoxAnchor::TopLeft),
        ) && !backing_box.contains(&text_box)
        {
            report.issues.push(MotionSceneQaIssue {
                severity: MotionSceneQaSeverity::Warning,
                kind: MotionSceneQaIssueKind::TextOutsideBacking,
                scene_id: scene.id.clone(),
                layer_id: Some(layer.id.clone()),
                message: format!("text layer {} is not contained by {}", layer.id, backing.id),
            });
        }
    }
}

fn audit_layer(scene: &MotionScene, layer: &MotionSceneLayer, report: &mut MotionSceneQaReport) {
    if layer.from_s + layer.duration_s > scene.duration_s + 1e-6 {
        report.issues.push(MotionSceneQaIssue {
            severity: MotionSceneQaSeverity::Error,
            kind: MotionSceneQaIssueKind::LayerOutsideScene,
            scene_id: scene.id.clone(),
            layer_id: Some(layer.id.clone()),
            message: format!("layer {} exceeds scene duration", layer.id),
        });
    }

    let anchor = match layer.kind {
        MotionSceneLayerKind::Text => BoxAnchor::Center,
        MotionSceneLayerKind::Shape | MotionSceneLayerKind::Solid => BoxAnchor::TopLeft,
        // Image layers scale to their box with `force_original_aspect_ratio`
        // (cover=`increase`, the default) and are overlaid WITHOUT cropping, so
        // the declared x/y/width/height is not the rendered extent — a small
        // in-frame box can still render well past the frame. Structural box QA
        // can't bound that without the asset's aspect ratio, so don't assert
        // containment here; image framing is verified via the rendered
        // frame_samples instead. See crates/render/src/timeline.rs image lowering.
        MotionSceneLayerKind::Image => return,
        _ => return,
    };
    let Some(layer_box) = normalized_box(scene, layer, anchor) else {
        return;
    };

    if !layer_box.is_in_frame() {
        report.issues.push(MotionSceneQaIssue {
            severity: MotionSceneQaSeverity::Warning,
            kind: MotionSceneQaIssueKind::LayerOutOfFrame,
            scene_id: scene.id.clone(),
            layer_id: Some(layer.id.clone()),
            message: format!("layer {} extends outside the normalized frame", layer.id),
        });
    }

    // `backdrop-full` is the planner's supported full-bleed card backing
    // (backdrop='full'); only flag *unexpected* full-frame backing layers.
    if matches!(
        layer.kind,
        MotionSceneLayerKind::Shape | MotionSceneLayerKind::Solid
    ) && layer.id != "backdrop-full"
        && layer_box.left <= 0.01
        && layer_box.top <= 0.01
        && layer_box.right >= 0.99
        && layer_box.bottom >= 0.99
    {
        report.issues.push(MotionSceneQaIssue {
            severity: MotionSceneQaSeverity::Warning,
            kind: MotionSceneQaIssueKind::FullFrameBacking,
            scene_id: scene.id.clone(),
            layer_id: Some(layer.id.clone()),
            message: format!("layer {} covers the full frame", layer.id),
        });
    }
}

fn frame_samples_for_scene(scene: &MotionScene) -> Vec<MotionSceneFrameSample> {
    if !scene.start_s.is_finite() || !scene.duration_s.is_finite() || scene.duration_s <= 0.0 {
        return Vec::new();
    }
    let inset = scene.duration_s.min(10.0) * 0.15;
    let raw_samples = [
        ("start", scene.start_s + inset.min(scene.duration_s * 0.25)),
        ("mid", scene.start_s + scene.duration_s * 0.5),
        (
            "end",
            scene.start_s + (scene.duration_s - inset.min(scene.duration_s * 0.25)),
        ),
    ];
    raw_samples
        .into_iter()
        .map(|(label, t_s)| MotionSceneFrameSample {
            scene_id: scene.id.clone(),
            label: label.into(),
            t_s,
        })
        .collect()
}

#[derive(Debug, Clone, Copy)]
enum BoxAnchor {
    TopLeft,
    Center,
}

#[derive(Debug, Clone, Copy)]
struct NormalizedBox {
    left: f64,
    top: f64,
    right: f64,
    bottom: f64,
}

impl NormalizedBox {
    fn is_in_frame(self) -> bool {
        self.left >= -1e-6
            && self.top >= -1e-6
            && self.right <= 1.0 + 1e-6
            && self.bottom <= 1.0 + 1e-6
    }

    fn contains(self, other: &NormalizedBox) -> bool {
        other.left >= self.left - 1e-6
            && other.top >= self.top - 1e-6
            && other.right <= self.right + 1e-6
            && other.bottom <= self.bottom + 1e-6
    }
}

/// Mirror the renderer: a layer value greater than 1 is scene-space pixels and
/// is divided by the scene extent to normalize, anything <= 1 is already a
/// fraction. See `scene_normalized_layer_param` in the render crate.
fn normalize_extent(value: f64, extent: u32) -> f64 {
    if value.abs() > 1.0 && extent > 0 {
        value / f64::from(extent)
    } else {
        value
    }
}

fn normalized_box(
    scene: &MotionScene,
    layer: &MotionSceneLayer,
    anchor: BoxAnchor,
) -> Option<NormalizedBox> {
    let is_text = layer.kind == MotionSceneLayerKind::Text;
    // Only text layers are normalized by the renderer (scene_normalized_layer_param).
    // Shape/image lowering copies raw x/y/width/height, so a pixel-space solid
    // really does render off-frame and must stay flagged — pass extent 0 there
    // so normalize_extent leaves the value untouched.
    let (ext_w, ext_h) = if is_text {
        (scene.width, scene.height)
    } else {
        (0, 0)
    };
    // Shape/solid lowering fills missing geometry with the renderer's defaults
    // (x=0, y=0, width=1, height=1), so mirror that for non-text layers — a
    // geometry-less solid renders full-frame and must remain auditable. Text
    // layers without an explicit box are skipped.
    let param = |key: &str, default: f64| -> Option<f64> {
        match number_param(layer, key) {
            Some(value) => Some(value),
            None if is_text => None,
            None => Some(default),
        }
    };
    let x = normalize_extent(param("x", 0.0)?, ext_w);
    let y = normalize_extent(param("y", 0.0)?, ext_h);
    let width = normalize_extent(param("width", 1.0)?, ext_w);
    let height = normalize_extent(param("height", 1.0)?, ext_h);
    if !x.is_finite()
        || !y.is_finite()
        || !width.is_finite()
        || !height.is_finite()
        || width < 0.0
        || height < 0.0
    {
        return None;
    }
    match anchor {
        BoxAnchor::TopLeft => Some(NormalizedBox {
            left: x,
            top: y,
            right: x + width,
            bottom: y + height,
        }),
        BoxAnchor::Center => Some(NormalizedBox {
            left: x - width * 0.5,
            top: y - height * 0.5,
            right: x + width * 0.5,
            bottom: y + height * 0.5,
        }),
    }
}

fn number_param(layer: &MotionSceneLayer, key: &str) -> Option<f64> {
    layer.params.get(key).and_then(serde_json::Value::as_f64)
}

fn numbered_suffix<'a>(id: &'a str, prefix: &str) -> Option<&'a str> {
    id.strip_prefix(prefix).filter(|suffix| {
        !suffix.is_empty() && suffix.chars().all(|character| character.is_ascii_digit())
    })
}
