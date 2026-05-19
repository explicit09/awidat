use std::fs;

use serde_json::json;

use super::failure_package::package_failure;

#[test]
fn failure_packet_copies_core_artifacts_and_writes_summary()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let run_root = dir.path().join("run");
    let artifacts = run_root.join("artifacts");
    fs::create_dir_all(&artifacts)?;
    let output_path = run_root
        .join("project")
        .join("renders")
        .join("timeline.mp4");
    fs::create_dir_all(output_path.parent().unwrap())?;
    fs::write(&output_path, b"mp4")?;

    for name in [
        "artifact_bundle.json",
        "final_edl.edl",
        "edit_manifest.json",
        "ffprobe.json",
        "silence.json",
        "blackdetect.json",
        "transcript.json",
        "edl_generator_stdout.txt",
        "edl_generator_stderr.txt",
    ] {
        fs::write(artifacts.join(name), format!("{name}\n"))?;
    }
    let scorecard_path = artifacts.join("scorecard.json");
    fs::write(
        &scorecard_path,
        serde_json::to_vec_pretty(&json!({
            "scenario_id": "acceptance::draft_failure",
            "objective": "Remove dead air from a candidate clip.",
            "status": "fail",
            "score": 84.0,
            "run_root": run_root,
            "output_path": output_path,
            "artifacts": {
                "artifact_bundle_json": artifacts.join("artifact_bundle.json"),
                "final_edl": artifacts.join("final_edl.edl"),
                "edit_manifest": artifacts.join("edit_manifest.json"),
                "ffprobe_json": artifacts.join("ffprobe.json"),
                "silence_json": artifacts.join("silence.json"),
                "blackdetect_json": artifacts.join("blackdetect.json"),
                "transcript_json": artifacts.join("transcript.json"),
                "edl_generator_stdout": artifacts.join("edl_generator_stdout.txt"),
                "edl_generator_stderr": artifacts.join("edl_generator_stderr.txt")
            },
            "gates": [
                {
                    "name": "no_long_silence",
                    "severity": "hard",
                    "status": "fail",
                    "message": "long silence remained",
                    "observed": {
                        "ranges": [{"start_s": 3.0, "end_s": 4.2}]
                    }
                }
            ]
        }))?,
    )?;

    let packet_base = dir.path().join("packets");
    let packet = package_failure(&scorecard_path, Some(&packet_base))?;

    assert!(packet.packet_root.join("scorecard.json").is_file());
    assert!(packet.packet_root.join("artifact_bundle.json").is_file());
    assert!(packet.packet_root.join("final_edl.edl").is_file());
    assert!(packet.packet_root.join("edit_manifest.json").is_file());
    assert!(packet.packet_root.join("ffprobe.json").is_file());
    assert!(packet.packet_root.join("silence.json").is_file());
    assert!(packet.packet_root.join("blackdetect.json").is_file());
    assert!(packet.packet_root.join("transcript.json").is_file());
    assert!(
        packet
            .packet_root
            .join("edl_generator_stdout.txt")
            .is_file()
    );
    assert!(
        packet
            .packet_root
            .join("edl_generator_stderr.txt")
            .is_file()
    );
    assert!(
        packet
            .packet_root
            .join("rendered_output_path.txt")
            .is_file()
    );
    let summary = fs::read_to_string(packet.packet_root.join("failure_summary.md"))?;
    assert!(summary.contains("acceptance::draft_failure"));
    assert!(summary.contains("no_long_silence"));
    assert!(summary.contains("3.000-4.200s"));
    Ok(())
}
