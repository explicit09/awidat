//! CLI planner integration tests.

#![allow(clippy::unwrap_used)]

mod common;

use std::fs;
use std::process::Command;

use common::{make_dead_air_mp4, make_tone_mp4, run, stderr, stdout, tmp_dir};
use montage_proto::otio::{StackChild, TrackChild};
use montage_proto::project::Project;

fn make_multi_dead_air_mp4(path: &std::path::Path) {
    let output = Command::new("ffmpeg")
        .args([
            "-y",
            "-f",
            "lavfi",
            "-i",
            "testsrc=duration=6:size=160x90:rate=30",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=44100:duration=1",
            "-f",
            "lavfi",
            "-i",
            "anullsrc=channel_layout=mono:sample_rate=44100:duration=1",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=660:sample_rate=44100:duration=1",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=550:sample_rate=44100:duration=1",
            "-f",
            "lavfi",
            "-i",
            "anullsrc=channel_layout=mono:sample_rate=44100:duration=1",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=770:sample_rate=44100:duration=1",
            "-filter_complex",
            "[1:a][2:a][3:a][4:a][5:a][6:a]concat=n=6:v=0:a=1[a]",
            "-map",
            "0:v",
            "-map",
            "[a]",
            "-t",
            "6",
            "-pix_fmt",
            "yuv420p",
        ])
        .arg(path)
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
}

fn v1_source_starts(project_root: &std::path::Path) -> Vec<f64> {
    let project = Project::read(project_root).unwrap();
    let Some(StackChild::Track(track)) = project.timeline.tracks.children.iter().find(|child| {
        matches!(
            child,
            StackChild::Track(track) if track.name == "V1"
        )
    }) else {
        panic!("missing V1 track");
    };
    track
        .children
        .iter()
        .filter_map(|child| {
            let TrackChild::Clip(clip) = child else {
                return None;
            };
            Some(clip.source_range?.start_time.to_seconds())
        })
        .collect()
}

fn assert_close(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() <= 0.050,
        "expected {actual:.3}s to be within 50ms of {expected:.3}s"
    );
}

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

fn write_dead_air_words(project_root: &std::path::Path, asset: &str) {
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
                {"start_s": 0.2, "end_s": 0.5, "text": "before"},
                {"start_s": 1.4, "end_s": 1.6, "text": "quiet"},
                {"start_s": 2.4, "end_s": 2.7, "text": "after"}
            ]
        }
    });
    fs::write(path, serde_json::to_vec_pretty(&payload).unwrap()).unwrap();
}

fn write_transcript_gap_words(project_root: &std::path::Path, asset: &str) {
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
                {"start_s": 0.2, "end_s": 0.5, "text": "before"},
                {"start_s": 2.4, "end_s": 2.7, "text": "after"}
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
fn plan_dead_air_edl_checks_later_clips_when_first_clip_has_no_silence() {
    let parent = tmp_dir("plan-dead-air-later-clip-parent");
    fs::create_dir_all(&parent).unwrap();
    let source = parent.join("dead-air.mp4");
    make_dead_air_mp4(&source);

    let parent_arg = parent.to_string_lossy();
    let source_arg = source.to_string_lossy();
    let new_output = run(&[
        "new",
        "dead-air-later-clip-project",
        "--at",
        &parent_arg,
        "--import",
        &source_arg,
        "--link",
        "--no-index",
    ]);
    assert!(new_output.status.success(), "{}", stderr(&new_output));

    let project_root = parent.join("dead-air-later-clip-project");
    let project_arg = project_root.to_string_lossy();
    let setup_edl = project_root.join("setup.edl");
    fs::write(
        &setup_edl,
        "*** Begin EDL\n\
*** Trim Clip\n\
@@ anchor: clip_uuid=clip-dead-air\n\
+ start: 0.000\n\
+ end: 0.500\n\
*** Insert Clip\n\
+ asset: raw/dead-air.mp4\n\
+ track: V1\n\
+ at_position: 1\n\
+ start: 1.000\n\
+ end: 3.000\n\
+ name: later-dead-air\n\
*** End EDL\n",
    )
    .unwrap();
    let setup = run(&["apply-edl", &project_arg, &setup_edl.to_string_lossy()]);
    assert!(setup.status.success(), "{}", stderr(&setup));

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
    assert!(edl.contains("*** Trim Clip"), "{edl}");
    assert!(edl.contains("+ start: 2.000"), "{edl}");

    let plan_edl = project_root.join("planned.edl");
    fs::write(&plan_edl, edl).unwrap();
    let apply = run(&["apply-edl", &project_arg, &plan_edl.to_string_lossy()]);
    assert!(apply.status.success(), "{}", stderr(&apply));

    fs::remove_dir_all(&parent).ok();
}

#[test]
fn plan_dead_air_edl_preserves_track_order_when_multiple_clips_insert_ranges() {
    let parent = tmp_dir("plan-dead-air-multi-clip-order-parent");
    fs::create_dir_all(&parent).unwrap();
    let source = parent.join("multi-dead-air.mp4");
    make_multi_dead_air_mp4(&source);

    let parent_arg = parent.to_string_lossy();
    let source_arg = source.to_string_lossy();
    let new_output = run(&[
        "new",
        "dead-air-multi-clip-order-project",
        "--at",
        &parent_arg,
        "--import",
        &source_arg,
        "--link",
        "--no-index",
    ]);
    assert!(new_output.status.success(), "{}", stderr(&new_output));

    let project_root = parent.join("dead-air-multi-clip-order-project");
    let project_arg = project_root.to_string_lossy();
    let setup_edl = project_root.join("setup.edl");
    fs::write(
        &setup_edl,
        "*** Begin EDL\n\
*** Trim Clip\n\
@@ anchor: clip_uuid=clip-multi-dead-air\n\
+ start: 0.000\n\
+ end: 3.000\n\
*** Insert Clip\n\
+ asset: raw/multi-dead-air.mp4\n\
+ track: V1\n\
+ at_position: 1\n\
+ start: 3.000\n\
+ end: 6.000\n\
+ name: later-dead-air\n\
*** End EDL\n",
    )
    .unwrap();
    let setup = run(&["apply-edl", &project_arg, &setup_edl.to_string_lossy()]);
    assert!(setup.status.success(), "{}", stderr(&setup));

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
    assert!(edl.contains("*** Trim Clip"), "{edl}");
    assert_eq!(edl.matches("*** Insert Clip").count(), 2, "{edl}");

    let plan_edl = project_root.join("planned.edl");
    fs::write(&plan_edl, edl).unwrap();
    let apply = run(&["apply-edl", &project_arg, &plan_edl.to_string_lossy()]);
    assert!(apply.status.success(), "{}", stderr(&apply));

    let starts = v1_source_starts(&project_root);
    assert_eq!(starts.len(), 4, "{starts:?}");
    for (actual, expected) in starts.iter().copied().zip([0.0, 2.0, 3.0, 5.0]) {
        assert_close(actual, expected);
    }

    fs::remove_dir_all(&parent).ok();
}

#[test]
fn plan_dead_air_edl_preserves_transcript_words_inside_detected_silence() {
    let parent = tmp_dir("plan-dead-air-transcript-guard-parent");
    fs::create_dir_all(&parent).unwrap();
    let source = parent.join("dead-air.mp4");
    make_dead_air_mp4(&source);

    let parent_arg = parent.to_string_lossy();
    let source_arg = source.to_string_lossy();
    let new_output = run(&[
        "new",
        "dead-air-transcript-guard-project",
        "--at",
        &parent_arg,
        "--import",
        &source_arg,
        "--link",
        "--no-index",
    ]);
    assert!(new_output.status.success(), "{}", stderr(&new_output));

    let project_root = parent.join("dead-air-transcript-guard-project");
    write_dead_air_words(&project_root, "raw/dead-air.mp4");
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
    assert!(edl.contains("+ start: 1.320"), "{edl}");
    assert!(edl.contains("+ end: 1.680"), "{edl}");

    let plan_edl = project_root.join("planned.edl");
    fs::write(&plan_edl, edl).unwrap();
    let apply = run(&["apply-edl", &project_arg, &plan_edl.to_string_lossy()]);
    assert!(apply.status.success(), "{}", stderr(&apply));

    fs::remove_dir_all(&parent).ok();
}

#[test]
fn plan_dead_air_edl_removes_overlong_transcript_gap_when_audio_detection_undershoots() {
    let parent = tmp_dir("plan-dead-air-transcript-gap-parent");
    fs::create_dir_all(&parent).unwrap();
    let source = parent.join("dead-air.mp4");
    make_dead_air_mp4(&source);

    let parent_arg = parent.to_string_lossy();
    let source_arg = source.to_string_lossy();
    let new_output = run(&[
        "new",
        "dead-air-transcript-gap-project",
        "--at",
        &parent_arg,
        "--import",
        &source_arg,
        "--link",
        "--no-index",
    ]);
    assert!(new_output.status.success(), "{}", stderr(&new_output));

    let project_root = parent.join("dead-air-transcript-gap-project");
    write_transcript_gap_words(&project_root, "raw/dead-air.mp4");
    let project_arg = project_root.to_string_lossy();
    let plan = run(&[
        "plan-dead-air-edl",
        &project_arg,
        "--min-duration-s",
        "1.1",
        "--max-transcript-gap-s",
        "1.0",
        "--silence-threshold-db",
        "-40",
    ]);
    assert!(plan.status.success(), "{}", stderr(&plan));
    let edl = stdout(&plan);
    assert!(edl.contains("*** Trim Clip"), "{edl}");
    assert!(edl.contains("+ start: 0.000"), "{edl}");
    assert!(edl.contains("+ end: 0.580"), "{edl}");
    assert!(edl.contains("+ start: 2.320"), "{edl}");
    assert!(edl.contains("+ end: 3.000"), "{edl}");

    let plan_edl = project_root.join("planned.edl");
    fs::write(&plan_edl, edl).unwrap();
    let apply = run(&["apply-edl", &project_arg, &plan_edl.to_string_lossy()]);
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
