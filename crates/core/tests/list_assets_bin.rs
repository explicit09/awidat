//! `list_assets` bin-filter integration tests.
//!
//! Slice C2 (wave3-bin-aware). Confirms:
//!   - `list_assets` without a `bin` arg is unchanged (lists every
//!     filesystem-discovered raw + render asset).
//!   - `list_assets { bin: "<id>" }` returns only assets whose
//!     `AssetRecord.bin_id` matches the provided id.
//!   - Unknown bin ids return zero results (not an error — the model
//!     should be able to probe).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use montage_core::media_catalog_mutation::{
    create_bin, ensure_montage_metadata, move_asset_to_bin, upsert_asset,
};
use montage_core::montage_mcp::context::McpToolCtx;
use montage_core::montage_mcp::tools::list_assets::{ListAssetsArgs, run};
use montage_proto::professional::{AssetReadiness, AssetRecord, AssetRole};
use montage_proto::project::Project;
use std::path::Path;

fn ctx_at(root: &Path) -> McpToolCtx {
    McpToolCtx {
        project_root: root.to_path_buf(),
    }
}

fn args(scope: Option<&str>, bin: Option<&str>, offset: Option<usize>) -> ListAssetsArgs {
    ListAssetsArgs {
        scope: scope.map(str::to_string),
        offset,
        limit: None,
        bin: bin.map(str::to_string),
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

    let meta = ensure_montage_metadata(&mut project.timeline);
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

#[test]
fn list_assets_without_bin_filter_is_unchanged() {
    let dir = project_with_bins();
    let out = run(args(None, None, None), ctx_at(dir.path())).unwrap();
    // 3 raw + 1 render = 4
    assert!(
        out.contains("scope=all total=4"),
        "expected total=4 without bin filter, got:\n{out}"
    );
    assert!(out.contains("[raw] a.mp4"));
    assert!(out.contains("[raw] b.mp4"));
    assert!(out.contains("[raw] c.mp4"));
    assert!(out.contains("[renders] out.mp4"));
}

#[test]
fn list_assets_with_bin_filter_returns_only_members() {
    let dir = project_with_bins();
    let out = run(args(None, Some("scene-1"), None), ctx_at(dir.path())).unwrap();
    assert!(
        out.contains("bin=scene-1 total=2"),
        "expected bin=scene-1 total=2, got:\n{out}"
    );
    assert!(out.contains("a.mp4"));
    assert!(out.contains("b.mp4"));
    assert!(
        !out.contains("c.mp4"),
        "c.mp4 belongs to broll, not scene-1; should be filtered out: {out}"
    );
    assert!(
        !out.contains("out.mp4"),
        "renders are not in bin scene-1; should be filtered out: {out}"
    );
}

#[test]
fn list_assets_unknown_bin_returns_zero_results() {
    let dir = project_with_bins();
    let out = run(args(None, Some("no-such-bin"), None), ctx_at(dir.path())).unwrap();
    assert!(
        out.contains("total=0"),
        "unknown bin should yield total=0, got:\n{out}"
    );
}

#[test]
fn list_assets_bin_filter_combines_with_scope_raw() {
    let dir = project_with_bins();
    // scope=raw + bin=broll → only c.mp4 (raw).
    let out = run(args(Some("raw"), Some("broll"), None), ctx_at(dir.path())).unwrap();
    assert!(out.contains("total=1"));
    assert!(out.contains("c.mp4"));
    assert!(!out.contains("a.mp4"));
    assert!(!out.contains("b.mp4"));
}

#[test]
fn list_assets_offset_zero_still_errors_with_bin() {
    let dir = project_with_bins();
    let err = run(args(None, Some("scene-1"), Some(0)), ctx_at(dir.path())).unwrap_err();
    assert!(err.contains("1-indexed"));
}
