use std::fs;

use super::fixture_manifest::build_fixture_manifest;

#[test]
fn fixture_manifest_summarizes_real_acceptance_directory() -> Result<(), Box<dyn std::error::Error>>
{
    let dir = tempfile::tempdir()?;
    fs::write(
        dir.path().join("generated.json"),
        r#"{
          "id": "acceptance::generated_real",
          "objective": "generated cleanup fixture",
          "project": {
            "name": "generated-real",
            "source_asset": "raw/source.mp4",
            "source_duration_s": 8.0,
            "source_path": "/tmp/generated.mp4",
            "source_start_s": 0.0
          },
          "edl_generator": {
            "command": ["/bin/sh", "-c", "printf '%s' edl"]
          },
          "expect": {
            "duration_min_s": 4.0,
            "duration_max_s": 8.0,
            "max_long_silence_s": 0.8,
            "silence_threshold_db": -40.0,
            "max_black_segment_s": 0.2,
            "black_min_duration_s": 0.2,
            "min_clip_count": 2,
            "removed_source_ranges": [
              {"start_s": 1.0, "end_s": 2.0}
            ],
            "kept_source_ranges": [
              {"start_s": 3.0, "end_s": 5.0}
            ],
            "transcript": {
              "segments": [
                {"start_s": 0.0, "end_s": 1.0, "text": "keep this"}
              ],
              "must_preserve": ["keep"],
              "must_remove": ["remove"]
            }
          }
        }"#,
    )?;
    fs::write(
        dir.path().join("literal.json"),
        r#"{
          "id": "acceptance::literal_real",
          "objective": "literal EDL fixture",
          "project": {
            "name": "literal-real",
            "source_asset": "raw/source.mp4",
            "source_duration_s": 5.0,
            "source_path": "/tmp/literal.mp4",
            "source_start_s": 0.0
          },
          "final_edl": "*** Begin EDL\n*** Trim Clip\n@@ anchor: clip_uuid=source\n+ end: 5.0\n*** End EDL\n",
          "expect": {
            "duration_min_s": 4.0,
            "duration_max_s": 5.5,
            "max_long_silence_s": 0.8,
            "silence_threshold_db": -40.0,
            "max_black_segment_s": 0.2,
            "black_min_duration_s": 0.2,
            "min_clip_count": 1
          }
        }"#,
    )?;
    fs::write(dir.path().join("scenario.template.json"), "{not runnable}")?;

    let manifest = build_fixture_manifest(dir.path())?;

    assert_eq!(manifest.fixture_count, 2);
    assert_eq!(manifest.generated_edl_count, 1);
    assert_eq!(manifest.literal_edl_count, 1);
    assert_eq!(manifest.transcript_check_count, 1);
    assert_eq!(manifest.source_range_check_count, 1);
    assert_eq!(manifest.fixtures[0].id, "acceptance::generated_real");
    assert_eq!(manifest.fixtures[0].edl_source, "generator");
    assert_eq!(manifest.fixtures[0].removed_source_range_count, 1);
    assert_eq!(manifest.fixtures[0].must_preserve_count, 1);
    assert_eq!(manifest.fixtures[0].must_remove_count, 1);
    assert_eq!(manifest.fixtures[1].id, "acceptance::literal_real");
    assert_eq!(manifest.fixtures[1].edl_source, "literal");
    Ok(())
}
