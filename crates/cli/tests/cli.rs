//! CLI integration tests that exercise the installed `montage` binary.

#![allow(clippy::unwrap_used)]

mod common;

use std::fs;

use common::{make_tiny_mp4, run, stderr, stdout, tmp_dir};

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
fn new_import_places_source_on_timeline() {
    let parent = tmp_dir("new-import-parent");
    fs::create_dir_all(&parent).unwrap();
    let source = parent.join("source.mp4");
    make_tiny_mp4(&source);

    let parent_arg = parent.to_string_lossy();
    let source_arg = source.to_string_lossy();
    let output = run(&[
        "new",
        "imported",
        "--at",
        &parent_arg,
        "--import",
        &source_arg,
        "--link",
        "--no-index",
    ]);
    assert!(output.status.success(), "{}", stderr(&output));
    let out = stdout(&output);
    assert!(out.contains("Added imported source to timeline track V1"));

    let project_root = parent.join("imported");
    let timeline: serde_json::Value =
        serde_json::from_slice(&fs::read(project_root.join("project.otio.json")).unwrap()).unwrap();
    assert_eq!(
        timeline["metadata"]["montage"]["source_assets"][0],
        "raw/source.mp4"
    );
    assert_eq!(timeline["tracks"]["children"][0]["name"], "V1");
    let clip = &timeline["tracks"]["children"][0]["children"][0];
    assert_eq!(clip["name"], "source");
    assert_eq!(
        clip["media_reference"]["target_url"],
        serde_json::Value::String("raw/source.mp4".to_string())
    );
    assert!(clip["source_range"]["duration"]["value"].as_f64().unwrap() > 0.0);

    fs::remove_dir_all(&parent).ok();
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

#[test]
fn visual_qa_motion_scenes_reports_structural_issues() {
    let root = tmp_dir("visual-qa-motion-scenes");
    let root_arg = root.to_string_lossy();
    let init = run(&["init", &root_arg]);
    assert!(init.status.success(), "{}", stderr(&init));

    let project_path = root.join("project.otio.json");
    let mut timeline: serde_json::Value =
        serde_json::from_slice(&fs::read(&project_path).unwrap()).unwrap();
    timeline["metadata"]["montage"]["motion_scenes"] = serde_json::json!([
        {
            "id": "bad-scene",
            "start_s": 0.0,
            "duration_s": 4.0,
            "fps": 30.0,
            "width": 1920,
            "height": 1080,
            "layers": [
                {
                    "id": "veil",
                    "kind": "solid",
                    "from_s": 0.0,
                    "duration_s": 4.0,
                    "z_index": 0,
                    "params": {"x": 0.0, "y": 0.0, "width": 1.0, "height": 1.0}
                }
            ]
        }
    ]);
    fs::write(&project_path, serde_json::to_vec_pretty(&timeline).unwrap()).unwrap();

    let output = run(&["visual-qa", "motion-scenes", &root_arg]);
    assert!(!output.status.success());
    let out = stdout(&output);
    assert!(out.contains("bad-scene"));
    assert!(out.contains("full_frame_backing"));

    fs::remove_dir_all(&root).ok();
}
