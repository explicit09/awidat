//! Output path safety policy tests.

use std::path::{Path, PathBuf};

use montage_render::{OutputPathPolicy, OutputPathSafetyError, validate_render_output_path};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn rel(path: &str) -> PathBuf {
    PathBuf::from(path)
}

#[test]
fn rejects_output_that_matches_an_input_asset() -> TestResult {
    let dir = tempfile::tempdir()?;
    let input = dir.path().join("raw/source.mp4");
    let parent = input
        .parent()
        .ok_or("fixture input should have a parent directory")?;
    std::fs::create_dir_all(parent)?;
    std::fs::write(&input, b"source")?;

    let result = validate_render_output_path(
        dir.path(),
        &input,
        &[rel("raw/source.mp4")],
        &[],
        OutputPathPolicy::default(),
    );

    assert!(matches!(
        result,
        Err(OutputPathSafetyError::SameAsInput { .. })
    ));
    Ok(())
}

#[test]
fn rejects_duplicate_planned_outputs_after_project_relative_resolution() -> TestResult {
    let dir = tempfile::tempdir()?;
    let output = dir.path().join("renders/final.mp4");
    let parent = output
        .parent()
        .ok_or("fixture output should have a parent directory")?;
    std::fs::create_dir_all(parent)?;

    let result = validate_render_output_path(
        dir.path(),
        &output,
        &[],
        &[rel("renders/final.mp4")],
        OutputPathPolicy::default(),
    );

    assert!(matches!(
        result,
        Err(OutputPathSafetyError::DuplicateOutput { .. })
    ));
    Ok(())
}

#[test]
fn rejects_parent_dir_segments_before_filesystem_preflight() -> TestResult {
    let dir = tempfile::tempdir()?;

    let result = validate_render_output_path(
        dir.path(),
        Path::new("renders/../raw/source.mp4"),
        &[],
        &[],
        OutputPathPolicy::default(),
    );

    assert!(matches!(
        result,
        Err(OutputPathSafetyError::UnsafePath { .. })
    ));
    Ok(())
}

#[test]
fn rejects_existing_output_unless_overwrite_is_explicitly_allowed() -> TestResult {
    let dir = tempfile::tempdir()?;
    let output = dir.path().join("renders/final.mp4");
    let parent = output
        .parent()
        .ok_or("fixture output should have a parent directory")?;
    std::fs::create_dir_all(parent)?;
    std::fs::write(&output, b"old")?;

    let result =
        validate_render_output_path(dir.path(), &output, &[], &[], OutputPathPolicy::default());
    assert!(matches!(
        result,
        Err(OutputPathSafetyError::WouldOverwrite { .. })
    ));

    validate_render_output_path(
        dir.path(),
        &output,
        &[],
        &[],
        OutputPathPolicy {
            allow_overwrite: true,
        },
    )?;
    Ok(())
}

#[test]
fn rejects_missing_output_parent_for_explicit_exports() -> TestResult {
    let dir = tempfile::tempdir()?;
    let output = dir.path().join("missing/final.mp4");

    let result =
        validate_render_output_path(dir.path(), &output, &[], &[], OutputPathPolicy::default());

    assert!(matches!(
        result,
        Err(OutputPathSafetyError::ParentUnavailable { .. })
    ));
    Ok(())
}
