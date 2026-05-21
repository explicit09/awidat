//! Integration tests for the version-control author override.
//!
//! Slice wave1-vedit-author: commits used to be hardcoded to
//! `awidat agent <agent@awidat.local>`. These tests pin the new
//! contract:
//!
//!   1. An explicit `CommitAuthor` passed to `commit_current_timeline_as`
//!      is preserved end-to-end (round-trip through commit -> log).
//!   2. The legacy `commit_current_timeline` entry point keeps stamping
//!      the "awidat agent" default — backward compat.
//!
//! The third leg of the slice — the env-var fallback — lives as a unit
//! test next to `resolve_commit_author` itself. Rust 2024 marks
//! `std::env::set_var` as `unsafe`, and the workspace forbids unsafe
//! code, so the env-var contract is exercised via a process-env-free
//! callback seam (`resolve_commit_author_with_env`) instead of by
//! mutating the test process's env.

use std::path::Path;

use awidat_core::vc::{
    CommitAuthor, commit_current_timeline, commit_current_timeline_as, log, open_or_init,
};

fn write_minimal_otio(path: &Path, name: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    let v = serde_json::json!({
        "OTIO_SCHEMA": "Timeline.1",
        "name": name,
        "tracks": {
            "OTIO_SCHEMA": "Stack.1",
            "name": "tracks",
            "children": []
        }
    });
    std::fs::write(path, serde_json::to_vec_pretty(&v).unwrap()).unwrap();
}

#[test]
fn explicit_author_override_round_trips_through_log() {
    let dir = tempfile::tempdir().unwrap();
    write_minimal_otio(&dir.path().join("project.otio.json"), "test");
    let repo = open_or_init(dir.path()).unwrap();

    let alice = CommitAuthor {
        name: "Alice".to_string(),
        email: "alice@example.com".to_string(),
    };
    let outcome = commit_current_timeline_as(
        &repo,
        "Trim drone_shot_04 -1.8s",
        Some("Tighter pacing requested."),
        Some(alice.clone()),
    )
    .unwrap();

    let entries = log(&repo, 5).unwrap();
    let entry = entries
        .iter()
        .find(|e| e.commit_hash == outcome.commit_hash)
        .expect("just-written commit must appear in log");
    assert_eq!(entry.author.name, alice.name);
    assert_eq!(entry.author.email, alice.email);
}

#[test]
fn legacy_commit_path_stamps_resolved_default_author() {
    // Backward compat: existing call sites that pass no author must
    // still produce a commit whose author resolves through the standard
    // chain. With env vars unset on the host running this test, the
    // chain falls back to the "awidat agent" default. Should the host
    // happen to have AWIDAT_USER_* set (CI runner, dev shell), we
    // accept that the resolver honored it — the contract is "no
    // override -> resolver decides", not "always the agent default".
    let dir = tempfile::tempdir().unwrap();
    write_minimal_otio(&dir.path().join("project.otio.json"), "test");
    let repo = open_or_init(dir.path()).unwrap();

    let outcome = commit_current_timeline(&repo, "Initial", None).unwrap();
    let entries = log(&repo, 5).unwrap();
    let entry = entries
        .iter()
        .find(|e| e.commit_hash == outcome.commit_hash)
        .expect("just-written commit must appear in log");

    match (
        std::env::var("AWIDAT_USER_NAME").ok(),
        std::env::var("AWIDAT_USER_EMAIL").ok(),
    ) {
        (Some(n), Some(e)) if !n.trim().is_empty() && !e.trim().is_empty() => {
            // Host has env-var identity configured; resolver should
            // have honored it.
            assert_eq!(entry.author.name, n.trim());
            assert_eq!(entry.author.email, e.trim());
        }
        _ => {
            // No env-var identity — fall back to the agent default.
            assert_eq!(entry.author.name, "awidat agent");
            assert_eq!(entry.author.email, "agent@awidat.local");
        }
    }
}
