//! Integration tests for the first-class `plan_reframe` agent tool.

use montage_core::montage_mcp::context::McpToolCtx;
use montage_core::montage_mcp::tools::plan_reframe::{PlanReframeArgs, SubjectCenter, run};

fn ctx_at(root: &std::path::Path) -> McpToolCtx {
    McpToolCtx {
        project_root: root.to_path_buf(),
    }
}

#[test]
fn plan_reframe_returns_apply_edl_ready_vertical_reframe_fragment() {
    let dir = match tempfile::tempdir() {
        Ok(dir) => dir,
        Err(err) => panic!("tempdir should create: {err}"),
    };
    let out = match run(
        PlanReframeArgs {
            clip_id: "clip-1".to_string(),
            aspect_ratio: Some("9:16".to_string()),
            source_width: Some(1920),
            source_height: Some(1080),
            subject_center: Some(SubjectCenter { x: 0.60, y: 0.42 }),
            context: None,
            safe_area: Some("mobile".to_string()),
            zoom: None,
        },
        ctx_at(dir.path()),
    ) {
        Ok(out) => out,
        Err(err) => panic!("plan_reframe should succeed: {err:?}"),
    };

    let body: serde_json::Value = match serde_json::from_str(&out) {
        Ok(body) => body,
        Err(err) => panic!("plan_reframe should return JSON: {err}; body={out}"),
    };
    assert_eq!(body["recommended"]["decision"], "set_reframe_effect");
    assert_eq!(body["recommended"]["clip_id"], "clip-1");
    assert_eq!(body["recommended"]["aspect_ratio"], "9:16");
    assert_eq!(body["recommended"]["zoom"], serde_json::json!(3.0));
    assert_eq!(body["recommended"]["x"], serde_json::json!(0.2));
    assert_eq!(body["recommended"]["y"], serde_json::json!(-0.16));
    assert_eq!(
        body["next_step"],
        "Pass edl_fragment to apply_edl, then render/review."
    );

    let edl = match body["edl_fragment"].as_str() {
        Some(edl) => edl,
        None => panic!("edl_fragment should be a string: {body}"),
    };
    assert!(edl.contains("*** Set Effect"));
    assert!(edl.contains("+ anchor: clip_uuid=clip-1"));
    assert!(edl.contains("+ effect: montage.reframe"));
    assert!(edl.contains(r#"+ params_json: {"zoom":3.0,"x":0.2,"y":-0.16}"#));
    assert!(edl.contains("+ rationale: Subject-aware 9:16 reframe"));
}

#[test]
fn plan_reframe_rejects_invalid_subject_center() {
    let dir = match tempfile::tempdir() {
        Ok(dir) => dir,
        Err(err) => panic!("tempdir should create: {err}"),
    };
    let err = match run(
        PlanReframeArgs {
            clip_id: "clip-1".to_string(),
            aspect_ratio: None,
            source_width: None,
            source_height: None,
            subject_center: Some(SubjectCenter { x: 1.4, y: 0.5 }),
            context: None,
            safe_area: None,
            zoom: None,
        },
        ctx_at(dir.path()),
    ) {
        Ok(out) => panic!("plan_reframe should reject invalid center, got {out:?}"),
        Err(err) => err,
    };

    assert!(err.contains("subject_center.x must be in 0..=1"));
}
