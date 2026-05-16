//! Golden edit runner.
//!
//! Golden cases are small JSON fixtures that describe a synthetic
//! project, an editorial objective, the EDL an agent/editor should
//! produce, and structural expectations after applying it. They are
//! intentionally media-free so `--golden` can run in CI.

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use awidat_core::edl::{AnchorContext, apply, parse};
use awidat_proto::awidat_meta::{AudioRelation, CutType, cut_boundary_key};
use awidat_proto::project::Project;
use serde::Deserialize;
use std::time::Instant;

use crate::fixtures::{
    ClipSpec, clip_count, clip_range_by_uuid, write_asset, write_project_with_clips,
};
use crate::{Scenario, ScenarioOutcome, ScenarioStatus};

/// Return all checked-in golden edit scenarios.
pub fn defaults() -> Vec<Box<dyn Scenario>> {
    GOLDEN_FIXTURES
        .iter()
        .map(|(scenario_id, fixture_name)| {
            Box::new(GoldenCaseScenario {
                scenario_id,
                fixture_name,
            }) as Box<dyn Scenario>
        })
        .collect()
}

const GOLDEN_FIXTURES: &[(&str, &str)] = &[
    ("golden::trim_dead_air_cut", "trim_dead_air.json"),
    ("golden::hard_cut_default", "hard_cut_default.json"),
    ("golden::cut_on_action", "cut_on_action.json"),
    ("golden::cutaway", "cutaway.json"),
    ("golden::insert", "insert.json"),
    ("golden::eyeline_match_cut", "eyeline_match_cut.json"),
    ("golden::shot_reverse_shot", "shot_reverse_shot.json"),
    ("golden::match_cut", "match_cut.json"),
    ("golden::smash_cut", "smash_cut.json"),
    ("golden::cross_cut", "cross_cut.json"),
    ("golden::j_cut_audio_lead", "j_cut_audio_lead.json"),
    ("golden::l_cut_audio_trail", "l_cut_audio_trail.json"),
    ("golden::thematic_montage", "thematic_montage.json"),
    (
        "golden::podcast_dialogue_j_cut_variant",
        "podcast_dialogue_j_cut_variant.json",
    ),
    (
        "golden::short_form_hook_smash_cut_variant",
        "short_form_hook_smash_cut_variant.json",
    ),
    (
        "golden::tutorial_insert_failure_repair",
        "tutorial_insert_failure_repair.json",
    ),
    (
        "golden::documentary_no_transition_failure",
        "documentary_no_transition_failure.json",
    ),
    (
        "golden::multi_step_dialogue_cleanup",
        "multi_step_dialogue_cleanup.json",
    ),
    (
        "golden::invalid_split_edit_rejected",
        "invalid_split_edit_rejected.json",
    ),
    (
        "golden::invalid_cut_confidence_rejected",
        "invalid_cut_confidence_rejected.json",
    ),
    (
        "golden::transition_overuse_rejection",
        "transition_overuse_rejection.json",
    ),
    (
        "golden::rough_assembly_delivery_constraints",
        "rough_assembly_delivery_constraints.json",
    ),
    (
        "golden::rough_assembly_rejected_followup",
        "rough_assembly_rejected_followup.json",
    ),
];

struct GoldenCaseScenario {
    scenario_id: &'static str,
    fixture_name: &'static str,
}

#[async_trait]
impl Scenario for GoldenCaseScenario {
    fn id(&self) -> &'static str {
        self.scenario_id
    }

    fn description(&self) -> &'static str {
        "Apply a JSON-defined golden EDL and compare the resulting tiny timeline to expected structure."
    }

    async fn run(&self) -> Result<ScenarioOutcome> {
        let started = Instant::now();
        let case = load_case(self.fixture_name)?;
        let result = run_case(&case).map(|msg| ScenarioOutcome {
            id: case.id.clone(),
            status: ScenarioStatus::Pass,
            elapsed: started.elapsed(),
            message: msg,
        });
        Ok(match result {
            Ok(outcome) => outcome,
            Err(err) => ScenarioOutcome {
                id: case.id,
                status: ScenarioStatus::Fail,
                elapsed: started.elapsed(),
                message: format!("{err:#}"),
            },
        })
    }
}

fn load_case(name: &str) -> Result<GoldenCase> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("golden")
        .join(name);
    let bytes = std::fs::read(&path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("parse {}", path.display()))
}

fn run_case(case: &GoldenCase) -> Result<String> {
    case.validate()?;
    assert_proposal_history(case)?;
    let dir = tempfile::tempdir()?;
    let clips: Vec<ClipSpec> = case
        .project
        .clips
        .iter()
        .map(|clip| {
            ClipSpec::video(
                &clip.name,
                &clip.uuid,
                &clip.asset,
                clip.start_s,
                clip.duration_s,
            )
            .with_transcript(&clip.transcript)
        })
        .collect();
    write_project_with_clips(dir.path(), &case.project.name, &clips)?;
    for asset in &case.project.assets {
        write_asset(dir.path(), asset)?;
    }

    for forbidden in &case.expect.forbidden_ops {
        if case.edl.contains(&format!("*** {forbidden}")) {
            bail!("forbidden op {forbidden:?} appeared in EDL");
        }
    }

    let edit_result = (|| {
        let envelope = parse(&case.edl).context("parse golden EDL")?;
        let project = Project::read(dir.path())?;
        let ctx = AnchorContext::with_project_root(dir.path());
        apply(&project.timeline, &envelope, &ctx).context("apply golden EDL")
    })();

    if let Some(expected_error) = &case.expect.error_contains {
        return match edit_result {
            Ok(_) => bail!(
                "expected golden case to fail containing {expected_error:?}, but EDL applied successfully"
            ),
            Err(err) => {
                let message = format!("{err:#}");
                if !message.contains(expected_error) {
                    bail!(
                        "expected golden case failure containing {expected_error:?}, got: {message}"
                    );
                }
                Ok(format!(
                    "{}: rejected expected invalid edit",
                    case.objective
                ))
            }
        };
    }

    let (timeline, outcome) = edit_result?;

    if let Some(expected) = case.expect.clip_count
        && clip_count(&timeline) != expected
    {
        bail!("expected {expected} clips, got {}", clip_count(&timeline));
    }

    let tolerance = case.expect.tolerance_s.unwrap_or(0.01);
    for expected in &case.expect.clip_ranges {
        let Some((start_s, duration_s)) = clip_range_by_uuid(&timeline, &expected.uuid) else {
            bail!("expected clip uuid {:?} not found", expected.uuid);
        };
        if (start_s - expected.start_s).abs() > tolerance {
            bail!(
                "clip {} start_s expected {:.3}, got {:.3}",
                expected.uuid,
                expected.start_s,
                start_s
            );
        }
        if (duration_s - expected.duration_s).abs() > tolerance {
            bail!(
                "clip {} duration_s expected {:.3}, got {:.3}",
                expected.uuid,
                expected.duration_s,
                duration_s
            );
        }
    }

    for required in &case.expect.applied_contains {
        if !outcome
            .applied
            .iter()
            .any(|applied| applied.description.contains(required))
        {
            bail!("applied-op log did not contain {required:?}");
        }
    }

    assert_cut_boundaries(&timeline, &case.expect.cut_boundaries)?;
    assert_output_format(&timeline, case.expect.output_format.as_ref())?;
    assert_package_metadata(&timeline, case.expect.package_metadata.as_ref())?;

    Ok(format!(
        "{}: {} op(s) matched expected structure",
        case.objective,
        outcome.applied.len()
    ))
}

fn assert_cut_boundaries(
    timeline: &awidat_proto::otio::Timeline,
    expected_boundaries: &[ExpectedCutBoundary],
) -> Result<()> {
    if expected_boundaries.is_empty() {
        return Ok(());
    }
    let Some(awidat) = timeline.metadata.awidat.as_ref() else {
        bail!("expected cut boundary metadata, but timeline has no awidat metadata");
    };
    for expected in expected_boundaries {
        let key = cut_boundary_key(&expected.from, &expected.to);
        let Some(spec) = awidat.cut_boundaries.get(&key) else {
            bail!("expected cut boundary {key:?} not found");
        };
        if spec.cut_type != expected.cut_type {
            bail!(
                "cut boundary {key} cut_type expected {:?}, got {:?}",
                expected.cut_type,
                spec.cut_type
            );
        }
        if spec.intent != expected.intent {
            bail!(
                "cut boundary {key} intent expected {:?}, got {:?}",
                expected.intent,
                spec.intent
            );
        }
        if let Some(audio_relation) = expected.audio_relation
            && spec.audio_relation != audio_relation
        {
            bail!(
                "cut boundary {key} audio_relation expected {:?}, got {:?}",
                audio_relation,
                spec.audio_relation
            );
        }
        if let Some(reason) = &expected.reason_contains
            && !spec
                .reason
                .as_deref()
                .is_some_and(|actual| actual.contains(reason))
        {
            bail!(
                "cut boundary {key} reason did not contain {:?}: {:?}",
                reason,
                spec.reason
            );
        }
    }
    Ok(())
}

fn assert_output_format(
    timeline: &awidat_proto::otio::Timeline,
    expected: Option<&ExpectedOutputFormat>,
) -> Result<()> {
    let Some(expected) = expected else {
        return Ok(());
    };
    let Some(awidat) = timeline.metadata.awidat.as_ref() else {
        bail!("expected output_format metadata, but timeline has no awidat metadata");
    };
    let Some(actual) = awidat.extra.get("output_format") else {
        bail!("expected output_format metadata, but none was found");
    };
    assert_json_string(actual, "aspect_ratio", Some(&expected.aspect_ratio))?;
    assert_json_string(actual, "platform", expected.platform.as_deref())?;
    assert_json_string(actual, "safe_area", expected.safe_area.as_deref())?;
    Ok(())
}

fn assert_json_string(
    actual: &serde_json::Value,
    field: &str,
    expected: Option<&str>,
) -> Result<()> {
    match expected {
        Some(expected) => {
            let got = actual.get(field).and_then(|value| value.as_str());
            if got != Some(expected) {
                bail!("output_format field {field:?} expected {expected:?}, got {got:?}");
            }
        }
        None => {
            if actual.get(field).is_some_and(|value| !value.is_null()) {
                bail!(
                    "output_format field {field:?} expected null or absent, got {:?}",
                    actual.get(field)
                );
            }
        }
    }
    Ok(())
}

fn assert_package_metadata(
    timeline: &awidat_proto::otio::Timeline,
    expected: Option<&ExpectedPackageMetadata>,
) -> Result<()> {
    let Some(expected) = expected else {
        return Ok(());
    };
    let Some(awidat) = timeline.metadata.awidat.as_ref() else {
        bail!("expected package_metadata, but timeline has no awidat metadata");
    };
    let Some(actual) = awidat.extra.get("package_metadata") else {
        bail!("expected package_metadata, but none was found");
    };
    if let Some(platform) = &expected.platform {
        assert_timeline_extra_string(actual, "package_metadata", "platform", platform)?;
    }
    if let Some(title) = &expected.title {
        assert_timeline_extra_string(actual, "package_metadata", "title", title)?;
    }
    if let Some(description) = &expected.description {
        assert_timeline_extra_string(actual, "package_metadata", "description", description)?;
    }
    if let Some(needle) = &expected.description_contains {
        let got = actual.get("description").and_then(|value| value.as_str());
        if !got.is_some_and(|text| text.contains(needle)) {
            bail!("package_metadata description expected to contain {needle:?}, got {got:?}");
        }
    }
    if let Some(expected_tags) = &expected.tags_contains {
        let got = actual.get("tags").and_then(|value| value.as_str());
        for expected_tag in expected_tags {
            if !got.is_some_and(|tags| tags.split(',').any(|tag| tag.trim() == expected_tag)) {
                bail!("package_metadata tags expected to contain {expected_tag:?}, got {got:?}");
            }
        }
    }
    Ok(())
}

fn assert_timeline_extra_string(
    actual: &serde_json::Value,
    namespace: &str,
    field: &str,
    expected: &str,
) -> Result<()> {
    let got = actual.get(field).and_then(|value| value.as_str());
    if got != Some(expected) {
        bail!("{namespace} field {field:?} expected {expected:?}, got {got:?}");
    }
    Ok(())
}

fn assert_proposal_history(case: &GoldenCase) -> Result<()> {
    for entry in &case.expect.proposal_history {
        if entry.id.trim().is_empty() {
            bail!("proposal_history entry has empty id");
        }
        if entry.status != "accepted" && entry.status != "rejected" {
            bail!(
                "proposal_history entry {} has unsupported status {:?}",
                entry.id,
                entry.status
            );
        }
        if entry.recommendation.trim().is_empty() {
            bail!(
                "proposal_history entry {} has empty recommendation",
                entry.id
            );
        }
        if let Some(reason) = &entry.reason_contains
            && reason.trim().is_empty()
        {
            bail!(
                "proposal_history entry {} has empty reason_contains",
                entry.id
            );
        }
        for snippet in &entry.edl_contains {
            if snippet.trim().is_empty() {
                bail!(
                    "proposal_history entry {} has empty edl_contains snippet",
                    entry.id
                );
            }
        }
        for snippet in &entry.final_edl_must_contain {
            if !case.edl.contains(snippet) {
                bail!(
                    "proposal_history entry {} expected final EDL to contain {:?}",
                    entry.id,
                    snippet
                );
            }
        }
        for snippet in &entry.final_edl_must_not_contain {
            if case.edl.contains(snippet) {
                bail!(
                    "proposal_history entry {} expected final EDL not to contain {:?}",
                    entry.id,
                    snippet
                );
            }
        }
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
struct GoldenCase {
    id: String,
    objective: String,
    project: GoldenProject,
    edl: String,
    expect: GoldenExpect,
}

impl GoldenCase {
    fn validate(&self) -> Result<()> {
        if self.id.trim().is_empty() {
            bail!("golden case id is empty");
        }
        if self.objective.trim().is_empty() {
            bail!("golden case objective is empty");
        }
        if self.project.clips.is_empty() {
            bail!("golden case has no input clips");
        }
        if self.edl.trim().is_empty() {
            bail!("golden case EDL is empty");
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
struct GoldenProject {
    name: String,
    #[serde(default)]
    assets: Vec<String>,
    clips: Vec<GoldenClip>,
}

#[derive(Debug, Deserialize)]
struct GoldenClip {
    name: String,
    uuid: String,
    asset: String,
    start_s: f64,
    duration_s: f64,
    transcript: String,
}

#[derive(Debug, Deserialize)]
struct GoldenExpect {
    #[serde(default)]
    tolerance_s: Option<f64>,
    #[serde(default)]
    error_contains: Option<String>,
    #[serde(default)]
    clip_count: Option<usize>,
    #[serde(default)]
    clip_ranges: Vec<ExpectedClipRange>,
    #[serde(default)]
    applied_contains: Vec<String>,
    #[serde(default)]
    forbidden_ops: Vec<String>,
    #[serde(default)]
    cut_boundaries: Vec<ExpectedCutBoundary>,
    #[serde(default)]
    output_format: Option<ExpectedOutputFormat>,
    #[serde(default)]
    package_metadata: Option<ExpectedPackageMetadata>,
    #[serde(default)]
    proposal_history: Vec<ExpectedProposalHistory>,
}

#[derive(Debug, Deserialize)]
struct ExpectedClipRange {
    uuid: String,
    start_s: f64,
    duration_s: f64,
}

#[derive(Debug, Deserialize)]
struct ExpectedCutBoundary {
    from: String,
    to: String,
    cut_type: CutType,
    intent: String,
    #[serde(default)]
    audio_relation: Option<AudioRelation>,
    #[serde(default)]
    reason_contains: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ExpectedOutputFormat {
    aspect_ratio: String,
    #[serde(default)]
    platform: Option<String>,
    #[serde(default)]
    safe_area: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ExpectedPackageMetadata {
    #[serde(default)]
    platform: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    description_contains: Option<String>,
    #[serde(default)]
    tags_contains: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct ExpectedProposalHistory {
    id: String,
    status: String,
    recommendation: String,
    #[serde(default)]
    reason_contains: Option<String>,
    #[serde(default)]
    edl_contains: Vec<String>,
    #[serde(default)]
    final_edl_must_contain: Vec<String>,
    #[serde(default)]
    final_edl_must_not_contain: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checked_in_golden_case_runs() {
        let case = load_case("trim_dead_air.json").unwrap();
        run_case(&case).unwrap();
    }

    #[test]
    fn semantic_cut_golden_case_declares_boundary_expectation() {
        let case = load_case("eyeline_match_cut.json").unwrap();
        assert_eq!(case.expect.cut_boundaries.len(), 1);
        run_case(&case).unwrap();
    }

    #[test]
    fn all_default_golden_cases_run() {
        for (_scenario_id, fixture_name) in GOLDEN_FIXTURES {
            let case = load_case(fixture_name).unwrap();
            run_case(&case).unwrap();
        }
    }

    #[test]
    fn rough_assembly_golden_case_asserts_delivery_constraints() {
        let case = load_case("rough_assembly_delivery_constraints.json").unwrap();
        let output_format = case.expect.output_format.as_ref().unwrap();
        assert_eq!(output_format.aspect_ratio, "9:16");
        assert_eq!(output_format.platform.as_deref(), Some("youtube_shorts"));
        assert_eq!(output_format.safe_area.as_deref(), Some("mobile"));
        let package_metadata = case.expect.package_metadata.as_ref().unwrap();
        assert_eq!(package_metadata.platform.as_deref(), Some("youtube_shorts"));
        assert_eq!(
            package_metadata.title.as_deref(),
            Some("Edit the pause before the answer")
        );
        run_case(&case).unwrap();
    }

    #[test]
    fn rough_assembly_golden_case_records_rejected_followup_history() {
        let case = load_case("rough_assembly_rejected_followup.json").unwrap();
        assert_eq!(case.expect.proposal_history.len(), 2);
        assert_eq!(case.expect.proposal_history[0].status, "rejected");
        assert_eq!(case.expect.proposal_history[1].status, "accepted");
        run_case(&case).unwrap();
    }
}
