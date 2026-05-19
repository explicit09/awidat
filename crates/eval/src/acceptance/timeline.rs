use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use awidat_core::edl::{AnchorContext, apply as apply_edl_envelope, parse as parse_edl};
use awidat_proto::otio::{Clip, MediaReference, Stack, StackChild, Timeline, Track, TrackChild};
use awidat_proto::project::Project;
use serde::Serialize;

use crate::fixtures::clip_count;

use super::case::AcceptanceCase;

pub(crate) fn apply_final_edl(
    project_root: &Path,
    artifacts_root: &Path,
    case: &AcceptanceCase,
) -> Result<AcceptanceEditManifest> {
    apply_final_edl_with_driver(
        project_root,
        artifacts_root,
        case,
        EdlApplyDriver::from_env(),
    )
}

pub(crate) fn apply_final_edl_with_driver(
    project_root: &Path,
    artifacts_root: &Path,
    case: &AcceptanceCase,
    driver: EdlApplyDriver,
) -> Result<AcceptanceEditManifest> {
    let final_edl = resolve_final_edl(project_root, artifacts_root, case)?;
    for snippet in &case.expect.edl_must_contain {
        if !final_edl.text.contains(snippet) {
            bail!("final_edl missing required snippet {snippet:?}");
        }
    }
    fs::create_dir_all(artifacts_root)?;
    let final_edl_path = artifacts_root.join("final_edl.edl");
    fs::write(&final_edl_path, &final_edl.text)
        .with_context(|| format!("write {}", final_edl_path.display()))?;

    match driver {
        EdlApplyDriver::Internal => {
            apply_final_edl_in_process(project_root, case, final_edl, Some(final_edl_path))
        }
        EdlApplyDriver::ExternalCli(cli_path) => apply_final_edl_with_external_cli(
            project_root,
            case,
            final_edl,
            &final_edl_path,
            &cli_path,
        ),
    }
}

fn resolve_final_edl(
    project_root: &Path,
    artifacts_root: &Path,
    case: &AcceptanceCase,
) -> Result<ResolvedFinalEdl> {
    if let Some(final_edl) = &case.final_edl {
        return Ok(ResolvedFinalEdl {
            text: final_edl.clone(),
            source: "fixture_final_edl",
            generator_command: None,
            generator_stdout_path: None,
            generator_stderr_path: None,
        });
    }
    let Some(generator) = &case.edl_generator else {
        bail!("acceptance case must specify final_edl or edl_generator");
    };
    let stdout_path = artifacts_root.join("edl_generator_stdout.txt");
    let stderr_path = artifacts_root.join("edl_generator_stderr.txt");
    let output = Command::new(&generator.command[0])
        .args(&generator.command[1..])
        .env("AWIDAT_ACCEPTANCE_PROJECT_ROOT", project_root)
        .env("AWIDAT_ACCEPTANCE_OBJECTIVE", &case.objective)
        .env("AWIDAT_ACCEPTANCE_SOURCE_ASSET", &case.project.source_asset)
        .env(
            "AWIDAT_ACCEPTANCE_SOURCE_DURATION_S",
            case.project.source_duration_s.to_string(),
        )
        .output()
        .with_context(|| format!("spawn EDL generator {:?}", generator.command))?;
    fs::write(&stdout_path, &output.stdout)
        .with_context(|| format!("write {}", stdout_path.display()))?;
    fs::write(&stderr_path, &output.stderr)
        .with_context(|| format!("write {}", stderr_path.display()))?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    if !output.status.success() {
        bail!(
            "EDL generator failed with status {:?}\nstdout:\n{}\nstderr:\n{}",
            output.status.code(),
            stdout,
            stderr
        );
    }
    if stdout.trim().is_empty() {
        bail!("EDL generator produced empty stdout");
    }
    Ok(ResolvedFinalEdl {
        text: stdout,
        source: "external_edl_command",
        generator_command: Some(generator.command.clone()),
        generator_stdout_path: Some(stdout_path),
        generator_stderr_path: Some(stderr_path),
    })
}

fn apply_final_edl_in_process(
    project_root: &Path,
    case: &AcceptanceCase,
    final_edl: ResolvedFinalEdl,
    final_edl_path: Option<PathBuf>,
) -> Result<AcceptanceEditManifest> {
    let envelope = parse_edl(&final_edl.text).context("parse acceptance final_edl")?;
    let mut project = Project::read(project_root)?;
    let ctx = AnchorContext::with_project_root(project_root);
    let (timeline, outcome) =
        apply_edl_envelope(&project.timeline, &envelope, &ctx).context("apply final_edl")?;
    project.timeline = timeline;
    project.write(project_root)?;
    let applied_descriptions = outcome
        .applied
        .iter()
        .map(|applied| applied.description.clone())
        .collect();
    build_edit_manifest(EditManifestInput {
        project_root,
        case,
        edit_driver: "internal_edl_apply",
        final_edl,
        final_edl_path,
        apply_stdout: None,
        applied_descriptions,
    })
}

fn apply_final_edl_with_external_cli(
    project_root: &Path,
    case: &AcceptanceCase,
    final_edl: ResolvedFinalEdl,
    final_edl_path: &Path,
    cli_path: &Path,
) -> Result<AcceptanceEditManifest> {
    let output = Command::new(cli_path)
        .arg("apply-edl")
        .arg(project_root)
        .arg(final_edl_path)
        .output()
        .with_context(|| format!("spawn external apply-edl driver {}", cli_path.display()))?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    if !output.status.success() {
        bail!(
            "external apply-edl driver failed with status {:?}\nstdout:\n{}\nstderr:\n{}",
            output.status.code(),
            stdout,
            stderr
        );
    }
    build_edit_manifest(EditManifestInput {
        project_root,
        case,
        edit_driver: "external_cli_apply_edl",
        final_edl,
        final_edl_path: Some(final_edl_path.to_path_buf()),
        apply_stdout: Some(stdout.clone()),
        applied_descriptions: applied_descriptions_from_cli_stdout(&stdout),
    })
}

fn applied_descriptions_from_cli_stdout(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .filter_map(|line| line.strip_prefix("  - ").map(str::to_string))
        .collect()
}

fn build_edit_manifest(input: EditManifestInput<'_>) -> Result<AcceptanceEditManifest> {
    let project = Project::read(input.project_root)
        .with_context(|| format!("read edited project at {}", input.project_root.display()))?;
    let final_clip_count = clip_count(&project.timeline);
    let final_kept_source_ranges =
        collect_final_kept_source_ranges(&project.timeline, &input.case.project.source_asset);
    Ok(AcceptanceEditManifest {
        scenario_id: input.case.id.clone(),
        objective: input.case.objective.clone(),
        final_edl_source: input.final_edl.source.into(),
        generator_command: input.final_edl.generator_command,
        generator_stdout_path: input
            .final_edl
            .generator_stdout_path
            .map(|path| path.to_string_lossy().into_owned()),
        generator_stderr_path: input
            .final_edl
            .generator_stderr_path
            .map(|path| path.to_string_lossy().into_owned()),
        edit_driver: input.edit_driver.into(),
        final_edl_path: input
            .final_edl_path
            .map(|path| path.to_string_lossy().into_owned()),
        apply_stdout: input.apply_stdout,
        final_edl: input.final_edl.text,
        applied_descriptions: input.applied_descriptions,
        expected_min_applied_ops: 1,
        final_clip_count,
        final_kept_source_ranges,
    })
}

fn collect_final_kept_source_ranges(
    timeline: &Timeline,
    source_asset: &str,
) -> Vec<FinalKeptSourceRange> {
    let mut ranges = Vec::new();
    collect_stack_kept_ranges(&timeline.tracks, 0.0, source_asset, &mut ranges);
    ranges
}

fn collect_stack_kept_ranges(
    stack: &Stack,
    timeline_start_s: f64,
    source_asset: &str,
    ranges: &mut Vec<FinalKeptSourceRange>,
) -> f64 {
    let mut max_duration_s = 0.0_f64;
    for child in &stack.children {
        let duration_s = match child {
            StackChild::Track(track) => {
                collect_track_kept_ranges(track, timeline_start_s, source_asset, ranges)
            }
            StackChild::Stack(stack) => {
                collect_stack_kept_ranges(stack, timeline_start_s, source_asset, ranges)
            }
            StackChild::Clip(clip) => {
                collect_clip_kept_range(clip, timeline_start_s, source_asset, ranges);
                clip_duration_s(clip)
            }
            StackChild::Gap(gap) => gap.source_range.duration.to_seconds(),
        };
        max_duration_s = max_duration_s.max(duration_s);
    }
    max_duration_s
}

fn collect_track_kept_ranges(
    track: &Track,
    timeline_start_s: f64,
    source_asset: &str,
    ranges: &mut Vec<FinalKeptSourceRange>,
) -> f64 {
    let mut cursor_s = timeline_start_s;
    for child in &track.children {
        match child {
            TrackChild::Clip(clip) => {
                collect_clip_kept_range(clip, cursor_s, source_asset, ranges);
                cursor_s += clip_duration_s(clip);
            }
            TrackChild::Gap(gap) => {
                cursor_s += gap.source_range.duration.to_seconds();
            }
            TrackChild::Transition(_) => {}
            TrackChild::Stack(stack) => {
                cursor_s += collect_stack_kept_ranges(stack, cursor_s, source_asset, ranges);
            }
        }
    }
    cursor_s - timeline_start_s
}

fn collect_clip_kept_range(
    clip: &Clip,
    timeline_start_s: f64,
    source_asset: &str,
    ranges: &mut Vec<FinalKeptSourceRange>,
) {
    if !clip.active || clip_asset(clip) != Some(source_asset) {
        return;
    }
    let Some(source_range) = &clip.source_range else {
        return;
    };
    let duration_s = source_range.duration.to_seconds();
    if duration_s <= 0.0 {
        return;
    }
    let source_start_s = source_range.start_time.to_seconds();
    ranges.push(FinalKeptSourceRange {
        clip_name: clip.name.clone(),
        asset: source_asset.into(),
        timeline_start_s,
        timeline_end_s: timeline_start_s + duration_s,
        source_start_s,
        source_end_s: source_start_s + duration_s,
    });
}

fn clip_asset(clip: &Clip) -> Option<&str> {
    match &clip.media_reference {
        MediaReference::External(reference) => Some(reference.target_url.as_str()),
        MediaReference::Missing(_) => None,
    }
}

fn clip_duration_s(clip: &Clip) -> f64 {
    clip.source_range
        .as_ref()
        .map(|range| range.duration.to_seconds())
        .unwrap_or(0.0)
}

struct ResolvedFinalEdl {
    text: String,
    source: &'static str,
    generator_command: Option<Vec<String>>,
    generator_stdout_path: Option<PathBuf>,
    generator_stderr_path: Option<PathBuf>,
}

#[derive(Debug)]
pub(crate) enum EdlApplyDriver {
    Internal,
    ExternalCli(PathBuf),
}

impl EdlApplyDriver {
    fn from_env() -> Self {
        if let Ok(cli) = std::env::var("AWIDAT_ACCEPTANCE_CLI")
            && !cli.trim().is_empty()
        {
            return Self::ExternalCli(PathBuf::from(cli));
        }
        Self::Internal
    }
}

struct EditManifestInput<'a> {
    project_root: &'a Path,
    case: &'a AcceptanceCase,
    edit_driver: &'a str,
    final_edl: ResolvedFinalEdl,
    final_edl_path: Option<PathBuf>,
    apply_stdout: Option<String>,
    applied_descriptions: Vec<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct AcceptanceEditManifest {
    pub(crate) scenario_id: String,
    pub(crate) objective: String,
    pub(crate) final_edl_source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) generator_command: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) generator_stdout_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) generator_stderr_path: Option<String>,
    pub(crate) edit_driver: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) final_edl_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) apply_stdout: Option<String>,
    pub(crate) final_edl: String,
    pub(crate) applied_descriptions: Vec<String>,
    pub(crate) expected_min_applied_ops: usize,
    pub(crate) final_clip_count: usize,
    pub(crate) final_kept_source_ranges: Vec<FinalKeptSourceRange>,
}
#[derive(Debug, Clone, Serialize)]
pub(crate) struct FinalKeptSourceRange {
    pub(crate) clip_name: String,
    pub(crate) asset: String,
    pub(crate) timeline_start_s: f64,
    pub(crate) timeline_end_s: f64,
    pub(crate) source_start_s: f64,
    pub(crate) source_end_s: f64,
}
