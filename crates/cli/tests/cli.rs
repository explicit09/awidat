//! CLI integration tests that exercise the installed `awidat` binary.

#![allow(clippy::unwrap_used)]

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

fn awidat() -> &'static str {
    env!("CARGO_BIN_EXE_awidat")
}

fn tmp_dir(name: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let mut path = std::env::temp_dir();
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    path.push(format!("awidat-cli-{name}-{}-{n}", std::process::id()));
    path
}

fn run(args: &[&str]) -> Output {
    Command::new(awidat()).args(args).output().unwrap()
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn init_creates_project_and_validate_succeeds() {
    let root = tmp_dir("init-validate");
    let root_arg = root.to_string_lossy();

    let init = run(&["init", &root_arg]);
    assert!(init.status.success(), "{}", stderr(&init));
    assert!(root.join("project.otio.json").exists());
    assert!(root.join("edit-plan.json").exists());
    assert!(root.join("index").join("manifest.json").exists());

    let validate = run(&["validate", &root_arg]);
    assert!(validate.status.success(), "{}", stderr(&validate));
    let out = stdout(&validate);
    assert!(out.contains("validates clean"));
    assert!(out.contains("Index:      0 indexer(s), 0 sidecar(s)"));

    fs::remove_dir_all(&root).ok();
}

#[test]
fn init_refuses_nonempty_directory() {
    let root = tmp_dir("nonempty");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("existing.txt"), "keep me").unwrap();
    let root_arg = root.to_string_lossy();

    let output = run(&["init", &root_arg]);
    assert!(!output.status.success());
    assert!(stderr(&output).contains("refusing to init"));

    fs::remove_dir_all(&root).ok();
}

#[test]
fn validate_fails_on_malformed_project_json() {
    let root = tmp_dir("malformed");
    let root_arg = root.to_string_lossy();
    let init = run(&["init", &root_arg]);
    assert!(init.status.success(), "{}", stderr(&init));
    fs::write(root.join("project.otio.json"), "{ bad json").unwrap();

    let output = run(&["validate", &root_arg]);
    assert!(!output.status.success());
    assert!(stderr(&output).contains("invalid JSON"));

    fs::remove_dir_all(&root).ok();
}

#[test]
fn validate_prints_index_path_warnings() {
    let root = tmp_dir("warnings");
    let root_arg = root.to_string_lossy();
    let init = run(&["init", &root_arg]);
    assert!(init.status.success(), "{}", stderr(&init));

    fs::write(
        root.join("index").join("manifest.json"),
        r#"{
  "version": "0.1",
  "indexers": [
    {
      "name": "Bad_Name",
      "version": "1",
      "schema_version": "1",
      "assets": ["../raw/foo.mp4"],
      "last_run": "2026-05-02T00:00:00Z"
    }
  ]
}"#,
    )
    .unwrap();
    let indexer_dir = root.join("index").join("Bad_Name");
    fs::create_dir_all(&indexer_dir).unwrap();
    fs::write(
        indexer_dir.join("foo.mp4.json"),
        r#"{
  "indexer": "Bad_Name",
  "indexer_version": "1",
  "schema_version": "1",
  "asset_id": "../raw/foo.mp4",
  "asset_sha256": "abc",
  "produced_at": "2026-05-02T12:00:00Z",
  "data": {}
}"#,
    )
    .unwrap();

    let output = run(&["validate", &root_arg]);
    assert!(output.status.success(), "{}", stderr(&output));
    let out = stdout(&output);
    assert!(out.contains("Index warnings"));
    assert!(out.contains("not a valid id"));
    assert!(out.contains("asset id '../raw/foo.mp4' is unsafe"));

    fs::remove_dir_all(&root).ok();
}
