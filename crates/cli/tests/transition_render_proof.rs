//! Render-backed transition proof tests.

#![allow(clippy::unwrap_used)]

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use common::{run, stderr, stdout, tmp_dir};

const OUTPUT_WIDTH: usize = 1080;
const OUTPUT_HEIGHT: usize = 1920;
const PIXELS: usize = OUTPUT_WIDTH * OUTPUT_HEIGHT;
const PROOF_DURATION_S: &str = "0.300";

#[test]
fn vertical_ffmpeg_transition_families_have_manifest_and_pixel_evidence() {
    for case in [
        TransitionProofCase {
            name: "slide-left",
            id: "montage.slide_left",
            family: "slide",
            intent: "screen_direction",
            direction: Some("left"),
            ffmpeg_xfade: "slideleft",
        },
        TransitionProofCase {
            name: "cross-dissolve",
            id: "montage.cross_dissolve",
            family: "dissolve",
            intent: "soft_time_passage",
            direction: None,
            ffmpeg_xfade: "fade",
        },
        TransitionProofCase {
            name: "flash-white",
            id: "montage.flash_white",
            family: "flash",
            intent: "beat_hit",
            direction: None,
            ffmpeg_xfade: "fadewhite",
        },
        TransitionProofCase {
            name: "wipe-left",
            id: "montage.wipe_left",
            family: "wipe",
            intent: "graphic_movement",
            direction: Some("left"),
            ffmpeg_xfade: "wipeleft",
        },
        TransitionProofCase {
            name: "zoom-in",
            id: "montage.zoom_in",
            family: "zoom",
            intent: "punch_in",
            direction: Some("in"),
            ffmpeg_xfade: "zoomin",
        },
    ] {
        assert_transition_case(case);
    }
}

struct TransitionProofCase {
    name: &'static str,
    id: &'static str,
    family: &'static str,
    intent: &'static str,
    direction: Option<&'static str>,
    ffmpeg_xfade: &'static str,
}

fn assert_transition_case(case: TransitionProofCase) {
    let parent = tmp_dir(&format!("transition-render-proof-{}", case.name));
    fs::create_dir_all(&parent).unwrap();

    let outgoing = parent.join("outgoing.mp4");
    let incoming = parent.join("incoming.mp4");
    make_pattern_tone_mp4(&outgoing, "testsrc2=duration=3:size=540x960:rate=30", "440");
    make_pattern_tone_mp4(
        &incoming,
        "smptebars=duration=3:size=540x960:rate=30",
        "660",
    );

    let parent_arg = parent.to_string_lossy();
    let outgoing_arg = outgoing.to_string_lossy();
    let new_output = run(&[
        "new",
        "transition-proof",
        "--at",
        &parent_arg,
        "--import",
        &outgoing_arg,
        "--link",
        "--no-index",
        "--no-md",
    ]);
    assert!(new_output.status.success(), "{}", stderr(&new_output));

    let project_root = parent.join("transition-proof");
    fs::copy(&incoming, project_root.join("raw").join("incoming.mp4")).unwrap();

    let project_arg = project_root.to_string_lossy();
    let shape_edl = project_root.join("shape.edl");
    fs::write(
        &shape_edl,
        "*** Begin EDL\n\
         *** Set Output Format\n\
         + aspect_ratio: 9:16\n\
         + platform: youtube_shorts\n\
         + safe_area: mobile\n\
         *** Trim Clip\n\
         @@ anchor: clip_uuid=clip-outgoing\n\
         + start: 0\n\
         + end: 1.5\n\
         *** Insert Clip\n\
         + asset: raw/incoming.mp4\n\
         + track: V1\n\
         + at_position: 1\n\
         + start: 0.75\n\
         + end: 2.25\n\
         + name: proof-incoming\n\
         *** End EDL\n",
    )
    .unwrap();

    let shape_apply = run(&["apply-edl", &project_arg, &shape_edl.to_string_lossy()]);
    assert!(shape_apply.status.success(), "{}", stderr(&shape_apply));

    let incoming_uuid = clip_uuid_by_name(&project_root, "proof-incoming");
    let transition_edl = project_root.join("transition.edl");
    let direction_line = case
        .direction
        .map(|direction| format!("+ direction: {direction}\n"))
        .unwrap_or_default();
    fs::write(
        &transition_edl,
        format!(
            "*** Begin EDL\n\
             *** Insert Transition\n\
             @@ between: clip_uuid=clip-outgoing and clip_uuid={incoming_uuid}\n\
             + id: {id}\n\
             + kind: {id}\n\
             + family: {family}\n\
             + intent: {intent}\n\
             + energy: 0.640\n\
{direction_line}\
             + duration_s: {PROOF_DURATION_S}\n\
             + alignment: center\n\
             *** End EDL\n",
            id = case.id,
            family = case.family,
            intent = case.intent,
        ),
    )
    .unwrap();

    let transition_apply = run(&["apply-edl", &project_arg, &transition_edl.to_string_lossy()]);
    assert!(
        transition_apply.status.success(),
        "{}",
        stderr(&transition_apply)
    );

    let render = run(&["render", &project_arg]);
    assert!(render.status.success(), "{}", stderr(&render));
    let render_stdout = stdout(&render);
    let output_path = parse_path_after(&render_stdout, "Render complete: ");
    let manifest_path = parse_path_after(&render_stdout, "Render manifest: ");

    let manifest = fs::read_to_string(&manifest_path).unwrap();
    assert!(manifest.contains("scale=1080:1920"), "{manifest}");
    assert!(
        manifest.contains(&format!(
            "xfade=transition={}:duration=0.3",
            case.ffmpeg_xfade
        )),
        "{manifest}"
    );
    assert!(manifest.contains("acrossfade=d=0.3"), "{manifest}");

    assert_output_dimensions(&output_path, OUTPUT_WIDTH, OUTPUT_HEIGHT);

    let before = extract_gray_frame(&output_path, "1.250");
    let mid = extract_gray_frame(&output_path, "1.500");
    let after = extract_gray_frame(&output_path, "1.750");

    let side_difference = mean_abs_diff(&before, &after);
    let mid_from_before = mean_abs_diff(&before, &mid);
    let mid_from_after = mean_abs_diff(&mid, &after);

    assert!(
        side_difference > 20.0,
        "{} proof clips are not visually distinct enough: {side_difference:.2}",
        case.name
    );
    assert!(
        mid_from_before > 8.0,
        "{} mid-transition frame does not differ from outgoing clip: {mid_from_before:.2}",
        case.name
    );
    assert!(
        mid_from_after > 8.0,
        "{} mid-transition frame does not differ from incoming clip: {mid_from_after:.2}",
        case.name
    );

    fs::remove_dir_all(&parent).ok();
}

fn make_pattern_tone_mp4(path: &Path, video_source: &str, frequency: &str) {
    let output = Command::new("ffmpeg")
        .args([
            "-y",
            "-f",
            "lavfi",
            "-i",
            video_source,
            "-f",
            "lavfi",
            "-i",
            &format!("sine=frequency={frequency}:duration=3"),
            "-shortest",
            "-pix_fmt",
            "yuv420p",
        ])
        .arg(path)
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
}

fn clip_uuid_by_name(project_root: &Path, name: &str) -> String {
    let timeline_path = project_root.join("project.otio.json");
    let timeline: serde_json::Value =
        serde_json::from_slice(&fs::read(&timeline_path).unwrap()).unwrap();
    find_clip_uuid_by_name(&timeline, name).unwrap_or_else(|| panic!("missing clip named {name}"))
}

fn find_clip_uuid_by_name(value: &serde_json::Value, name: &str) -> Option<String> {
    if value.get("name").and_then(serde_json::Value::as_str) == Some(name) {
        return value
            .pointer("/metadata/montage/clip_uuid")
            .or_else(|| value.pointer("/metadata/montage/extra/clip_uuid"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
    }

    match value {
        serde_json::Value::Array(items) => items
            .iter()
            .find_map(|item| find_clip_uuid_by_name(item, name)),
        serde_json::Value::Object(map) => map
            .values()
            .find_map(|item| find_clip_uuid_by_name(item, name)),
        _ => None,
    }
}

fn parse_path_after(stdout: &str, prefix: &str) -> PathBuf {
    stdout
        .lines()
        .find_map(|line| line.strip_prefix(prefix))
        .map(PathBuf::from)
        .unwrap_or_else(|| panic!("missing `{prefix}` in output:\n{stdout}"))
}

fn assert_output_dimensions(path: &Path, expected_width: usize, expected_height: usize) {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height",
            "-of",
            "csv=p=0:s=x",
        ])
        .arg(path)
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", stderr(&output));

    let dimensions = stdout(&output);
    assert_eq!(
        dimensions.trim(),
        format!("{expected_width}x{expected_height}")
    );
}

fn extract_gray_frame(path: &Path, t_s: &str) -> Vec<u8> {
    let output = Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-ss",
            t_s,
            "-i",
            &path.to_string_lossy(),
            "-frames:v",
            "1",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "gray",
            "-",
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(output.stdout.len(), PIXELS);
    output.stdout
}

fn mean_abs_diff(left: &[u8], right: &[u8]) -> f64 {
    let sum: u64 = left
        .iter()
        .zip(right)
        .map(|(a, b)| u64::from(a.abs_diff(*b)))
        .sum();
    sum as f64 / left.len() as f64
}
