//! `list_assets` bin-filter integration tests.
//!
//! Slice C2 (wave3-bin-aware). Confirms:
//!   - `list_assets` without a `bin` arg is unchanged (lists every
//!     filesystem-discovered raw + render asset).
//!   - `list_assets { bin: "<id>" }` returns only assets whose
//!     `AssetRecord.bin_id` matches the provided id.
//!   - Unknown bin ids return zero results (not an error — the model
//!     should be able to probe).

use awidat_core::FunctionCallError;
use awidat_core::media_catalog_mutation::{
    create_bin, ensure_awidat_metadata, move_asset_to_bin, upsert_asset,
};
use awidat_core::tool::{SandboxMode, ToolContext, ToolHandler, ToolInvocation};
use awidat_core::tools::list_assets::ListAssetsTool;
use awidat_proto::professional::{AssetReadiness, AssetRecord, AssetRole};
use awidat_proto::project::Project;
use std::path::Path;
use tokio::sync::broadcast;

fn ctx_at(root: &Path) -> ToolContext {
    let (tx, _) = broadcast::channel(8);
    ToolContext {
        project_root: root.to_path_buf(),
        events_tx: tx,
        user_input_tx: None,
        job_manager: awidat_render::JobManager::new(),
        approval_tx: None,
        sandbox_mode: SandboxMode::Default,
        mcp_host: awidat_core::mcp_host::McpHost::new(awidat_mcp::ClientInfo {
            name: "test".into(),
            version: "0.0.0".into(),
        }),
        skills: std::sync::Arc::new(awidat_core::skills::SkillRegistry::default()),
        subagent_return: None,
    }
}

fn invoke(args: serde_json::Value) -> ToolInvocation {
    ToolInvocation {
        call_id: "c1".into(),
        name: "list_assets".into(),
        args,
    }
}

fn make(p: &Path, body: &[u8]) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

/// Build a project with three raw assets + one render, two bins, and
/// assign two of the raw assets into bin "scene-1" and one into "broll".
fn project_with_bins() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    // First initialize an empty project, then write files on disk, then
    // populate the catalog with matching asset ids and bins.
    let mut project = Project::init(root).unwrap();
    make(&root.join("raw/a.mp4"), &[0u8; 1024]);
    make(&root.join("raw/b.mp4"), &[0u8; 1024]);
    make(&root.join("raw/c.mp4"), &[0u8; 1024]);
    make(&root.join("renders/out.mp4"), &[0u8; 1024]);

    let meta = ensure_awidat_metadata(&mut project.timeline);
    for path in ["raw/a.mp4", "raw/b.mp4", "raw/c.mp4"] {
        upsert_asset(
            meta,
            AssetRecord {
                id: path.into(),
                path: path.into(),
                role: AssetRole::Video,
                readiness: AssetReadiness::default(),
                ..AssetRecord::default()
            },
        );
    }
    create_bin(meta, "scene-1".into(), "Scene 1".into(), None).unwrap();
    create_bin(meta, "broll".into(), "B-roll".into(), None).unwrap();
    move_asset_to_bin(meta, "raw/a.mp4", Some("scene-1".into())).unwrap();
    move_asset_to_bin(meta, "raw/b.mp4", Some("scene-1".into())).unwrap();
    move_asset_to_bin(meta, "raw/c.mp4", Some("broll".into())).unwrap();
    project.write(root).unwrap();
    dir
}

#[tokio::test]
async fn list_assets_without_bin_filter_is_unchanged() {
    let dir = project_with_bins();
    let out = ListAssetsTool
        .handle(invoke(serde_json::json!({})), ctx_at(dir.path()))
        .await
        .unwrap();
    // 3 raw + 1 render = 4
    assert!(
        out.content.contains("scope=all total=4"),
        "expected total=4 without bin filter, got:\n{}",
        out.content
    );
    assert!(out.content.contains("[raw] a.mp4"));
    assert!(out.content.contains("[raw] b.mp4"));
    assert!(out.content.contains("[raw] c.mp4"));
    assert!(out.content.contains("[renders] out.mp4"));
}

#[tokio::test]
async fn list_assets_with_bin_filter_returns_only_members() {
    let dir = project_with_bins();
    let out = ListAssetsTool
        .handle(
            invoke(serde_json::json!({"bin": "scene-1"})),
            ctx_at(dir.path()),
        )
        .await
        .unwrap();
    assert!(
        out.content.contains("bin=scene-1 total=2"),
        "expected bin=scene-1 total=2, got:\n{}",
        out.content
    );
    assert!(out.content.contains("a.mp4"));
    assert!(out.content.contains("b.mp4"));
    assert!(
        !out.content.contains("c.mp4"),
        "c.mp4 belongs to broll, not scene-1; should be filtered out: {}",
        out.content
    );
    assert!(
        !out.content.contains("out.mp4"),
        "renders are not in bin scene-1; should be filtered out: {}",
        out.content
    );
}

#[tokio::test]
async fn list_assets_unknown_bin_returns_zero_results() {
    let dir = project_with_bins();
    let out = ListAssetsTool
        .handle(
            invoke(serde_json::json!({"bin": "no-such-bin"})),
            ctx_at(dir.path()),
        )
        .await
        .unwrap();
    assert!(
        out.content.contains("total=0"),
        "unknown bin should yield total=0, got:\n{}",
        out.content
    );
}

#[tokio::test]
async fn list_assets_bin_filter_combines_with_scope_raw() {
    let dir = project_with_bins();
    // scope=raw + bin=broll → only c.mp4 (raw).
    let out = ListAssetsTool
        .handle(
            invoke(serde_json::json!({"scope": "raw", "bin": "broll"})),
            ctx_at(dir.path()),
        )
        .await
        .unwrap();
    assert!(out.content.contains("total=1"));
    assert!(out.content.contains("c.mp4"));
    assert!(!out.content.contains("a.mp4"));
    assert!(!out.content.contains("b.mp4"));
}

#[tokio::test]
async fn list_assets_offset_zero_still_errors_with_bin() {
    let dir = project_with_bins();
    let err = ListAssetsTool
        .handle(
            invoke(serde_json::json!({"bin": "scene-1", "offset": 0})),
            ctx_at(dir.path()),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, FunctionCallError::RespondToModel(msg) if msg.contains("1-indexed")));
}
