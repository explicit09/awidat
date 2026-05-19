use std::fs;

use crate::fixtures::{ClipSpec, write_project_with_clips};

use super::case::{AcceptanceCase, AcceptanceEdlGenerator, AcceptanceExpect, AcceptanceProject};
use super::timeline::{EdlApplyDriver, apply_final_edl_with_driver};

#[test]
fn external_apply_edl_driver_records_public_cli_surface() {
    let dir = tempfile::tempdir().unwrap();
    let project_root = dir.path().join("project");
    let artifacts_root = dir.path().join("artifacts");
    fs::create_dir_all(&artifacts_root).unwrap();
    write_project_with_clips(
        &project_root,
        "external-apply-driver",
        &[ClipSpec::video(
            "source",
            "source",
            "raw/source.mp4",
            0.0,
            5.0,
        )],
    )
    .unwrap();
    let fake_cli = dir.path().join("fake-awidat");
    fs::write(
        &fake_cli,
        "#!/bin/sh\n\
         test \"$1\" = \"apply-edl\" || exit 2\n\
         test -d \"$2\" || exit 3\n\
         test -f \"$3\" || exit 4\n\
         echo 'Applied 1 EDL op(s):'\n\
         echo '  - fake public apply'\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&fake_cli).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&fake_cli, permissions).unwrap();
    }
    let case = AcceptanceCase {
        id: "acceptance::external_apply_driver".into(),
        objective: "record public apply-edl execution".into(),
        scenario_type: None,
        project: AcceptanceProject {
            name: "external-apply-driver".into(),
            source_asset: "raw/source.mp4".into(),
            source_duration_s: 5.0,
            source_path: Some("/tmp/source.mp4".into()),
            source_start_s: Some(0.0),
        },
        synthetic_media: None,
        final_edl: Some(
            "*** Begin EDL\n*** Trim Clip\n@@ anchor: clip_uuid=source\n+ end: 4.0\n*** End EDL\n"
                .into(),
        ),
        edl_generator: None,
        expect: AcceptanceExpect {
            duration_min_s: 3.5,
            duration_max_s: 4.5,
            max_long_silence_s: 0.8,
            silence_threshold_db: -40.0,
            max_black_segment_s: 0.2,
            black_min_duration_s: 0.2,
            min_clip_count: 1,
            edl_must_contain: vec!["*** Trim Clip".into()],
            removed_source_ranges: Vec::new(),
            kept_source_ranges: Vec::new(),
            transcript: None,
        },
    };

    let edit = apply_final_edl_with_driver(
        &project_root,
        &artifacts_root,
        &case,
        EdlApplyDriver::ExternalCli(fake_cli),
    )
    .unwrap();

    assert_eq!(edit.edit_driver, "external_cli_apply_edl");
    assert_eq!(edit.applied_descriptions, vec!["fake public apply"]);
    assert!(
        edit.final_edl_path
            .as_deref()
            .is_some_and(|path| path.ends_with("final_edl.edl"))
    );
    assert!(
        edit.apply_stdout
            .as_deref()
            .is_some_and(|stdout| stdout.contains("Applied 1 EDL op"))
    );
    assert!(artifacts_root.join("final_edl.edl").is_file());
}

#[test]
fn command_generated_final_edl_feeds_apply_driver_and_manifest() {
    let dir = tempfile::tempdir().unwrap();
    let project_root = dir.path().join("project");
    let artifacts_root = dir.path().join("artifacts");
    fs::create_dir_all(&artifacts_root).unwrap();
    write_project_with_clips(
        &project_root,
        "generated-edl",
        &[ClipSpec::video(
            "source",
            "source",
            "raw/source.mp4",
            0.0,
            5.0,
        )],
    )
    .unwrap();
    let case = AcceptanceCase {
        id: "acceptance::generated_edl".into(),
        objective: "obtain the final EDL from a command driver".into(),
        scenario_type: None,
        project: AcceptanceProject {
            name: "generated-edl".into(),
            source_asset: "raw/source.mp4".into(),
            source_duration_s: 5.0,
            source_path: Some("/tmp/source.mp4".into()),
            source_start_s: Some(0.0),
        },
        synthetic_media: None,
        final_edl: None,
        edl_generator: Some(AcceptanceEdlGenerator {
            command: vec![
                "/bin/sh".into(),
                "-c".into(),
                "printf '%s' '*** Begin EDL\n*** Trim Clip\n@@ anchor: clip_uuid=source\n+ end: 3.5\n*** End EDL\n'"
                    .into(),
            ],
        }),
        expect: AcceptanceExpect {
            duration_min_s: 3.0,
            duration_max_s: 4.0,
            max_long_silence_s: 0.8,
            silence_threshold_db: -40.0,
            max_black_segment_s: 0.2,
            black_min_duration_s: 0.2,
            min_clip_count: 1,
            edl_must_contain: vec!["+ end: 3.5".into()],
            removed_source_ranges: Vec::new(),
            kept_source_ranges: Vec::new(),
            transcript: None,
        },
    };

    let edit = apply_final_edl_with_driver(
        &project_root,
        &artifacts_root,
        &case,
        EdlApplyDriver::Internal,
    )
    .unwrap();

    assert_eq!(edit.final_edl_source, "external_edl_command");
    assert!(edit.final_edl.contains("+ end: 3.5"));
    assert!(edit.generator_stdout_path.is_some());
    assert!(edit.generator_stderr_path.is_some());
    assert!(artifacts_root.join("final_edl.edl").is_file());
    assert!(artifacts_root.join("edl_generator_stdout.txt").is_file());
}
