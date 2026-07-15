//! Integration tests for the `find_audio_asset` agent tool.
//!
//! Validates that the bundled synthetic CC0 SFX starter pack is
//! discoverable through the tool, can be filtered by `kind` + `mood`,
//! and respects `max_duration_s`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

use montage_core::montage_mcp::context::McpToolCtx;
use montage_core::montage_mcp::tools::find_audio_asset::{
    FindAudioAssetArgs, find_audio_assets, run,
};

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

#[test]
fn tool_run_returns_json_with_results_and_resolves_default_pack() {
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
    let out = run(
        FindAudioAssetArgs {
            kind: "sfx".to_string(),
            mood: Some("hype".to_string()),
            max_duration_s: None,
            max_results: None,
        },
        McpToolCtx {
            project_root: dir.path().to_path_buf(),
        },
    )
    .unwrap_or_else(|e| panic!("tool should succeed: {e:?}"));

    let body: serde_json::Value =
        serde_json::from_str(&out).unwrap_or_else(|e| panic!("tool should return JSON: {e}"));
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
