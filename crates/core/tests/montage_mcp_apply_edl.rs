//! Integration tests for the production `apply_edl` tool surface:
//! `montage_core::montage_mcp::tools::apply_edl::run`. This is the
//! path the real `montage-mcp-server` binary dispatches into — the
//! legacy `crate::tools::ToolHandler` tree it once had to be
//! distinguished from is gone (see `docs/risk-register-2026-07-15.md`).
//!
//! Mirrors editorial_workflow.rs's fixture-project shape but drives
//! `montage_mcp::tools::apply_edl::run` directly with a hand-built
//! `McpToolCtx`, so no env vars or child processes are involved.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::path::Path;

use montage_core::montage_mcp::context::McpToolCtx;
use montage_core::montage_mcp::tools::apply_edl::{self, ApplyEdlArgs};
use montage_proto::otio::{
    Clip, ExternalReference, MediaReference, RationalTime, StackChild, TimeRange, Track,
    TrackChild, TrackKind,
};
use montage_proto::project::Project;

/// Seeds a temp project with 3 clips on V1, each backed by a raw asset
/// file that actually exists on disk (so asset-existence checks that
/// read other clips don't spuriously fail).
fn seed_project() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut project = Project::init(dir.path()).expect("Project::init");
    std::fs::create_dir_all(dir.path().join("raw")).expect("raw dir");
    let mut track = Track::empty("V1", TrackKind::Video);
    for i in 0..3 {
        let asset = format!("raw/ep-{i}.mp4");
        std::fs::write(dir.path().join(&asset), b"fake media").expect("write asset");
        let mut clip = Clip::empty(format!("clip-{i}"));
        clip.media_reference = MediaReference::External(ExternalReference::new(asset));
        clip.source_range = Some(TimeRange::new(
            RationalTime::new(0.0, 24.0),
            RationalTime::new(5.0 * 24.0, 24.0),
        ));
        track.children.push(TrackChild::Clip(clip));
    }
    project
        .timeline
        .tracks
        .children
        .push(StackChild::Track(track));
    project.write(dir.path()).expect("project write");
    dir
}

fn ctx_at(root: &Path) -> McpToolCtx {
    McpToolCtx {
        project_root: root.to_path_buf(),
    }
}

fn args(edl: &str, dry_run: bool) -> ApplyEdlArgs {
    ApplyEdlArgs {
        edl: edl.to_string(),
        dry_run,
        reasoning: None,
    }
}

#[test]
fn dry_run_returns_plan_and_mutates_nothing_on_disk() {
    let dir = seed_project();
    let before = std::fs::read_to_string(dir.path().join("project.otio.json")).unwrap();

    let edl = "\
*** Begin EDL
*** Delete Clip
@@ anchor: clip_uuid=clip-1
*** End EDL
";
    let out = apply_edl::run(args(edl, true), ctx_at(dir.path())).expect("dry run succeeds");
    assert!(out.contains("DRY RUN"), "{out}");
    assert!(out.contains("Validated 1 op"), "{out}");

    let after = std::fs::read_to_string(dir.path().join("project.otio.json")).unwrap();
    assert_eq!(after, before, "dry_run must not touch project.otio.json");
}

#[test]
fn commit_path_applies_op_and_writes_project() {
    let dir = seed_project();

    let edl = "\
*** Begin EDL
*** Delete Clip
@@ anchor: clip_uuid=clip-1
*** End EDL
";
    let out = apply_edl::run(args(edl, false), ctx_at(dir.path())).expect("commit succeeds");
    assert!(out.contains("committed 1 op(s)"), "{out}");

    let after = Project::read(dir.path()).expect("re-read project");
    let StackChild::Track(t) = &after.timeline.tracks.children[0] else {
        panic!("expected track at index 0");
    };
    // clip-1 replaced by a timing-preserving gap; clip-0/clip-2 remain.
    assert_eq!(t.children.len(), 3);
    assert!(matches!(&t.children[0], TrackChild::Clip(c) if c.name == "clip-0"));
    assert!(matches!(&t.children[1], TrackChild::Gap(_)));
    assert!(matches!(&t.children[2], TrackChild::Clip(c) if c.name == "clip-2"));
}

#[test]
fn picture_locked_project_rejects_picture_mutating_edl() {
    let dir = seed_project();
    montage_core::picture_lock::set_picture_lock(dir.path(), true, Some("gates passed".into()))
        .expect("set lock");
    let before = std::fs::read_to_string(dir.path().join("project.otio.json")).unwrap();

    let edl = "\
*** Begin EDL
*** Delete Clip
@@ anchor: clip_uuid=clip-1
*** End EDL
";
    let err = apply_edl::run(args(edl, false), ctx_at(dir.path())).unwrap_err();
    assert!(err.contains("picture is locked"), "{err}");

    let after = std::fs::read_to_string(dir.path().join("project.otio.json")).unwrap();
    assert_eq!(after, before, "rejected commit must not touch the project");
}

#[test]
fn picture_locked_project_allows_sound_only_edl() {
    let dir = seed_project();
    montage_core::picture_lock::set_picture_lock(dir.path(), true, Some("gates passed".into()))
        .expect("set lock");

    let edl = "\
*** Begin EDL
*** Set Volume
@@ anchor: clip_uuid=clip-0
+ value: 0.5
*** End EDL
";
    let out = apply_edl::run(args(edl, false), ctx_at(dir.path()))
        .expect("sound-only op succeeds under lock");
    assert!(out.contains("committed 1 op(s)"), "{out}");
}

#[test]
fn insert_clip_with_missing_asset_errors_and_leaves_project_unchanged() {
    let dir = seed_project();
    let before = std::fs::read_to_string(dir.path().join("project.otio.json")).unwrap();

    let edl = "\
*** Begin EDL
*** Insert Clip
+ asset: raw/does-not-exist.mp4
+ track: V1
+ start: 0
+ end: 1
*** End EDL
";
    let err = apply_edl::run(args(edl, false), ctx_at(dir.path())).unwrap_err();
    assert!(err.contains("no such file"), "{err}");
    assert!(err.contains("does-not-exist.mp4"), "{err}");

    let after = std::fs::read_to_string(dir.path().join("project.otio.json")).unwrap();
    assert_eq!(
        after, before,
        "asset-existence rejection must not touch the project"
    );
}

/// pre_apply_edl hooks run via `Command::new("bash")` reading
/// `[hooks] pre_apply_edl` from `<project>/.montage/config.toml`.
/// A hook that exits nonzero should block the commit before any disk
/// write. Fully hermetic setup (hook script + config.toml both live
/// under the temp project dir); `Config::load()` also consults a
/// user-global config at `~/.config/montage/config.toml`, but that
/// layer is skipped entirely unless the file exists, so this test has
/// no dependency on the machine's real global config as long as none
/// is present. Previously blocked by a bug in
/// `montage_config::Config::overlay` (crates/config/src/lib.rs) that
/// unconditionally returned `hooks: HooksConfig::default()` instead of
/// threading through `project.hooks`; now fixed with a per-field merge.
#[test]
fn failing_pre_apply_edl_hook_blocks_commit() {
    let dir = seed_project();
    let before = std::fs::read_to_string(dir.path().join("project.otio.json")).unwrap();

    let montage_dir = dir.path().join(".montage");
    std::fs::create_dir_all(&montage_dir).expect("create .montage dir");

    let hook_path = montage_dir.join("reject_hook.sh");
    std::fs::write(
        &hook_path,
        "#!/bin/bash\necho 'hook rejects this edit' >&2\nexit 1\n",
    )
    .expect("write hook script");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&hook_path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&hook_path, perms).unwrap();
    }

    let config_path = montage_dir.join("config.toml");
    std::fs::write(
        &config_path,
        format!(
            "[hooks]\npre_apply_edl = \"bash {}\"\n",
            hook_path.display()
        ),
    )
    .expect("write config.toml");

    let edl = "\
*** Begin EDL
*** Delete Clip
@@ anchor: clip_uuid=clip-1
*** End EDL
";
    let err = apply_edl::run(args(edl, false), ctx_at(dir.path())).unwrap_err();
    assert!(
        err.contains("pre_apply_edl") || err.contains("pre-hook"),
        "{err}"
    );
    assert!(err.contains("rejected"), "{err}");

    let after = std::fs::read_to_string(dir.path().join("project.otio.json")).unwrap();
    assert_eq!(
        after, before,
        "a rejecting pre_apply_edl hook must block the write"
    );
}
