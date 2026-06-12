#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::collections::BTreeMap;

use montage_core::visual_qa::{MotionSceneQaIssueKind, audit_motion_scenes};
use montage_proto::otio::Timeline;
use montage_proto::professional::{MotionScene, MotionSceneLayer, MotionSceneLayerKind};

#[test]
fn motion_scene_qa_flags_full_frame_backing_layers() {
    let mut timeline = Timeline::empty("test");
    let montage = timeline.metadata.montage.as_mut().unwrap();
    montage.motion_scenes.push(MotionScene {
        id: "scene-a".into(),
        start_s: 1.0,
        duration_s: 5.0,
        fps: 30.0,
        width: 1920,
        height: 1080,
        layers: vec![shape_layer("veil", 0.0, 0.0, 1.0, 1.0)],
        rationale: None,
    });

    let report = audit_motion_scenes(&timeline, Some(10.0), None);

    assert!(report.issues.iter().any(|issue| {
        issue.kind == MotionSceneQaIssueKind::FullFrameBacking
            && issue.layer_id.as_deref() == Some("veil")
    }));
}

#[test]
fn motion_scene_qa_flags_text_outside_matching_bar() {
    let mut timeline = Timeline::empty("test");
    let montage = timeline.metadata.montage.as_mut().unwrap();
    montage.motion_scenes.push(MotionScene {
        id: "scene-a".into(),
        start_s: 1.0,
        duration_s: 5.0,
        fps: 30.0,
        width: 1920,
        height: 1080,
        layers: vec![
            shape_layer("bar-1", 0.1, 0.2, 0.2, 0.06),
            text_layer("text-1", "This wraps badly", 0.32, 0.23, 0.3, 0.08),
        ],
        rationale: None,
    });

    let report = audit_motion_scenes(&timeline, Some(10.0), None);

    assert!(report.issues.iter().any(|issue| {
        issue.kind == MotionSceneQaIssueKind::TextOutsideBacking
            && issue.layer_id.as_deref() == Some("text-1")
    }));
}

fn shape_layer(id: &str, x: f64, y: f64, width: f64, height: f64) -> MotionSceneLayer {
    let mut params = BTreeMap::new();
    params.insert("shape".into(), serde_json::json!("rectangle"));
    params.insert("x".into(), serde_json::json!(x));
    params.insert("y".into(), serde_json::json!(y));
    params.insert("width".into(), serde_json::json!(width));
    params.insert("height".into(), serde_json::json!(height));
    MotionSceneLayer {
        id: id.into(),
        kind: MotionSceneLayerKind::Shape,
        from_s: 0.0,
        duration_s: 4.0,
        z_index: 0,
        params,
    }
}

fn text_layer(id: &str, text: &str, x: f64, y: f64, width: f64, height: f64) -> MotionSceneLayer {
    let mut params = BTreeMap::new();
    params.insert("text".into(), serde_json::json!(text));
    params.insert("x".into(), serde_json::json!(x));
    params.insert("y".into(), serde_json::json!(y));
    params.insert("width".into(), serde_json::json!(width));
    params.insert("height".into(), serde_json::json!(height));
    MotionSceneLayer {
        id: id.into(),
        kind: MotionSceneLayerKind::Text,
        from_s: 0.0,
        duration_s: 4.0,
        z_index: 1,
        params,
    }
}
