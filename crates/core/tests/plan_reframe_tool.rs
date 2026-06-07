//! Integration tests for the first-class `plan_reframe` agent tool.

use montage_core::tools::plan_reframe::PlanReframeTool;
use montage_core::{FunctionCallError, ToolContext, ToolHandler, ToolInvocation};
use tokio::sync::broadcast;

fn ctx_at(root: &std::path::Path) -> ToolContext {
    let (tx, _) = broadcast::channel(8);
    ToolContext {
        project_root: root.to_path_buf(),
        events_tx: tx,
        user_input_tx: None,
        job_manager: montage_render::JobManager::new(),
        approval_tx: None,
        sandbox_mode: montage_core::tool::SandboxMode::Default,
        mcp_host: montage_core::mcp_host::McpHost::new(montage_mcp::ClientInfo {
            name: "test".into(),
            version: "0.0.0".into(),
        }),
        skills: std::sync::Arc::new(montage_core::skills::SkillRegistry::default()),
        subagent_return: None,
    }
}

fn invoke(args: serde_json::Value) -> ToolInvocation {
    ToolInvocation {
        call_id: "c1".into(),
        name: "plan_reframe".into(),
        args,
    }
}

#[test]
fn plan_reframe_schema_is_read_only_and_first_class() {
    let tool = PlanReframeTool;
    assert_eq!(tool.name(), "plan_reframe");
    assert!(!tool.is_mutating(&invoke(serde_json::json!({
        "clip_id": "clip-1"
    }))));

    let schema = tool.schema();
    assert_eq!(schema.name, "plan_reframe");
    assert!(schema.description.contains("vertical"));
    assert_eq!(
        schema.input_schema["required"],
        serde_json::json!(["clip_id"])
    );
}

#[tokio::test]
async fn plan_reframe_returns_apply_edl_ready_vertical_reframe_fragment() {
    let dir = match tempfile::tempdir() {
        Ok(dir) => dir,
        Err(err) => panic!("tempdir should create: {err}"),
    };
    let out = match PlanReframeTool
        .handle(
            invoke(serde_json::json!({
                "clip_id": "clip-1",
                "aspect_ratio": "9:16",
                "source_width": 1920,
                "source_height": 1080,
                "subject_center": {"x": 0.60, "y": 0.42},
                "safe_area": "mobile"
            })),
            ctx_at(dir.path()),
        )
        .await
    {
        Ok(out) => out,
        Err(err) => panic!("plan_reframe should succeed: {err:?}"),
    };

    let body: serde_json::Value = match serde_json::from_str(&out.content) {
        Ok(body) => body,
        Err(err) => panic!(
            "plan_reframe should return JSON: {err}; body={}",
            out.content
        ),
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

#[tokio::test]
async fn plan_reframe_rejects_invalid_subject_center() {
    let dir = match tempfile::tempdir() {
        Ok(dir) => dir,
        Err(err) => panic!("tempdir should create: {err}"),
    };
    let err = match PlanReframeTool
        .handle(
            invoke(serde_json::json!({
                "clip_id": "clip-1",
                "subject_center": {"x": 1.4, "y": 0.5}
            })),
            ctx_at(dir.path()),
        )
        .await
    {
        Ok(out) => panic!("plan_reframe should reject invalid center, got {out:?}"),
        Err(err) => err,
    };

    match err {
        FunctionCallError::RespondToModel(message) => {
            assert!(message.contains("subject_center.x must be in 0..=1"));
        }
        other => panic!("expected model-facing validation error, got {other:?}"),
    }
}
