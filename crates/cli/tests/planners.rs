//! CLI planner integration tests.

#![allow(clippy::unwrap_used)]

mod common;

use std::fs;

use common::{make_dead_air_mp4, make_tone_mp4, run, stderr, stdout, tmp_dir};

fn write_whisper_segments(project_root: &std::path::Path, asset: &str) {
    let path = project_root
        .join("index")
        .join("whisper")
        .join(format!("{asset}.json"));
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let payload = serde_json::json!({
        "indexer": "whisper",
        "asset_id": asset,
        "data": {
            "segments": [
                {
                    "start_s": 0.0,
                    "end_s": 1.2,
                    "text": "Um this setup should go away"
                },
                {
                    "start_s": 1.5,
                    "end_s": 2.2,
                    "text": "get those"
                },
                {
                    "start_s": 2.2,
                    "end_s": 3.5,
                    "text": "papers signed today"
                }
            ]
        }
    });
    fs::write(path, serde_json::to_vec_pretty(&payload).unwrap()).unwrap();
}

fn write_setup_transcript(project_root: &std::path::Path, asset: &str) {
    let path = project_root
        .join("index")
        .join("whisper")
        .join(format!("{asset}.json"));
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let payload = serde_json::json!({
        "indexer": "whisper",
        "asset_id": asset,
        "data": {
            "segments": [
                {
                    "start_s": 0.0,
                    "end_s": 1.2,
                    "text": "Um like this opening setup should go away"
                },
                {
                    "start_s": 1.2,
                    "end_s": 2.5,
                    "text": "we were not too passionate about the old idea"
                },
                {
                    "start_s": 2.5,
                    "end_s": 4.0,
                    "text": "biggest thing to all the founders is get those papers signed"
                }
            ]
        }
    });
    fs::write(path, serde_json::to_vec_pretty(&payload).unwrap()).unwrap();
}

fn write_false_start_words(project_root: &std::path::Path, asset: &str) {
    let path = project_root
        .join("index")
        .join("whisper")
        .join(format!("{asset}.json"));
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let payload = serde_json::json!({
        "indexer": "whisper",
        "asset_id": asset,
        "data": {
            "words": [
                {"start_s": 0.0, "end_s": 0.3, "text": "So"},
                {"start_s": 0.3, "end_s": 0.7, "text": "this"},
                {"start_s": 0.7, "end_s": 1.0, "text": "wait,"},
                {"start_s": 1.0, "end_s": 1.4, "text": "actually"},
                {"start_s": 1.4, "end_s": 2.0, "text": "keep"},
                {"start_s": 2.0, "end_s": 6.0, "text": "going"}
            ]
        }
    });
    fs::write(path, serde_json::to_vec_pretty(&payload).unwrap()).unwrap();
}

#[test]
fn plan_dead_air_edl_emits_applyable_cleanup_edl() {
    let parent = tmp_dir("plan-dead-air-parent");
    fs::create_dir_all(&parent).unwrap();
    let source = parent.join("dead-air.mp4");
    make_dead_air_mp4(&source);

    let parent_arg = parent.to_string_lossy();
    let source_arg = source.to_string_lossy();
    let new_output = run(&[
        "new",
        "dead-air-project",
        "--at",
        &parent_arg,
        "--import",
        &source_arg,
        "--link",
        "--no-index",
    ]);
    assert!(new_output.status.success(), "{}", stderr(&new_output));

    let project_root = parent.join("dead-air-project");
    let project_arg = project_root.to_string_lossy();
    let plan = run(&[
        "plan-dead-air-edl",
        &project_arg,
        "--min-duration-s",
        "0.8",
        "--silence-threshold-db",
        "-40",
    ]);
    assert!(plan.status.success(), "{}", stderr(&plan));
    let edl = stdout(&plan);
    assert!(edl.contains("*** Begin EDL"), "{edl}");
    assert!(edl.contains("*** Trim Clip"), "{edl}");
    assert!(edl.contains("*** Insert Clip"), "{edl}");
    assert!(edl.contains("clip_uuid=clip-dead-air"), "{edl}");

    let edl_path = project_root.join("planned.edl");
    fs::write(&edl_path, edl).unwrap();
    let apply = run(&["apply-edl", &project_arg, &edl_path.to_string_lossy()]);
    assert!(apply.status.success(), "{}", stderr(&apply));

    fs::remove_dir_all(&parent).ok();
}

#[test]
fn plan_transcript_trim_edl_emits_applyable_phrase_anchor_trim() {
    let parent = tmp_dir("plan-transcript-trim-parent");
    fs::create_dir_all(&parent).unwrap();
    let source = parent.join("source.mp4");
    make_tone_mp4(&source, "4");

    let parent_arg = parent.to_string_lossy();
    let source_arg = source.to_string_lossy();
    let new_output = run(&[
        "new",
        "transcript-project",
        "--at",
        &parent_arg,
        "--import",
        &source_arg,
        "--link",
        "--no-index",
    ]);
    assert!(new_output.status.success(), "{}", stderr(&new_output));

    let project_root = parent.join("transcript-project");
    write_whisper_segments(&project_root, "raw/source.mp4");
    let project_arg = project_root.to_string_lossy();
    let plan = run(&[
        "plan-transcript-trim-edl",
        &project_arg,
        "--keep-from-phrase",
        "get those papers",
    ]);
    assert!(plan.status.success(), "{}", stderr(&plan));
    let edl = stdout(&plan);
    assert!(edl.contains("*** Begin EDL"), "{edl}");
    assert!(edl.contains("*** Trim Clip"), "{edl}");
    assert!(edl.contains("clip_uuid=clip-source"), "{edl}");
    assert!(edl.contains("+ start: 1.500"), "{edl}");
    assert!(edl.contains("+ end: 4.000"), "{edl}");

    let edl_path = project_root.join("planned-transcript.edl");
    fs::write(&edl_path, edl).unwrap();
    let apply = run(&["apply-edl", &project_arg, &edl_path.to_string_lossy()]);
    assert!(apply.status.success(), "{}", stderr(&apply));

    fs::remove_dir_all(&parent).ok();
}

#[test]
fn plan_transcript_setup_edl_autonomously_trims_to_advice_segment() {
    let parent = tmp_dir("plan-transcript-setup-parent");
    fs::create_dir_all(&parent).unwrap();
    let source = parent.join("source.mp4");
    make_tone_mp4(&source, "5");

    let parent_arg = parent.to_string_lossy();
    let source_arg = source.to_string_lossy();
    let new_output = run(&[
        "new",
        "setup-project",
        "--at",
        &parent_arg,
        "--import",
        &source_arg,
        "--link",
        "--no-index",
    ]);
    assert!(new_output.status.success(), "{}", stderr(&new_output));

    let project_root = parent.join("setup-project");
    write_setup_transcript(&project_root, "raw/source.mp4");
    let project_arg = project_root.to_string_lossy();
    let plan = run(&["plan-transcript-setup-edl", &project_arg]);
    assert!(plan.status.success(), "{}", stderr(&plan));
    let edl = stdout(&plan);
    assert!(edl.contains("*** Begin EDL"), "{edl}");
    assert!(edl.contains("*** Trim Clip"), "{edl}");
    assert!(edl.contains("clip_uuid=clip-source"), "{edl}");
    assert!(edl.contains("+ start: 2.500"), "{edl}");
    assert!(edl.contains("+ end: 5.000"), "{edl}");

    let edl_path = project_root.join("planned-setup.edl");
    fs::write(&edl_path, edl).unwrap();
    let apply = run(&["apply-edl", &project_arg, &edl_path.to_string_lossy()]);
    assert!(apply.status.success(), "{}", stderr(&apply));

    fs::remove_dir_all(&parent).ok();
}

#[test]
fn plan_transcript_remove_edl_emits_internal_removal_ranges() {
    let parent = tmp_dir("plan-transcript-remove-parent");
    fs::create_dir_all(&parent).unwrap();
    let source = parent.join("source.mp4");
    make_tone_mp4(&source, "8");

    let parent_arg = parent.to_string_lossy();
    let source_arg = source.to_string_lossy();
    let new_output = run(&[
        "new",
        "remove-project",
        "--at",
        &parent_arg,
        "--import",
        &source_arg,
        "--link",
        "--no-index",
    ]);
    assert!(new_output.status.success(), "{}", stderr(&new_output));

    let project_root = parent.join("remove-project");
    let transcript_path = project_root
        .join("index")
        .join("whisper")
        .join("raw/source.mp4.json");
    fs::create_dir_all(transcript_path.parent().unwrap()).unwrap();
    fs::write(
        &transcript_path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "indexer": "whisper",
            "asset_id": "raw/source.mp4",
            "data": { "segments": [
                {"start_s": 0.0, "end_s": 2.0, "text": "keep this opening point"},
                {"start_s": 2.0, "end_s": 4.0, "text": "awkward mistaken aside should disappear"},
                {"start_s": 4.0, "end_s": 8.0, "text": "keep this final conclusion"}
            ] }
        }))
        .unwrap(),
    )
    .unwrap();

    let project_arg = project_root.to_string_lossy();
    let plan = run(&[
        "plan-transcript-remove-edl",
        &project_arg,
        "--remove-phrase",
        "mistaken aside",
    ]);
    assert!(plan.status.success(), "{}", stderr(&plan));
    let edl = stdout(&plan);
    assert!(edl.contains("*** Trim Clip"), "{edl}");
    assert!(edl.contains("*** Insert Clip"), "{edl}");
    assert!(edl.contains("+ start: 0.000"), "{edl}");
    assert!(edl.contains("+ end: 2.000"), "{edl}");
    assert!(edl.contains("+ start: 4.000"), "{edl}");
    assert!(edl.contains("+ end: 8.000"), "{edl}");

    let edl_path = project_root.join("planned-remove.edl");
    fs::write(&edl_path, edl).unwrap();
    let apply = run(&["apply-edl", &project_arg, &edl_path.to_string_lossy()]);
    assert!(apply.status.success(), "{}", stderr(&apply));

    fs::remove_dir_all(&parent).ok();
}

#[test]
fn plan_transcript_cleanup_edl_autonomously_removes_filler_segment() {
    let parent = tmp_dir("plan-transcript-cleanup-parent");
    fs::create_dir_all(&parent).unwrap();
    let source = parent.join("source.mp4");
    make_tone_mp4(&source, "8");

    let parent_arg = parent.to_string_lossy();
    let source_arg = source.to_string_lossy();
    let new_output = run(&[
        "new",
        "cleanup-project",
        "--at",
        &parent_arg,
        "--import",
        &source_arg,
        "--link",
        "--no-index",
    ]);
    assert!(new_output.status.success(), "{}", stderr(&new_output));

    let project_root = parent.join("cleanup-project");
    let transcript_path = project_root
        .join("index")
        .join("whisper")
        .join("raw/source.mp4.json");
    fs::create_dir_all(transcript_path.parent().unwrap()).unwrap();
    fs::write(
        &transcript_path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "indexer": "whisper",
            "asset_id": "raw/source.mp4",
            "data": { "segments": [
                {"start_s": 0.0, "end_s": 2.0, "text": "keep this opening point"},
                {"start_s": 2.0, "end_s": 4.0, "text": "um uh like um"},
                {"start_s": 4.0, "end_s": 8.0, "text": "keep this final conclusion"}
            ] }
        }))
        .unwrap(),
    )
    .unwrap();

    let project_arg = project_root.to_string_lossy();
    let plan = run(&["plan-transcript-cleanup-edl", &project_arg]);
    assert!(plan.status.success(), "{}", stderr(&plan));
    let edl = stdout(&plan);
    assert!(edl.contains("*** Trim Clip"), "{edl}");
    assert!(edl.contains("*** Insert Clip"), "{edl}");
    assert!(edl.contains("+ start: 0.000"), "{edl}");
    assert!(edl.contains("+ end: 2.000"), "{edl}");
    assert!(edl.contains("+ start: 4.000"), "{edl}");
    assert!(edl.contains("+ end: 8.000"), "{edl}");

    let edl_path = project_root.join("planned-cleanup.edl");
    fs::write(&edl_path, edl).unwrap();
    let apply = run(&["apply-edl", &project_arg, &edl_path.to_string_lossy()]);
    assert!(apply.status.success(), "{}", stderr(&apply));

    fs::remove_dir_all(&parent).ok();
}

#[test]
fn plan_false_start_edl_removes_restart_marker_preceding_fragment() {
    let parent = tmp_dir("plan-false-start-parent");
    fs::create_dir_all(&parent).unwrap();
    let source = parent.join("source.mp4");
    make_tone_mp4(&source, "6");

    let parent_arg = parent.to_string_lossy();
    let source_arg = source.to_string_lossy();
    let new_output = run(&[
        "new",
        "false-start-project",
        "--at",
        &parent_arg,
        "--import",
        &source_arg,
        "--link",
        "--no-index",
    ]);
    assert!(new_output.status.success(), "{}", stderr(&new_output));

    let project_root = parent.join("false-start-project");
    write_false_start_words(&project_root, "raw/source.mp4");
    let project_arg = project_root.to_string_lossy();
    let plan = run(&["plan-false-start-edl", &project_arg]);
    assert!(plan.status.success(), "{}", stderr(&plan));
    let edl = stdout(&plan);
    assert!(edl.contains("*** Trim Clip"), "{edl}");
    assert!(edl.contains("clip_uuid=clip-source"), "{edl}");
    assert!(edl.contains("+ start: 0.700"), "{edl}");
    assert!(edl.contains("+ end: 6.000"), "{edl}");

    let edl_path = project_root.join("planned-false-start.edl");
    fs::write(&edl_path, edl).unwrap();
    let apply = run(&["apply-edl", &project_arg, &edl_path.to_string_lossy()]);
    assert!(apply.status.success(), "{}", stderr(&apply));

    fs::remove_dir_all(&parent).ok();
}
