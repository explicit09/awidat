//! Integration tests for the `find_audio_asset` agent tool.
//!
//! Validates that the bundled synthetic CC0 SFX starter pack is
//! discoverable through the tool, can be filtered by `kind` + `mood`,
//! and respects `max_duration_s`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};

use montage_core::tools::find_audio_asset::{FindAudioAssetTool, find_audio_assets};
use montage_core::{ToolContext, ToolHandler, ToolInvocation};
use tokio::sync::broadcast;

fn ctx_at(root: &Path) -> ToolContext {
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
        name: "find_audio_asset".into(),
        args,
    }
}

/// Resolve the workspace root from this integration test's
/// `CARGO_MANIFEST_DIR`. The bundled pack lives at
/// `<workspace>/assets/audio/`.
fn pack_root() -> PathBuf {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    // CARGO_MANIFEST_DIR = <workspace>/crates/core
    let workspace = crate_dir
        .parent()
        .and_then(|p| p.parent())
        .unwrap_or_else(|| panic!("workspace root above crates/core"));
    workspace.join("assets").join("audio")
}

#[test]
fn schema_is_read_only_and_declares_required_kind() {
    let tool = FindAudioAssetTool;
    assert_eq!(tool.name(), "find_audio_asset");
    assert!(!tool.is_mutating(&invoke(serde_json::json!({ "kind": "sfx" }))));

    let schema = tool.schema();
    assert_eq!(schema.name, "find_audio_asset");
    assert_eq!(schema.input_schema["required"], serde_json::json!(["kind"]));
    let enum_vals = &schema.input_schema["properties"]["kind"]["enum"];
    assert!(enum_vals.is_array(), "kind must be an enum");
    let kinds: Vec<&str> = match enum_vals.as_array() {
        Some(arr) => arr.iter().filter_map(|v| v.as_str()).collect(),
        None => panic!("kind enum must be an array"),
    };
    assert!(kinds.contains(&"sfx"));
    assert!(kinds.contains(&"music"));
    assert!(kinds.contains(&"ambience"));
}

#[test]
fn returns_hype_whoosh_when_mood_matches() {
    let results = find_audio_assets(&pack_root(), "sfx", Some("hype"), None)
        .unwrap_or_else(|e| panic!("pack should load cleanly: {e}"));
    assert!(!results.is_empty(), "should find at least one hype SFX");
    let top = &results[0];
    assert_eq!(top.slug, "whoosh_hype");
    assert_eq!(top.kind, "sfx");
    assert!(
        top.path.ends_with("whoosh_hype.wav"),
        "path should end with whoosh_hype.wav, got {}",
        top.path.display()
    );
    assert!(
        top.path.is_absolute(),
        "path should be absolute, got {}",
        top.path.display()
    );
    assert!(top.path.exists(), "the resolved wav file must exist");
}

#[test]
fn respects_max_duration_s() {
    let results = find_audio_assets(&pack_root(), "sfx", None, Some(0.5))
        .unwrap_or_else(|e| panic!("pack should load: {e}"));
    assert!(!results.is_empty(), "should find at least one short SFX");
    for r in &results {
        assert!(
            r.duration_s <= 0.5 + 1e-9,
            "all results must be <= 0.5s, got {} for {}",
            r.duration_s,
            r.slug
        );
        assert_eq!(r.kind, "sfx");
    }
}

#[test]
fn filters_by_kind_ambience() {
    let results = find_audio_assets(&pack_root(), "ambience", None, None)
        .unwrap_or_else(|e| panic!("pack should load: {e}"));
    assert!(!results.is_empty(), "starter pack should ship an ambience");
    for r in &results {
        assert_eq!(r.kind, "ambience");
    }
}

#[test]
fn empty_results_for_unknown_mood_is_not_an_error() {
    let results = find_audio_assets(
        &pack_root(),
        "sfx",
        Some("definitely-not-in-the-pack-xyz"),
        None,
    )
    .unwrap_or_else(|e| panic!("unknown mood should be empty, not error: {e}"));
    assert!(results.is_empty(), "expected empty results");
}

#[tokio::test]
async fn tool_handle_returns_json_with_results_and_resolves_default_pack() {
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
    let out = FindAudioAssetTool
        .handle(
            invoke(serde_json::json!({
                "kind": "sfx",
                "mood": "hype"
            })),
            ctx_at(dir.path()),
        )
        .await
        .unwrap_or_else(|e| panic!("tool should succeed: {e:?}"));

    let body: serde_json::Value = serde_json::from_str(&out.content)
        .unwrap_or_else(|e| panic!("tool should return JSON: {e}"));
    let results = body["results"]
        .as_array()
        .unwrap_or_else(|| panic!("results must be array, body={body}"));
    // Tool resolves the default pack via CARGO_MANIFEST_DIR-relative
    // discovery; if the workspace pack ships the hype whoosh, the tool
    // should surface it as the top result.
    assert!(!results.is_empty(), "expected at least one result");
    assert_eq!(results[0]["slug"].as_str(), Some("whoosh_hype"));
    assert_eq!(results[0]["kind"].as_str(), Some("sfx"));
}
