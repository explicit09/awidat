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
use montage_core::montage_mcp::context::McpToolCtx;
use montage_core::montage_mcp::tools::create_stringout::{self, CreateStringoutArgs};
use montage_core::montage_mcp::tools::list_bins::{self, ListBinsArgs};
use montage_core::montage_mcp::tools::list_stringouts::{self, ListStringoutsArgs};
use montage_proto::project::Project;
use std::path::Path;

fn ctx_at(root: &Path) -> McpToolCtx {
    McpToolCtx {
        project_root: root.to_path_buf(),
    }
}

fn init_project_with_bins(root: &Path) {
    let mut project = Project::init(root).unwrap();
    let meta = ensure_montage_metadata(&mut project.timeline);
    create_bin(meta, "scene-1".into(), "Scene 1".into(), None).unwrap();
    create_bin(meta, "broll".into(), "B-roll".into(), None).unwrap();
    project.write(root).unwrap();
}

#[test]
fn list_stringouts_empty_then_after_creation() {
    let dir = tempfile::tempdir().unwrap();
    init_project_with_bins(dir.path());

    // Empty up front.
    let out = list_stringouts::run(ListStringoutsArgs {}, ctx_at(dir.path())).unwrap();
    assert!(out.contains("total=0"));

    // Create two stringouts (multi-cardinality).
    create_stringout::run(
        CreateStringoutArgs {
            id: "stringout-cold-open".to_string(),
            name: Some("Cold open".to_string()),
            items: vec!["select-a".to_string(), "select-b".to_string()],
        },
        ctx_at(dir.path()),
    )
    .unwrap();
    create_stringout::run(
        CreateStringoutArgs {
            id: "stringout-arc-2".to_string(),
            name: Some("Arc 2".to_string()),
            items: vec!["select-c".to_string()],
        },
        ctx_at(dir.path()),
    )
    .unwrap();

    let out = list_stringouts::run(ListStringoutsArgs {}, ctx_at(dir.path())).unwrap();
    assert!(out.contains("total=2"), "expected total=2, got:\n{out}");
    assert!(out.contains("stringout-cold-open"));
    assert!(out.contains("stringout-arc-2"));
    // Items count surfaced
    assert!(out.contains("items=2"));
    assert!(out.contains("items=1"));
}

#[test]
fn create_stringout_rejects_duplicate_id() {
    let dir = tempfile::tempdir().unwrap();
    init_project_with_bins(dir.path());
    create_stringout::run(
        CreateStringoutArgs {
            id: "stringout-x".to_string(),
            name: Some("X".to_string()),
            items: Vec::new(),
        },
        ctx_at(dir.path()),
    )
    .unwrap();
    let err = create_stringout::run(
        CreateStringoutArgs {
            id: "stringout-x".to_string(),
            name: Some("X again".to_string()),
            items: Vec::new(),
        },
        ctx_at(dir.path()),
    )
    .unwrap_err();
    assert!(err.contains("already exists"));
}

#[test]
fn create_stringout_requires_non_empty_id() {
    let dir = tempfile::tempdir().unwrap();
    init_project_with_bins(dir.path());
    let err = create_stringout::run(
        CreateStringoutArgs {
            id: "  ".to_string(),
            name: Some("Whitespace".to_string()),
            items: Vec::new(),
        },
        ctx_at(dir.path()),
    )
    .unwrap_err();
    assert!(err.contains("empty"));
}

#[test]
fn list_bins_includes_user_defined_and_built_in_roles() {
    let dir = tempfile::tempdir().unwrap();
    init_project_with_bins(dir.path());

    let out = list_bins::run(ListBinsArgs {}, ctx_at(dir.path())).unwrap();
    // User-defined.
    assert!(out.contains("scene-1"));
    assert!(out.contains("broll"));
    // Built-in role buckets. Kdenlive surfaces role buckets like
    // "Audio Clips" / "Video Clips" — we expose them as virtual bins
    // with stable ids "role:video", "role:audio", etc. so the agent can
    // filter on them without the user manually creating bins.
    assert!(out.contains("role:video"));
    assert!(out.contains("role:audio"));
    assert!(out.contains("role:still"));
    assert!(out.contains("role:graphic"));
    assert!(out.contains("role:caption"));
    assert!(out.contains("role:support"));
    // Marker that distinguishes built-in from user-defined.
    assert!(out.contains("kind=user"));
    assert!(out.contains("kind=role"));
}

#[test]
fn list_bins_on_empty_project_still_lists_role_buckets() {
    let dir = tempfile::tempdir().unwrap();
    Project::init(dir.path()).unwrap();

    let out = list_bins::run(ListBinsArgs {}, ctx_at(dir.path())).unwrap();
    // No user-defined bins yet but role buckets always appear.
    assert!(out.contains("role:video"));
    assert!(out.contains("kind=role"));
}
