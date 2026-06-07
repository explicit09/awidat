//! Stringout multi-cardinality + listing/creation tool contract tests.
//!
//! Slice C2 (wave3-bin-aware). Confirms:
//!   - `list_stringouts` returns the multi-cardinality `meta.stringouts`
//!     vector, not just a single object.
//!   - `create_stringout` appends a new named stringout with the given
//!     ordered items; calling it twice keeps both.
//!   - `list_bins` returns user-defined bins plus the built-in role
//!     buckets, identifiable by a kind marker.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use montage_core::media_catalog_mutation::{create_bin, ensure_montage_metadata};
use montage_core::tool::{SandboxMode, ToolContext, ToolHandler, ToolInvocation};
use montage_core::tools::create_stringout::CreateStringoutTool;
use montage_core::tools::list_bins::ListBinsTool;
use montage_core::tools::list_stringouts::ListStringoutsTool;
use montage_proto::project::Project;
use std::path::Path;
use tokio::sync::broadcast;

fn ctx_at(root: &Path) -> ToolContext {
    let (tx, _) = broadcast::channel(8);
    ToolContext {
        project_root: root.to_path_buf(),
        events_tx: tx,
        user_input_tx: None,
        job_manager: montage_render::JobManager::new(),
        approval_tx: None,
        sandbox_mode: SandboxMode::Default,
        mcp_host: montage_core::mcp_host::McpHost::new(montage_mcp::ClientInfo {
            name: "test".into(),
            version: "0.0.0".into(),
        }),
        skills: std::sync::Arc::new(montage_core::skills::SkillRegistry::default()),
        subagent_return: None,
    }
}

fn invoke(name: &str, args: serde_json::Value) -> ToolInvocation {
    ToolInvocation {
        call_id: "c1".into(),
        name: name.into(),
        args,
    }
}

fn init_project_with_bins(root: &Path) {
    let mut project = Project::init(root).unwrap();
    let meta = ensure_montage_metadata(&mut project.timeline);
    create_bin(meta, "scene-1".into(), "Scene 1".into(), None).unwrap();
    create_bin(meta, "broll".into(), "B-roll".into(), None).unwrap();
    project.write(root).unwrap();
}

#[tokio::test]
async fn list_stringouts_empty_then_after_creation() {
    let dir = tempfile::tempdir().unwrap();
    init_project_with_bins(dir.path());

    // Empty up front.
    let out = ListStringoutsTool
        .handle(
            invoke("list_stringouts", serde_json::json!({})),
            ctx_at(dir.path()),
        )
        .await
        .unwrap();
    assert!(out.content.contains("total=0"));

    // Create two stringouts (multi-cardinality).
    CreateStringoutTool
        .handle(
            invoke(
                "create_stringout",
                serde_json::json!({
                    "id": "stringout-cold-open",
                    "name": "Cold open",
                    "items": ["select-a", "select-b"],
                }),
            ),
            ctx_at(dir.path()),
        )
        .await
        .unwrap();
    CreateStringoutTool
        .handle(
            invoke(
                "create_stringout",
                serde_json::json!({
                    "id": "stringout-arc-2",
                    "name": "Arc 2",
                    "items": ["select-c"],
                }),
            ),
            ctx_at(dir.path()),
        )
        .await
        .unwrap();

    let out = ListStringoutsTool
        .handle(
            invoke("list_stringouts", serde_json::json!({})),
            ctx_at(dir.path()),
        )
        .await
        .unwrap();
    assert!(
        out.content.contains("total=2"),
        "expected total=2, got:\n{}",
        out.content
    );
    assert!(out.content.contains("stringout-cold-open"));
    assert!(out.content.contains("stringout-arc-2"));
    // Items count surfaced
    assert!(out.content.contains("items=2"));
    assert!(out.content.contains("items=1"));
}

#[tokio::test]
async fn create_stringout_rejects_duplicate_id() {
    let dir = tempfile::tempdir().unwrap();
    init_project_with_bins(dir.path());
    CreateStringoutTool
        .handle(
            invoke(
                "create_stringout",
                serde_json::json!({"id": "stringout-x", "name": "X"}),
            ),
            ctx_at(dir.path()),
        )
        .await
        .unwrap();
    let err = CreateStringoutTool
        .handle(
            invoke(
                "create_stringout",
                serde_json::json!({"id": "stringout-x", "name": "X again"}),
            ),
            ctx_at(dir.path()),
        )
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        montage_core::FunctionCallError::RespondToModel(msg) if msg.contains("already exists")
    ));
}

#[tokio::test]
async fn create_stringout_requires_non_empty_id() {
    let dir = tempfile::tempdir().unwrap();
    init_project_with_bins(dir.path());
    let err = CreateStringoutTool
        .handle(
            invoke(
                "create_stringout",
                serde_json::json!({"id": "  ", "name": "Whitespace"}),
            ),
            ctx_at(dir.path()),
        )
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        montage_core::FunctionCallError::RespondToModel(_)
    ));
}

#[tokio::test]
async fn list_bins_includes_user_defined_and_built_in_roles() {
    let dir = tempfile::tempdir().unwrap();
    init_project_with_bins(dir.path());

    let out = ListBinsTool
        .handle(
            invoke("list_bins", serde_json::json!({})),
            ctx_at(dir.path()),
        )
        .await
        .unwrap();
    // User-defined.
    assert!(out.content.contains("scene-1"));
    assert!(out.content.contains("broll"));
    // Built-in role buckets. Kdenlive surfaces role buckets like
    // "Audio Clips" / "Video Clips" — we expose them as virtual bins
    // with stable ids "role:video", "role:audio", etc. so the agent can
    // filter on them without the user manually creating bins.
    assert!(out.content.contains("role:video"));
    assert!(out.content.contains("role:audio"));
    assert!(out.content.contains("role:still"));
    assert!(out.content.contains("role:graphic"));
    assert!(out.content.contains("role:caption"));
    assert!(out.content.contains("role:support"));
    // Marker that distinguishes built-in from user-defined.
    assert!(out.content.contains("kind=user"));
    assert!(out.content.contains("kind=role"));
}

#[tokio::test]
async fn list_bins_on_empty_project_still_lists_role_buckets() {
    let dir = tempfile::tempdir().unwrap();
    Project::init(dir.path()).unwrap();

    let out = ListBinsTool
        .handle(
            invoke("list_bins", serde_json::json!({})),
            ctx_at(dir.path()),
        )
        .await
        .unwrap();
    // No user-defined bins yet but role buckets always appear.
    assert!(out.content.contains("role:video"));
    assert!(out.content.contains("kind=role"));
}
