//! Professional workflow lens and pre-autonomy orchestration helpers.

use std::collections::{HashMap, HashSet};

use awidat_proto::awidat_meta::AwidatTimelineMetadata;
use awidat_proto::professional::{
    CapabilityArea, CapabilityRegistry, FindingSeverity, LearningSignal, PipelineConflict,
    PipelineReadinessReport, PlannerPassContract, ProfessionalDiagnostic, ReadinessState,
    WorkflowLens,
};
use serde::{Deserialize, Serialize};

/// Thin desktop/backend snapshot for one workflow lens.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowLensSnapshot {
    /// Lens tag.
    pub lens: WorkflowLens,
    /// Current readiness.
    pub readiness: ReadinessState,
    /// Relevant artifact references.
    pub artifacts: Vec<String>,
    /// Review findings.
    pub findings: Vec<ProfessionalDiagnostic>,
    /// Correction actions the UI may surface.
    pub correction_actions: Vec<LensCorrectionAction>,
}

/// Correction actions exposed by thin workflow lenses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LensCorrectionAction {
    /// Open the review package.
    OpenReview,
    /// Generate a fix proposal.
    GenerateProposal,
    /// Run or refresh analysis/indexing.
    RefreshAnalysis,
    /// Start render/export.
    StartRender,
}

/// Pre-autonomy readiness inspection result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrchestrationInspection {
    /// Readiness report after registry and metadata inspection.
    pub readiness: PipelineReadinessReport,
    /// Conflicts collected across planner passes.
    pub conflicts: Vec<PipelineConflict>,
    /// Data-flow edges between planner passes.
    pub planner_edges: Vec<PlannerPassEdge>,
}

/// Data-flow edge between planner passes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannerPassEdge {
    /// Producer pass id.
    pub from_pass_id: String,
    /// Consumer pass id.
    pub to_pass_id: String,
    /// Shared artifact reference.
    pub artifact: String,
}

/// Build the nine workflow lens snapshots without cloning an NLE UI.
pub fn build_workflow_lens_snapshots(
    metadata: &AwidatTimelineMetadata,
) -> Vec<WorkflowLensSnapshot> {
    let readiness = metadata.build_professional_readiness_report();
    all_lenses()
        .into_iter()
        .map(|lens| lens_snapshot(metadata, &readiness, lens))
        .collect()
}

fn lens_snapshot(
    metadata: &AwidatTimelineMetadata,
    readiness: &PipelineReadinessReport,
    lens: WorkflowLens,
) -> WorkflowLensSnapshot {
    let areas = lens_areas(lens);
    let state = areas
        .iter()
        .filter_map(|area| readiness.stage(*area))
        .find(|state| *state == ReadinessState::Blocked)
        .unwrap_or(ReadinessState::Ready);
    let findings = metadata
        .validate_professional_substrate()
        .into_iter()
        .filter(|diagnostic| areas.contains(&diagnostic.area))
        .collect::<Vec<_>>();
    WorkflowLensSnapshot {
        lens,
        readiness: state,
        artifacts: lens_artifacts(metadata, lens),
        correction_actions: lens_actions(lens, state, &findings),
        findings,
    }
}

fn lens_artifacts(metadata: &AwidatTimelineMetadata, lens: WorkflowLens) -> Vec<String> {
    match lens {
        WorkflowLens::Media => metadata
            .asset_catalog
            .as_ref()
            .map(|catalog| {
                catalog
                    .assets
                    .iter()
                    .map(|asset| asset.path.clone())
                    .collect()
            })
            .unwrap_or_default(),
        WorkflowLens::Selects => metadata
            .selects
            .iter()
            .map(|select| select.id.clone())
            .collect(),
        WorkflowLens::Assembly | WorkflowLens::EditReview => metadata
            .proposal_packages
            .iter()
            .map(|package| package.id.clone())
            .collect(),
        WorkflowLens::Vfx => metadata
            .composition_graphs
            .iter()
            .map(|graph| graph.id.clone())
            .chain(
                metadata
                    .tracking_package
                    .as_ref()
                    .into_iter()
                    .flat_map(|package| package.tracks.iter().map(|track| track.id.clone())),
            )
            .collect(),
        WorkflowLens::Color => metadata
            .color_finishing
            .as_ref()
            .map(|state| {
                state
                    .grade_stacks
                    .iter()
                    .map(|stack| stack.id.clone())
                    .collect()
            })
            .unwrap_or_default(),
        WorkflowLens::Audio => metadata
            .audio_finishing
            .as_ref()
            .map(|state| state.buses.iter().map(|bus| bus.id.clone()).collect())
            .unwrap_or_default(),
        WorkflowLens::Delivery => metadata
            .delivery_profiles
            .iter()
            .map(|profile| profile.id.clone())
            .collect(),
        WorkflowLens::Preflight => metadata
            .preflight_reports
            .iter()
            .map(|report| report.id.clone())
            .collect(),
    }
}

fn lens_actions(
    lens: WorkflowLens,
    state: ReadinessState,
    findings: &[ProfessionalDiagnostic],
) -> Vec<LensCorrectionAction> {
    let mut actions = Vec::new();
    if state == ReadinessState::Blocked
        || findings
            .iter()
            .any(|finding| finding.severity == FindingSeverity::Error)
    {
        actions.push(LensCorrectionAction::GenerateProposal);
    }
    match lens {
        WorkflowLens::Media | WorkflowLens::Vfx | WorkflowLens::Color | WorkflowLens::Audio => {
            actions.push(LensCorrectionAction::RefreshAnalysis);
            actions.push(LensCorrectionAction::OpenReview);
        }
        WorkflowLens::Delivery => {
            actions.push(LensCorrectionAction::StartRender);
            actions.push(LensCorrectionAction::OpenReview);
        }
        WorkflowLens::Preflight => actions.push(LensCorrectionAction::GenerateProposal),
        WorkflowLens::Selects | WorkflowLens::Assembly | WorkflowLens::EditReview => {
            actions.push(LensCorrectionAction::OpenReview);
        }
    }
    actions.sort_by_key(|action| *action as u8);
    actions.dedup();
    actions
}

fn lens_areas(lens: WorkflowLens) -> Vec<CapabilityArea> {
    match lens {
        WorkflowLens::Media => vec![CapabilityArea::AssetCatalog],
        WorkflowLens::Selects => vec![CapabilityArea::SourceReviewSelects],
        WorkflowLens::Assembly => vec![CapabilityArea::AssemblyAndTimelineOperations],
        WorkflowLens::EditReview => vec![CapabilityArea::EditorialIntentAndReview],
        WorkflowLens::Vfx => vec![
            CapabilityArea::CompositionGraph,
            CapabilityArea::TrackingMasksMattes,
            CapabilityArea::MotionGraphicsTemplates,
        ],
        WorkflowLens::Color => vec![CapabilityArea::ColorFinishing],
        WorkflowLens::Audio => vec![CapabilityArea::AudioFinishing],
        WorkflowLens::Delivery => vec![CapabilityArea::DeliveryProfilesAndPreflight],
        WorkflowLens::Preflight => vec![CapabilityArea::DeliveryProfilesAndPreflight],
    }
}

fn all_lenses() -> Vec<WorkflowLens> {
    vec![
        WorkflowLens::Media,
        WorkflowLens::Selects,
        WorkflowLens::Assembly,
        WorkflowLens::EditReview,
        WorkflowLens::Vfx,
        WorkflowLens::Color,
        WorkflowLens::Audio,
        WorkflowLens::Delivery,
        WorkflowLens::Preflight,
    ]
}

/// Inspect registry, planner pass data-flow, and conflicts for autopilot gates.
pub fn inspect_pre_autonomy_readiness(
    metadata: &AwidatTimelineMetadata,
) -> OrchestrationInspection {
    let registry = metadata
        .capability_registry
        .clone()
        .unwrap_or_else(CapabilityRegistry::professional_substrate_v1);
    let mut readiness = PipelineReadinessReport::from_registry(registry);
    merge_metadata_readiness(metadata, &mut readiness);
    OrchestrationInspection {
        readiness,
        conflicts: collect_cross_stage_conflicts(&metadata.planner_passes),
        planner_edges: planner_edges(&metadata.planner_passes),
    }
}

fn merge_metadata_readiness(
    metadata: &AwidatTimelineMetadata,
    readiness: &mut PipelineReadinessReport,
) {
    let metadata_readiness = metadata.build_professional_readiness_report();
    for stage in metadata_readiness.stages {
        if let Some(existing) = readiness
            .stages
            .iter_mut()
            .find(|existing| existing.area == stage.area)
            && existing.state == ReadinessState::Ready
            && stage.state == ReadinessState::Blocked
        {
            existing.state = ReadinessState::Blocked;
            existing.blocker = stage.blocker;
        }
    }
}

fn collect_cross_stage_conflicts(passes: &[PlannerPassContract]) -> Vec<PipelineConflict> {
    let mut seen = HashSet::new();
    let mut conflicts = Vec::new();
    for conflict in passes.iter().flat_map(|pass| pass.conflicts.iter()) {
        if seen.insert(conflict.id.clone()) {
            conflicts.push(conflict.clone());
        }
    }
    conflicts
}

fn planner_edges(passes: &[PlannerPassContract]) -> Vec<PlannerPassEdge> {
    let mut producers: HashMap<&str, Vec<&str>> = HashMap::new();
    for pass in passes {
        for output in &pass.outputs {
            producers
                .entry(output.as_str())
                .or_default()
                .push(pass.id.as_str());
        }
    }
    let mut edges = Vec::new();
    for pass in passes {
        for input in &pass.inputs {
            if let Some(from_ids) = producers.get(input.as_str()) {
                for from_id in from_ids {
                    if *from_id != pass.id {
                        edges.push(PlannerPassEdge {
                            from_pass_id: (*from_id).to_string(),
                            to_pass_id: pass.id.clone(),
                            artifact: input.clone(),
                        });
                    }
                }
            }
        }
    }
    edges
}

/// Capture a learning signal from an accepted or rejected proposal.
pub fn record_learning_signal(metadata: &mut AwidatTimelineMetadata, signal: LearningSignal) {
    metadata.learning_signals.push(signal);
}
