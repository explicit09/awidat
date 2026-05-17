//! Professional workflow lens and pre-autonomy orchestration helpers.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use awidat_proto::awidat_meta::AwidatTimelineMetadata;
use awidat_proto::otio::{
    Clip, Marker, MediaReference, Stack, StackChild, Timeline, Track, TrackChild,
    TrackKind as OtioTrackKind,
};
use awidat_proto::professional::{
    AssetBin, AssetCatalog, AssetProvenance, AssetReadiness, AssetRecord, AssetRole,
    AudioAutomationLane, AudioBus, AudioChainPreset, AudioFinishingState, AudioMeterReading,
    AudioRole, CapabilityArea, CapabilityRegistry, ColorFinishingState, ColorManagement,
    CompositionEdge, CompositionGraph, CompositionNode, CompositionNodeType, DeliveryProfile,
    FindingSeverity, GradeStack, GradeStage, Keyframe, LearningSignal, MaskKeyframe, MaskOperation,
    MaskSidecar, MatteSidecar, PipelineConflict, PipelineReadinessReport, PlannerPassContract,
    PreflightReport, ProfessionalDiagnostic, ReadinessState, ReferenceStill, ReviewStatus,
    SelectDecision, ShotGroup, SourceRange, SourceSelect, Stringout, TrackKind as VfxTrackKind,
    TrackSample, TrackSidecar, TrackingPackage, WorkflowLens,
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

/// Derived VFX package with compositing graphs and tracking sidecars.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct VfxPipelinePackage {
    /// Per-shot composition graphs.
    pub composition_graphs: Vec<CompositionGraph>,
    /// Tracking, mask, and matte contracts needed by those graphs.
    pub tracking_package: TrackingPackage,
    /// Per-shot vendor handoff and versioning records.
    pub shot_manifests: Vec<VfxShotManifest>,
}

/// Per-shot VFX handoff record.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct VfxShotManifest {
    /// Stable Awidat VFX shot id.
    pub shot_id: String,
    /// Timeline clip id.
    pub clip_id: String,
    /// Vendor-facing shot name.
    pub vendor_shot_name: String,
    /// Assigned vendor, if known.
    pub vendor: Option<String>,
    /// Source plate asset.
    pub source_asset: String,
    /// Timeline start in seconds.
    pub timeline_start_s: f64,
    /// Timeline end in seconds.
    pub timeline_end_s: f64,
    /// Source in point in seconds.
    pub source_start_s: Option<f64>,
    /// Source out point in seconds.
    pub source_end_s: Option<f64>,
    /// Available source handle before the pull.
    pub handle_in_s: Option<f64>,
    /// Available source handle after the pull.
    pub handle_out_s: Option<f64>,
    /// Pulled plate reference.
    pub pull_ref: String,
    /// Review movie/reference for the current version.
    pub review_ref: String,
    /// Current VFX version, if known.
    pub current_version: Option<String>,
    /// Effects driving this shot record.
    pub effect_names: Vec<String>,
}

/// Review package with collaboration security policy.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SecureReviewPackage {
    /// Stable package id.
    pub id: String,
    /// Pending proposal packages included for review.
    pub proposals: Vec<awidat_proto::professional::ProposalPackage>,
    /// Artifact references reviewers need to inspect.
    pub artifact_refs: Vec<String>,
    /// Current reviewable versions exposed to collaborators.
    pub review_versions: Vec<ReviewVersion>,
    /// Frame/time-specific notes derived from proposal evidence.
    pub comments: Vec<ReviewComment>,
    /// Security policy for review distribution.
    pub security: ReviewSecurityPolicy,
}

/// Reviewable cut/version entry for collaboration tools.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewVersion {
    /// Stable review version id.
    pub id: String,
    /// Review package id this version belongs to.
    pub package_id: String,
    /// Cut/version label.
    pub cut_id: String,
    /// Artifacts included in this review version.
    pub artifact_refs: Vec<String>,
    /// Rollback refs available if the review rejects the proposal.
    pub rollback_refs: Vec<String>,
}

/// Frame/time-specific review note.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ReviewComment {
    /// Stable comment id.
    pub id: String,
    /// Source proposal id.
    pub proposal_id: String,
    /// Reviewer this package is addressed to.
    pub reviewer: String,
    /// Human-readable note body.
    pub body: String,
    /// Optional frame number.
    pub frame: Option<u64>,
    /// Optional timeline time in seconds.
    pub time_s: Option<f64>,
    /// Evidence refs this comment points at.
    pub artifact_refs: Vec<String>,
}

/// Access policy for a review package.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewSecurityPolicy {
    /// Reviewers allowed to access the package.
    pub allowed_reviewers: Vec<String>,
    /// Whether authenticated access is required.
    pub require_auth: bool,
    /// Whether package media/download is allowed.
    pub allow_download: bool,
    /// Burn-in or overlay watermark text for review exports.
    pub watermark: String,
}

/// Timeline scalability profile for large-project UI planning.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TimelineScaleProfile {
    /// Top-level track count.
    pub track_count: usize,
    /// Timeline clip count.
    pub clip_count: usize,
    /// Timeline item count including clips, gaps, transitions, and nested stacks.
    pub item_count: usize,
    /// Longest track duration in seconds.
    pub total_duration_s: f64,
    /// Maximum rows the caller should render at once.
    pub max_visible_rows: usize,
    /// Suggested time window for timeline inspection.
    pub recommended_window_s: f64,
    /// Whether the UI should virtualize/page rows instead of rendering all items.
    pub requires_virtualization: bool,
    /// Maximum timeline items expected in one UI/agent window.
    pub max_items_per_window: usize,
    /// Row windows callers can render independently.
    pub row_windows: Vec<TimelineRowWindow>,
    /// Timeline time windows callers can query independently.
    pub time_windows: Vec<TimelineTimeWindow>,
    /// Stable refs for row/time window intersections.
    pub agent_window_refs: Vec<String>,
    /// Human-readable implementation guidance.
    pub guidance: Vec<String>,
}

/// Exclusive row window for virtualized timeline rendering.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimelineRowWindow {
    /// Inclusive start row.
    pub start_row: usize,
    /// Exclusive end row.
    pub end_row: usize,
}

/// Time window for agent/context paging.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TimelineTimeWindow {
    /// Inclusive start time.
    pub start_s: f64,
    /// Exclusive end time, except the final window may equal total duration.
    pub end_s: f64,
}

/// Integrated professional pipeline snapshot for the current cut.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProfessionalPipelineSnapshot {
    /// Derived media catalog.
    pub asset_catalog: AssetCatalog,
    /// Media operations plan for proxies, indexes, relinks, backup, and archive.
    pub media_operations: MediaOperationsState,
    /// Assistant-editor dailies, sync, and source-review state.
    pub assistant_workflow: AssistantWorkflowState,
    /// Derived audio finishing state.
    pub audio_finishing: AudioFinishingState,
    /// Derived color finishing state.
    pub color_finishing: ColorFinishingState,
    /// Derived VFX pipeline package.
    pub vfx: VfxPipelinePackage,
    /// Secure review package for pending proposals.
    pub review: SecureReviewPackage,
    /// Delivery, QC, and archive package.
    pub delivery: DeliveryMasterPackage,
    /// Timeline scale profile for UI and agent windowing.
    pub scale_profile: TimelineScaleProfile,
    /// Workflow lens snapshots over the derived professional state.
    pub lens_snapshots: Vec<WorkflowLensSnapshot>,
}

/// Derived assistant-editor source review state.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AssistantSourceReview {
    /// Durable source selects.
    pub selects: Vec<SourceSelect>,
    /// Ordered stringouts built from non-rejected selects.
    pub stringouts: Vec<Stringout>,
}

/// Derived assistant-editor workflow state.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AssistantWorkflowState {
    /// Durable source review state.
    pub source_review: AssistantSourceReview,
    /// Picture/sound dailies sync groups.
    pub sync_groups: Vec<AssistantSyncGroup>,
}

/// Picture/sound sync group prepared by assistant-editor metadata.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssistantSyncGroup {
    /// Stable sync group id.
    pub id: String,
    /// Clip ids participating in the group.
    pub clip_ids: Vec<String>,
    /// Source asset ids participating in the group.
    pub asset_ids: Vec<String>,
    /// Optional scene/slate id.
    pub scene: Option<String>,
    /// Optional take id.
    pub take: Option<String>,
    /// Whether the group has enough online picture and sound to sync.
    pub readiness: ReadinessState,
}

/// Media operations plan derived from asset readiness.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaOperationsState {
    /// Total known assets.
    pub asset_count: usize,
    /// Assets whose original media is online.
    pub online_count: usize,
    /// Assets whose original media is offline or missing.
    pub offline_count: usize,
    /// Concrete media operations still needed or worth recording.
    pub actions: Vec<MediaOperationAction>,
    /// Archive records that should be preserved with the project.
    pub archive_refs: Vec<String>,
}

/// One concrete media operation.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaOperationAction {
    /// Stable action id.
    pub id: String,
    /// Source asset id.
    pub asset_id: String,
    /// Operation kind.
    pub kind: String,
    /// Source path or URI.
    pub source: String,
    /// Target path or manifest reference.
    pub target: Option<String>,
    /// Whether this action is blocked until user/media input is supplied.
    pub blocked: bool,
}

/// Audio post handoff package.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AudioPostPackage {
    /// Target session/reference path for downstream sound editors.
    pub session_ref: String,
    /// Stem deliverables expected from the mix.
    pub stem_deliverables: Vec<AudioStemDeliverable>,
    /// ADR/Foley/SFX cue sheet.
    pub cues: Vec<AudioPostCue>,
}

/// Expected audio stem.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioStemDeliverable {
    /// Stable stem id.
    pub id: String,
    /// Source bus id.
    pub bus_id: String,
    /// Output path.
    pub path: String,
}

/// Sound editorial cue.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AudioPostCue {
    /// Stable cue id.
    pub id: String,
    /// Cue kind, such as adr, foley, or sfx.
    pub kind: String,
    /// Source clip id.
    pub clip_id: String,
    /// Optional source asset id.
    pub asset_id: Option<String>,
    /// Timeline start in seconds.
    pub timeline_start_s: f64,
    /// Timeline end in seconds.
    pub timeline_end_s: f64,
    /// Cue label.
    pub label: String,
    /// Optional cue note.
    pub note: Option<String>,
}

/// Color department handoff package.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ColorHandoffPackage {
    /// Interchange timeline for the grading system.
    pub timeline_ref: String,
    /// Aggregate CDL package path.
    pub cdl_path: String,
    /// LUTs required to view or reproduce the editorial look.
    pub lut_refs: Vec<String>,
    /// Reference stills carried into the color session.
    pub reference_stills: Vec<ReferenceStill>,
    /// Per-shot handoff records for conform and grading.
    pub shot_manifests: Vec<ColorShotManifest>,
}

/// Per-shot color handoff record.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColorShotManifest {
    /// Timeline clip id.
    pub clip_id: String,
    /// Grade stack attached to the clip.
    pub grade_stack_id: String,
    /// Source media used for conform.
    pub source_asset: String,
    /// Optional camera technical LUT.
    pub camera_lut: Option<String>,
    /// Optional show creative LUT.
    pub show_lut: Option<String>,
    /// Per-shot CDL sidecar reference.
    pub cdl_ref: String,
}

/// Delivery/QC package for final masters and archive handoff.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DeliveryMasterPackage {
    /// Delivery profile driving the package.
    pub profile_id: String,
    /// Required and discovered delivery artifacts.
    pub deliverables: Vec<DeliveryArtifact>,
    /// QC/preflight report references.
    pub qc_reports: Vec<String>,
    /// Archive refs that should be retained with the delivery.
    pub archive_refs: Vec<String>,
    /// Whether a blocking preflight finding prevents delivery.
    pub blocked: bool,
}

/// One delivery artifact contract.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveryArtifact {
    /// Stable artifact id.
    pub id: String,
    /// Artifact kind, such as prores_master, dcp, imf, caption, or audio_stem.
    pub kind: String,
    /// Expected path.
    pub path: String,
    /// Whether delivery requires this artifact.
    pub required: bool,
    /// Whether the current package manifests already include the artifact.
    pub present: bool,
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

/// Derive an assistant-editor asset catalog from declared sources and timeline usage.
pub fn derive_assistant_asset_catalog(project_root: &Path, timeline: &Timeline) -> AssetCatalog {
    let mut assets = BTreeMap::new();
    if let Some(metadata) = &timeline.metadata.awidat {
        for asset in &metadata.source_assets {
            upsert_assistant_asset(project_root, &mut assets, asset, "source", None);
        }
    }
    collect_stack_assets(project_root, &timeline.tracks, &mut assets);

    let mut bin_ids = BTreeSet::new();
    let mut records = assets.into_values().collect::<Vec<_>>();
    for record in &mut records {
        record.tags.sort();
        record.tags.dedup();
        record.usage.clip_ids.sort();
        record.usage.clip_ids.dedup();
        if let Some(bin_id) = &record.bin_id {
            bin_ids.insert(bin_id.clone());
        }
    }

    AssetCatalog {
        assets: records,
        bins: bin_ids
            .into_iter()
            .map(|id| AssetBin {
                name: assistant_bin_name(&id).to_string(),
                id,
                parent_id: None,
            })
            .collect(),
        ..AssetCatalog::default()
    }
}

fn collect_stack_assets(
    project_root: &Path,
    stack: &Stack,
    assets: &mut BTreeMap<String, AssetRecord>,
) {
    for child in &stack.children {
        match child {
            StackChild::Track(track) => collect_track_assets(project_root, track, assets),
            StackChild::Stack(nested) => collect_stack_assets(project_root, nested, assets),
            StackChild::Clip(clip) => collect_clip_asset(project_root, assets, clip),
            StackChild::Gap(_) => {}
        }
    }
}

fn collect_track_assets(
    project_root: &Path,
    track: &Track,
    assets: &mut BTreeMap<String, AssetRecord>,
) {
    for child in &track.children {
        match child {
            TrackChild::Clip(clip) => collect_clip_asset(project_root, assets, clip),
            TrackChild::Stack(stack) => collect_stack_assets(project_root, stack, assets),
            TrackChild::Gap(_) | TrackChild::Transition(_) => {}
        }
    }
}

fn collect_clip_asset(
    project_root: &Path,
    assets: &mut BTreeMap<String, AssetRecord>,
    clip: &Clip,
) {
    if let MediaReference::External(reference) = &clip.media_reference {
        let clip_id = assistant_clip_id(clip);
        upsert_assistant_asset(
            project_root,
            assets,
            &reference.target_url,
            "timeline",
            Some(clip_id),
        );
    }
}

fn upsert_assistant_asset(
    project_root: &Path,
    assets: &mut BTreeMap<String, AssetRecord>,
    asset_path: &str,
    tag: &str,
    clip_id: Option<String>,
) {
    let path = asset_path.trim();
    if path.is_empty() {
        return;
    }
    let role = assistant_asset_role(path);
    let bin_id = assistant_bin_id(role).to_string();
    let record = assets
        .entry(path.to_string())
        .or_insert_with(|| assistant_asset_record(project_root, path, role, &bin_id));
    if !record.tags.iter().any(|existing| existing == tag) {
        record.tags.push(tag.to_string());
    }
    if let Some(clip_id) = clip_id
        && !record.usage.clip_ids.contains(&clip_id)
    {
        record.usage.clip_ids.push(clip_id);
    }
}

fn assistant_asset_record(
    project_root: &Path,
    path: &str,
    role: AssetRole,
    bin_id: &str,
) -> AssetRecord {
    let resolved = resolve_assistant_asset_path(project_root, path);
    let checksum = resolved
        .as_ref()
        .filter(|resolved| resolved.is_file())
        .and_then(|resolved| awidat_index::asset_sha256(resolved).ok());
    let online = if checksum.is_some() {
        ReadinessState::Ready
    } else {
        ReadinessState::Blocked
    };
    let proxy = assistant_proxy_readiness(project_root, resolved.as_deref(), online);
    let index = assistant_index_readiness(project_root, path);
    AssetRecord {
        id: path.to_string(),
        path: path.to_string(),
        role,
        bin_id: Some(bin_id.to_string()),
        tags: Vec::new(),
        label: None,
        rating: None,
        readiness: AssetReadiness {
            proxy,
            index,
            online,
        },
        provenance: Some(AssetProvenance {
            imported_from: Some(path.to_string()),
            checksum,
            created_by: Some("awidat-assistant-catalog".to_string()),
        }),
        usage: Default::default(),
    }
}

fn assistant_proxy_readiness(
    project_root: &Path,
    resolved_asset: Option<&Path>,
    online: ReadinessState,
) -> ReadinessState {
    if online == ReadinessState::Blocked {
        return ReadinessState::Blocked;
    }
    let Some(resolved_asset) = resolved_asset else {
        return ReadinessState::Unavailable;
    };
    if assistant_proxy_path(project_root, resolved_asset).is_file() {
        ReadinessState::Ready
    } else {
        ReadinessState::Pending
    }
}

fn assistant_proxy_path(project_root: &Path, asset_path: &Path) -> PathBuf {
    let stem = asset_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("asset");
    project_root.join(".awidat").join("proxies").join(format!(
        "{stem}-{:08x}.mp4",
        assistant_stable_path_hash(asset_path)
    ))
}

fn assistant_stable_path_hash(path: &Path) -> u32 {
    let mut hash = 0x811c9dc5_u32;
    for byte in path.to_string_lossy().as_bytes() {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(0x01000193);
    }
    hash
}

fn assistant_index_readiness(project_root: &Path, asset_path: &str) -> ReadinessState {
    if asset_path.contains("://") {
        return ReadinessState::Unavailable;
    }
    let asset_id = awidat_proto::index::AssetId::new(asset_path.to_string());
    let index_root = project_root.join(awidat_proto::project::files::INDEX_DIR);
    let Ok(entries) = std::fs::read_dir(index_root) else {
        return ReadinessState::Pending;
    };
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_dir() {
            continue;
        }
        let indexer = entry.file_name().to_string_lossy().to_string();
        let Ok(sidecar_path) = awidat_index::sidecar_path(project_root, &indexer, &asset_id) else {
            continue;
        };
        if sidecar_path.is_file() {
            return ReadinessState::Ready;
        }
    }
    ReadinessState::Pending
}

fn resolve_assistant_asset_path(project_root: &Path, path: &str) -> Option<PathBuf> {
    if path.contains("://") {
        return None;
    }
    let path = Path::new(path);
    if path.is_absolute() {
        Some(path.to_path_buf())
    } else {
        Some(project_root.join(path))
    }
}

fn assistant_asset_role(path: &str) -> AssetRole {
    match Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("wav" | "aif" | "aiff" | "mp3" | "m4a" | "flac" | "ogg") => AssetRole::Audio,
        Some("png" | "jpg" | "jpeg" | "tif" | "tiff" | "webp" | "heic") => AssetRole::Still,
        Some("psd" | "ai" | "svg") => AssetRole::Graphic,
        Some("srt" | "vtt" | "itt" | "stl") => AssetRole::Caption,
        Some("cube" | "cdl" | "json" | "xml" | "otio" | "edl" | "ale") => AssetRole::Support,
        _ => AssetRole::Video,
    }
}

fn assistant_bin_id(role: AssetRole) -> &'static str {
    match role {
        AssetRole::Video => "video",
        AssetRole::Audio => "audio",
        AssetRole::Still => "stills",
        AssetRole::Graphic => "graphics",
        AssetRole::Caption => "captions",
        AssetRole::Support => "support",
    }
}

fn assistant_bin_name(id: &str) -> &'static str {
    match id {
        "video" => "Video",
        "audio" => "Audio",
        "stills" => "Stills",
        "graphics" => "Graphics",
        "captions" => "Captions",
        "support" => "Support",
        _ => "Assets",
    }
}

/// Derive concrete media operations from catalog readiness.
pub fn derive_media_operations_state(
    project_root: &Path,
    catalog: &AssetCatalog,
) -> MediaOperationsState {
    let mut actions = Vec::new();
    for asset in &catalog.assets {
        collect_media_operation_actions(project_root, asset, &mut actions);
    }
    actions.sort_by(|a, b| a.asset_id.cmp(&b.asset_id).then(a.kind.cmp(&b.kind)));

    MediaOperationsState {
        asset_count: catalog.assets.len(),
        online_count: catalog
            .assets
            .iter()
            .filter(|asset| asset.readiness.online == ReadinessState::Ready)
            .count(),
        offline_count: catalog
            .assets
            .iter()
            .filter(|asset| asset.readiness.online == ReadinessState::Blocked)
            .count(),
        actions,
        archive_refs: vec![
            "archive/media/checksums.sha256".to_string(),
            "archive/media/relink-map.json".to_string(),
            "archive/media/source-manifest.json".to_string(),
        ],
    }
}

fn collect_media_operation_actions(
    project_root: &Path,
    asset: &AssetRecord,
    actions: &mut Vec<MediaOperationAction>,
) {
    if asset.readiness.online == ReadinessState::Ready {
        actions.push(media_operation_action(
            asset,
            "verify_checksum",
            asset.path.clone(),
            Some("archive/media/checksums.sha256".to_string()),
            false,
        ));
        actions.push(media_operation_action(
            asset,
            "backup_source",
            asset.path.clone(),
            Some(format!("backups/{}", asset.path)),
            false,
        ));
    } else if asset.readiness.online == ReadinessState::Blocked {
        actions.push(media_operation_action(
            asset,
            "relink_offline_media",
            asset.path.clone(),
            Some("archive/media/relink-map.json".to_string()),
            true,
        ));
    }

    if asset.readiness.proxy == ReadinessState::Pending {
        actions.push(media_operation_action(
            asset,
            "generate_proxy",
            asset.path.clone(),
            media_proxy_target(project_root, asset),
            false,
        ));
    }
    if asset.readiness.index == ReadinessState::Pending {
        actions.push(media_operation_action(
            asset,
            "run_indexing",
            asset.path.clone(),
            Some("index/".to_string()),
            false,
        ));
    }
}

fn media_operation_action(
    asset: &AssetRecord,
    kind: &str,
    source: String,
    target: Option<String>,
    blocked: bool,
) -> MediaOperationAction {
    MediaOperationAction {
        id: format!(
            "media-{}-{}",
            slug_for_review_id(kind),
            slug_for_review_id(&asset.id)
        ),
        asset_id: asset.id.clone(),
        kind: kind.to_string(),
        source,
        target,
        blocked,
    }
}

fn media_proxy_target(project_root: &Path, asset: &AssetRecord) -> Option<String> {
    resolve_assistant_asset_path(project_root, &asset.path)
        .map(|path| assistant_proxy_path(project_root, &path))
        .map(|path| {
            path.strip_prefix(project_root)
                .unwrap_or(&path)
                .to_path_buf()
        })
        .map(|path| path.to_string_lossy().to_string())
}

fn assistant_clip_id(clip: &Clip) -> String {
    clip.metadata
        .awidat
        .as_ref()
        .and_then(|metadata| metadata.extra.get("clip_uuid"))
        .and_then(|value| value.as_str())
        .filter(|uuid| !uuid.trim().is_empty())
        .unwrap_or(clip.name.as_str())
        .to_string()
}

/// Derive durable source selects and a first-pass stringout from clip markers.
pub fn derive_assistant_source_review(timeline: &Timeline) -> AssistantSourceReview {
    let mut selects = Vec::new();
    let mut seen_ids = HashSet::new();
    collect_stack_source_selects(&timeline.tracks, &mut selects, &mut seen_ids);

    let select_ids = selects
        .iter()
        .filter(|select| select.decision != SelectDecision::Reject)
        .map(|select| select.id.clone())
        .collect::<Vec<_>>();
    let stringouts = if select_ids.is_empty() {
        Vec::new()
    } else {
        vec![Stringout {
            id: "stringout-assistant-selects".to_string(),
            name: Some("Assistant Selects".to_string()),
            select_ids,
        }]
    };

    AssistantSourceReview {
        selects,
        stringouts,
    }
}

fn collect_stack_source_selects(
    stack: &Stack,
    selects: &mut Vec<SourceSelect>,
    seen_ids: &mut HashSet<String>,
) {
    for child in &stack.children {
        match child {
            StackChild::Track(track) => collect_track_source_selects(track, selects, seen_ids),
            StackChild::Stack(nested) => collect_stack_source_selects(nested, selects, seen_ids),
            StackChild::Clip(clip) => collect_clip_source_selects(clip, selects, seen_ids),
            StackChild::Gap(_) => {}
        }
    }
}

fn collect_track_source_selects(
    track: &Track,
    selects: &mut Vec<SourceSelect>,
    seen_ids: &mut HashSet<String>,
) {
    for child in &track.children {
        match child {
            TrackChild::Clip(clip) => collect_clip_source_selects(clip, selects, seen_ids),
            TrackChild::Stack(stack) => collect_stack_source_selects(stack, selects, seen_ids),
            TrackChild::Gap(_) | TrackChild::Transition(_) => {}
        }
    }
}

fn collect_clip_source_selects(
    clip: &Clip,
    selects: &mut Vec<SourceSelect>,
    seen_ids: &mut HashSet<String>,
) {
    let MediaReference::External(reference) = &clip.media_reference else {
        return;
    };
    let clip_id = assistant_clip_id(clip);
    for marker in &clip.markers {
        let Some(decision) = marker_select_decision(marker) else {
            continue;
        };
        let Some(range) = marker_source_range(clip, marker) else {
            continue;
        };
        let base_id = format!(
            "select-{}-{}",
            slug_for_review_id(&clip_id),
            slug_for_review_id(&marker.name)
        );
        let id = unique_source_select_id(&base_id, seen_ids);
        selects.push(SourceSelect {
            id,
            asset_id: reference.target_url.clone(),
            range,
            decision,
            take_group_id: source_select_take_group(clip, marker),
            rank: source_select_rank(marker),
            reason: source_select_reason(marker),
            evidence_refs: source_select_evidence_refs(marker),
            notes: source_select_notes(marker),
        });
    }
}

fn marker_select_decision(marker: &Marker) -> Option<SelectDecision> {
    let category = marker
        .metadata
        .awidat
        .as_ref()
        .and_then(|metadata| metadata.category.as_deref())
        .or_else(|| marker.name.split_whitespace().next())
        .map(|value| value.trim().to_ascii_lowercase())?;
    match category.as_str() {
        "select" | "selected" | "keep" | "keeper" | "circled" => Some(SelectDecision::Select),
        "favorite" | "favourite" | "fav" | "best" => Some(SelectDecision::Favorite),
        "maybe" | "alt" | "alternate" => Some(SelectDecision::Maybe),
        "reject" | "rejected" | "no" | "ng" => Some(SelectDecision::Reject),
        _ => None,
    }
}

fn marker_source_range(clip: &Clip, marker: &Marker) -> Option<SourceRange> {
    let marker_start_s = marker.marked_range.start_time.to_seconds();
    let marker_duration_s = marker.marked_range.duration.to_seconds();
    let source_start_s = clip
        .source_range
        .as_ref()
        .map(|range| range.start_time.to_seconds())
        .unwrap_or(0.0);
    let range = SourceRange {
        start_s: source_start_s + marker_start_s,
        end_s: source_start_s + marker_start_s + marker_duration_s,
    };
    range.is_valid().then_some(range)
}

fn source_select_take_group(clip: &Clip, marker: &Marker) -> Option<String> {
    marker
        .metadata
        .awidat
        .as_ref()
        .and_then(|metadata| metadata.extra.get("take_group_id"))
        .and_then(serde_json::Value::as_str)
        .or_else(|| {
            clip.metadata
                .awidat
                .as_ref()
                .and_then(|metadata| metadata.extra.get("take_group_id"))
                .and_then(serde_json::Value::as_str)
        })
        .map(str::to_string)
}

fn source_select_rank(marker: &Marker) -> Option<u32> {
    marker
        .metadata
        .awidat
        .as_ref()
        .and_then(|metadata| metadata.extra.get("rank"))
        .and_then(serde_json::Value::as_u64)
        .and_then(|rank| u32::try_from(rank).ok())
}

fn source_select_reason(marker: &Marker) -> Option<String> {
    marker
        .metadata
        .awidat
        .as_ref()
        .and_then(|metadata| metadata.note.clone())
        .or_else(|| marker.comment.clone())
}

fn source_select_evidence_refs(marker: &Marker) -> Vec<String> {
    let Some(metadata) = marker.metadata.awidat.as_ref() else {
        return Vec::new();
    };
    let mut refs = Vec::new();
    if let Some(reference) = metadata
        .extra
        .get("evidence_ref")
        .and_then(serde_json::Value::as_str)
    {
        refs.push(reference.to_string());
    }
    if let Some(values) = metadata
        .extra
        .get("evidence_refs")
        .and_then(serde_json::Value::as_array)
    {
        refs.extend(
            values
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::to_string),
        );
    }
    refs.sort();
    refs.dedup();
    refs
}

fn source_select_notes(marker: &Marker) -> Vec<String> {
    marker
        .comment
        .iter()
        .chain(
            marker
                .metadata
                .awidat
                .as_ref()
                .and_then(|metadata| metadata.note.as_ref()),
        )
        .cloned()
        .collect()
}

fn unique_source_select_id(base_id: &str, seen_ids: &mut HashSet<String>) -> String {
    if seen_ids.insert(base_id.to_string()) {
        return base_id.to_string();
    }
    let mut index = 2;
    loop {
        let candidate = format!("{base_id}-{index}");
        if seen_ids.insert(candidate.clone()) {
            return candidate;
        }
        index += 1;
    }
}

fn annotate_catalog_select_usage(catalog: &mut AssetCatalog, selects: &[SourceSelect]) {
    for select in selects {
        if let Some(asset) = catalog
            .assets
            .iter_mut()
            .find(|asset| asset.id == select.asset_id || asset.path == select.asset_id)
            && !asset.usage.select_ids.contains(&select.id)
        {
            asset.usage.select_ids.push(select.id.clone());
        }
    }
    for asset in &mut catalog.assets {
        asset.usage.select_ids.sort();
        asset.usage.select_ids.dedup();
    }
}

/// Derive assistant-editor workflow state for dailies and source review.
pub fn derive_assistant_workflow_state(timeline: &Timeline) -> AssistantWorkflowState {
    AssistantWorkflowState {
        source_review: derive_assistant_source_review(timeline),
        sync_groups: derive_assistant_sync_groups(timeline),
    }
}

fn derive_assistant_sync_groups(timeline: &Timeline) -> Vec<AssistantSyncGroup> {
    let mut groups = BTreeMap::<String, AssistantSyncGroupBuilder>::new();
    collect_stack_sync_groups(&timeline.tracks, &mut groups);
    groups
        .into_iter()
        .map(|(id, mut group)| {
            group.clip_ids.sort();
            group.clip_ids.dedup();
            group.asset_ids.sort();
            group.asset_ids.dedup();
            AssistantSyncGroup {
                id,
                clip_ids: group.clip_ids,
                asset_ids: group.asset_ids,
                scene: group.scene,
                take: group.take,
                readiness: if group.has_picture && group.has_sound {
                    ReadinessState::Ready
                } else {
                    ReadinessState::Pending
                },
            }
        })
        .collect()
}

#[derive(Debug, Clone, Default)]
struct AssistantSyncGroupBuilder {
    clip_ids: Vec<String>,
    asset_ids: Vec<String>,
    scene: Option<String>,
    take: Option<String>,
    has_picture: bool,
    has_sound: bool,
}

fn collect_stack_sync_groups(
    stack: &Stack,
    groups: &mut BTreeMap<String, AssistantSyncGroupBuilder>,
) {
    for child in &stack.children {
        match child {
            StackChild::Track(track) => collect_track_sync_groups(track, groups),
            StackChild::Stack(stack) => collect_stack_sync_groups(stack, groups),
            StackChild::Clip(clip) => collect_clip_sync_group(clip, None, groups),
            StackChild::Gap(_) => {}
        }
    }
}

fn collect_track_sync_groups(
    track: &Track,
    groups: &mut BTreeMap<String, AssistantSyncGroupBuilder>,
) {
    for child in &track.children {
        match child {
            TrackChild::Clip(clip) => collect_clip_sync_group(clip, Some(track.kind), groups),
            TrackChild::Stack(stack) => collect_stack_sync_groups(stack, groups),
            TrackChild::Gap(_) | TrackChild::Transition(_) => {}
        }
    }
}

fn collect_clip_sync_group(
    clip: &Clip,
    track_kind: Option<OtioTrackKind>,
    groups: &mut BTreeMap<String, AssistantSyncGroupBuilder>,
) {
    let Some(metadata) = clip.metadata.awidat.as_ref() else {
        return;
    };
    let Some(group_id) = assistant_string_extra(metadata, "sync_group_id")
        .or_else(|| assistant_string_extra(metadata, "link_group_id"))
        .or_else(|| assistant_string_extra(metadata, "take_group_id"))
    else {
        return;
    };
    let group = groups.entry(group_id).or_default();
    let clip_id = assistant_clip_id(clip);
    if !group.clip_ids.contains(&clip_id) {
        group.clip_ids.push(clip_id);
    }
    if let MediaReference::External(reference) = &clip.media_reference
        && !group.asset_ids.contains(&reference.target_url)
    {
        group.asset_ids.push(reference.target_url.clone());
    }
    if group.scene.is_none() {
        group.scene = assistant_string_extra(metadata, "scene");
    }
    if group.take.is_none() {
        group.take = assistant_string_extra(metadata, "take");
    }
    let is_sound = track_kind == Some(OtioTrackKind::Audio)
        || matches!(&clip.media_reference, MediaReference::External(reference) if assistant_asset_role(&reference.target_url) == AssetRole::Audio);
    if is_sound {
        group.has_sound = true;
    } else {
        group.has_picture = true;
    }
}

fn assistant_string_extra(
    metadata: &awidat_proto::awidat_meta::AwidatClipMetadata,
    key: &str,
) -> Option<String> {
    metadata
        .extra
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn merge_assistant_source_review(
    metadata: &mut AwidatTimelineMetadata,
    source_review: AssistantSourceReview,
) {
    let mut select_ids = metadata
        .selects
        .iter()
        .map(|select| select.id.clone())
        .collect::<HashSet<_>>();
    metadata.selects.extend(
        source_review
            .selects
            .into_iter()
            .filter(|select| select_ids.insert(select.id.clone())),
    );

    let mut stringout_ids = metadata
        .stringouts
        .iter()
        .map(|stringout| stringout.id.clone())
        .collect::<HashSet<_>>();
    metadata.stringouts.extend(
        source_review
            .stringouts
            .into_iter()
            .filter(|stringout| stringout_ids.insert(stringout.id.clone())),
    );
}

/// Derive a first-pass professional audio finishing plan from timeline audio tracks.
pub fn derive_audio_finishing_state(timeline: &Timeline) -> AudioFinishingState {
    let mut role_inputs: HashMap<&'static str, Vec<String>> = HashMap::new();
    collect_audio_track_inputs(&timeline.tracks, &mut role_inputs);
    let dialogue_windows = collect_dialogue_windows(&timeline.tracks);

    let mut buses = Vec::new();
    for (id, role) in audio_bus_order() {
        if let Some(mut inputs) = role_inputs.remove(id) {
            inputs.sort();
            inputs.dedup();
            buses.push(AudioBus {
                id: id.to_string(),
                role,
                inputs,
            });
        }
    }
    if buses.is_empty() {
        return AudioFinishingState::default();
    }

    let master_inputs = buses.iter().map(|bus| bus.id.clone()).collect::<Vec<_>>();
    buses.push(AudioBus {
        id: "master".to_string(),
        role: AudioRole::Master,
        inputs: master_inputs,
    });
    let automation = default_audio_automation(&buses, &dialogue_windows);
    let meters = derive_audio_meter_readings(timeline);

    AudioFinishingState {
        buses,
        automation,
        chains: default_audio_finishing_chains(),
        meters,
    }
}

fn collect_audio_track_inputs(stack: &Stack, role_inputs: &mut HashMap<&'static str, Vec<String>>) {
    for child in &stack.children {
        match child {
            StackChild::Track(track) => {
                if track.kind == OtioTrackKind::Audio {
                    role_inputs
                        .entry(audio_role_id_for_track(&track.name))
                        .or_default()
                        .push(track.name.clone());
                }
                for child in &track.children {
                    if let TrackChild::Stack(stack) = child {
                        collect_audio_track_inputs(stack, role_inputs);
                    }
                }
            }
            StackChild::Stack(stack) => collect_audio_track_inputs(stack, role_inputs),
            StackChild::Clip(_) | StackChild::Gap(_) => {}
        }
    }
}

fn audio_role_id_for_track(name: &str) -> &'static str {
    let normalized = name.to_ascii_lowercase();
    if contains_any(&normalized, &["music", "mx", "score"]) {
        "music"
    } else if contains_any(&normalized, &["sfx", "fx", "effects"]) {
        "sfx"
    } else if contains_any(&normalized, &["amb", "ambience", "room", "wild", "walla"]) {
        "ambience"
    } else if contains_any(&normalized, &["vo", "voiceover", "narration", "narrator"]) {
        "voiceover"
    } else {
        "dialogue"
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct AudioWindow {
    start_s: f64,
    end_s: f64,
}

fn collect_dialogue_windows(stack: &Stack) -> Vec<AudioWindow> {
    let mut windows = Vec::new();
    collect_dialogue_windows_from_stack(stack, &mut windows);
    windows
}

fn collect_dialogue_windows_from_stack(stack: &Stack, windows: &mut Vec<AudioWindow>) {
    for child in &stack.children {
        match child {
            StackChild::Track(track) => {
                if track.kind == OtioTrackKind::Audio
                    && audio_role_id_for_track(&track.name) == "dialogue"
                {
                    collect_dialogue_windows_from_track(track, windows);
                }
                for child in &track.children {
                    if let TrackChild::Stack(stack) = child {
                        collect_dialogue_windows_from_stack(stack, windows);
                    }
                }
            }
            StackChild::Stack(stack) => collect_dialogue_windows_from_stack(stack, windows),
            StackChild::Clip(_) | StackChild::Gap(_) => {}
        }
    }
}

fn collect_dialogue_windows_from_track(track: &Track, windows: &mut Vec<AudioWindow>) {
    let mut cursor_s = 0.0;
    for child in &track.children {
        match child {
            TrackChild::Clip(clip) => {
                let duration_s = clip_duration_s(clip);
                if duration_s.is_finite() && duration_s > 0.0 {
                    windows.push(AudioWindow {
                        start_s: cursor_s,
                        end_s: cursor_s + duration_s,
                    });
                    cursor_s += duration_s;
                }
            }
            TrackChild::Gap(gap) => {
                cursor_s += gap.source_range.duration.to_seconds();
            }
            TrackChild::Transition(transition) => {
                cursor_s += transition.in_offset.to_seconds() + transition.out_offset.to_seconds();
            }
            TrackChild::Stack(stack) => {
                collect_dialogue_windows_from_stack(stack, windows);
            }
        }
    }
}

fn contains_any(value: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| value.contains(needle))
}

fn audio_bus_order() -> [(&'static str, AudioRole); 5] {
    [
        ("dialogue", AudioRole::Dialogue),
        ("music", AudioRole::Music),
        ("sfx", AudioRole::Sfx),
        ("ambience", AudioRole::Ambience),
        ("voiceover", AudioRole::Voiceover),
    ]
}

fn default_audio_finishing_chains() -> Vec<AudioChainPreset> {
    vec![
        AudioChainPreset {
            id: "dialogue_cleanup".to_string(),
            processors: vec![
                "noise_reduction".to_string(),
                "de_esser".to_string(),
                "eq".to_string(),
                "compression".to_string(),
            ],
        },
        AudioChainPreset {
            id: "master_delivery".to_string(),
            processors: vec!["limiter".to_string(), "loudness_meter".to_string()],
        },
    ]
}

fn default_audio_automation(
    buses: &[AudioBus],
    dialogue_windows: &[AudioWindow],
) -> Vec<AudioAutomationLane> {
    let has_dialogue = buses.iter().any(|bus| bus.id == "dialogue");
    let has_music = buses.iter().any(|bus| bus.id == "music");
    if !has_dialogue || !has_music || dialogue_windows.is_empty() {
        return Vec::new();
    }

    let mut keyframes = Vec::new();
    for window in dialogue_windows {
        keyframes.push(Keyframe::linear(window.start_s, 0.0));
        keyframes.push(Keyframe::linear(window.start_s, -8.0));
        keyframes.push(Keyframe::linear(window.end_s, -8.0));
        keyframes.push(Keyframe::linear(window.end_s, 0.0));
    }

    vec![AudioAutomationLane {
        target: "music".to_string(),
        parameter: "gain_db".to_string(),
        keyframes,
    }]
}

fn derive_audio_meter_readings(timeline: &Timeline) -> Vec<AudioMeterReading> {
    let Some(measurements) = timeline
        .metadata
        .awidat
        .as_ref()
        .and_then(|metadata| metadata.extra.get("audio_measurements"))
        .and_then(serde_json::Value::as_object)
    else {
        return Vec::new();
    };

    let mut meters = measurements
        .iter()
        .filter_map(|(target, value)| audio_meter_reading(target, value))
        .collect::<Vec<_>>();
    meters.sort_by(|a, b| a.target.cmp(&b.target));
    meters
}

fn audio_meter_reading(target: &str, value: &serde_json::Value) -> Option<AudioMeterReading> {
    let target = target.trim();
    if target.is_empty() {
        return None;
    }
    let object = value.as_object()?;
    Some(AudioMeterReading {
        target: target.to_string(),
        integrated_lufs: finite_field(object, "integrated_lufs"),
        true_peak_db: finite_field(object, "true_peak_db"),
        noise_floor_db: finite_field(object, "noise_floor_db"),
        clipping: object
            .get("clipping")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
    })
}

fn finite_field(object: &serde_json::Map<String, serde_json::Value>, key: &str) -> Option<f64> {
    object
        .get(key)
        .and_then(serde_json::Value::as_f64)
        .filter(|value| value.is_finite())
}

/// Derive a sound editorial handoff package from timeline cues and buses.
pub fn derive_audio_post_package(
    timeline: &Timeline,
    finishing: &AudioFinishingState,
) -> AudioPostPackage {
    AudioPostPackage {
        session_ref: "audio/audio-finish.ptx".to_string(),
        stem_deliverables: derive_audio_stem_deliverables(finishing),
        cues: derive_audio_post_cues(timeline),
    }
}

fn derive_audio_stem_deliverables(finishing: &AudioFinishingState) -> Vec<AudioStemDeliverable> {
    finishing
        .buses
        .iter()
        .filter(|bus| bus.role != AudioRole::Master)
        .map(|bus| AudioStemDeliverable {
            id: format!("stem-{}", bus.id),
            bus_id: bus.id.clone(),
            path: format!("audio/stems/{}.wav", bus.id),
        })
        .collect()
}

fn derive_audio_post_cues(timeline: &Timeline) -> Vec<AudioPostCue> {
    let mut cues = Vec::new();
    collect_stack_audio_post_cues(&timeline.tracks, 0.0, &mut cues);
    cues.sort_by(|a, b| {
        a.timeline_start_s
            .total_cmp(&b.timeline_start_s)
            .then_with(|| a.id.cmp(&b.id))
    });
    cues
}

fn collect_stack_audio_post_cues(stack: &Stack, offset_s: f64, cues: &mut Vec<AudioPostCue>) {
    for child in &stack.children {
        match child {
            StackChild::Track(track) => collect_track_audio_post_cues(track, offset_s, cues),
            StackChild::Stack(stack) => collect_stack_audio_post_cues(stack, offset_s, cues),
            StackChild::Clip(clip) => collect_clip_audio_post_cues(clip, offset_s, cues),
            StackChild::Gap(_) => {}
        }
    }
}

fn collect_track_audio_post_cues(track: &Track, offset_s: f64, cues: &mut Vec<AudioPostCue>) {
    let mut cursor_s = offset_s;
    for child in &track.children {
        match child {
            TrackChild::Clip(clip) => {
                collect_clip_audio_post_cues(clip, cursor_s, cues);
                cursor_s += clip_duration_s(clip);
            }
            TrackChild::Gap(gap) => {
                cursor_s += gap.source_range.duration.to_seconds();
            }
            TrackChild::Transition(transition) => {
                cursor_s += transition.in_offset.to_seconds() + transition.out_offset.to_seconds();
            }
            TrackChild::Stack(stack) => collect_stack_audio_post_cues(stack, cursor_s, cues),
        }
    }
}

fn collect_clip_audio_post_cues(clip: &Clip, clip_start_s: f64, cues: &mut Vec<AudioPostCue>) {
    let clip_id = assistant_clip_id(clip);
    let asset_id = match &clip.media_reference {
        MediaReference::External(reference) => Some(reference.target_url.clone()),
        _ => None,
    };
    for marker in &clip.markers {
        let Some(kind) = audio_post_cue_kind(marker) else {
            continue;
        };
        let marker_start_s = marker.marked_range.start_time.to_seconds();
        let marker_duration_s = marker.marked_range.duration.to_seconds();
        let timeline_start_s = clip_start_s + marker_start_s;
        let timeline_end_s = timeline_start_s + marker_duration_s.max(0.0);
        cues.push(AudioPostCue {
            id: format!(
                "cue-{}-{}",
                slug_for_review_id(&clip_id),
                slug_for_review_id(&marker.name)
            ),
            kind,
            clip_id: clip_id.clone(),
            asset_id: asset_id.clone(),
            timeline_start_s,
            timeline_end_s,
            label: marker.name.clone(),
            note: marker
                .metadata
                .awidat
                .as_ref()
                .and_then(|metadata| metadata.note.clone())
                .or_else(|| marker.comment.clone()),
        });
    }
}

fn audio_post_cue_kind(marker: &Marker) -> Option<String> {
    let category = marker
        .metadata
        .awidat
        .as_ref()
        .and_then(|metadata| metadata.category.as_deref())
        .or_else(|| marker.name.split_whitespace().next())
        .map(|value| value.trim().to_ascii_lowercase())?;
    match category.as_str() {
        "adr" | "foley" | "sfx" | "fx" => Some(category),
        _ => None,
    }
}

/// Derive a first-pass color finishing plan from picture clips in the timeline.
pub fn derive_color_finishing_state(
    timeline: &Timeline,
    input_color_space: impl Into<String>,
    output_color_space: impl Into<String>,
) -> ColorFinishingState {
    let input_color_space = input_color_space.into();
    let output_color_space = output_color_space.into();
    let mut groups: BTreeMap<String, Vec<ColorClipContext>> = BTreeMap::new();
    collect_picture_clip_groups(&timeline.tracks, &mut groups);

    let mut shot_groups = Vec::new();
    let mut reference_stills = Vec::new();
    let mut grade_stacks = Vec::new();
    for (source, mut clips) in groups {
        clips.sort_by(|a, b| a.clip_id.cmp(&b.clip_id));
        clips.dedup_by(|a, b| a.clip_id == b.clip_id);
        for clip in &clips {
            if let Some(look_reference) = &clip.look_reference {
                reference_stills.push(ReferenceStill {
                    id: format!("look-{}", clip.clip_id),
                    source: look_reference.clone(),
                });
            }
        }
        let mut clip_ids = clips
            .iter()
            .map(|clip| clip.clip_id.clone())
            .collect::<Vec<_>>();
        clip_ids.sort();
        if reference_stills.is_empty() {
            reference_stills.push(ReferenceStill {
                id: "reference-1".to_string(),
                source: source.clone(),
            });
        }
        shot_groups.push(ShotGroup {
            id: source,
            clip_ids: clip_ids.clone(),
        });
        for clip in clips {
            grade_stacks.push(default_grade_stack_for_clip(
                &clip,
                &input_color_space,
                &output_color_space,
            ));
        }
    }

    ColorFinishingState {
        reference_stills,
        shot_groups,
        grade_stacks,
        color_management: Some(ColorManagement {
            input_color_space,
            output_color_space,
        }),
    }
}

/// Derive a grading handoff package from the editorial color state.
pub fn derive_color_handoff_package(
    timeline: &Timeline,
    state: &ColorFinishingState,
) -> ColorHandoffPackage {
    let clip_sources = collect_picture_clip_sources(timeline);
    let mut shot_manifests = Vec::new();
    for grade_stack in &state.grade_stacks {
        let clip_id = grade_stack_clip_id(grade_stack);
        let Some(source_asset) = color_source_asset_for_clip(&clip_sources, state, &clip_id) else {
            continue;
        };
        let (camera_lut, show_lut) = grade_stack_luts(grade_stack);
        shot_manifests.push(ColorShotManifest {
            clip_id: clip_id.clone(),
            grade_stack_id: grade_stack.id.clone(),
            source_asset,
            camera_lut,
            show_lut,
            cdl_ref: format!("color/cdl/{}.cdl", slug_for_review_id(&clip_id)),
        });
    }
    shot_manifests.sort_by(|a, b| a.clip_id.cmp(&b.clip_id));

    ColorHandoffPackage {
        timeline_ref: "color/resolve-timeline.xml".to_string(),
        cdl_path: "color/cdl/shot-grades.ccc".to_string(),
        lut_refs: color_lut_refs(state),
        reference_stills: state.reference_stills.clone(),
        shot_manifests,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ColorClipContext {
    clip_id: String,
    camera_color_space: Option<String>,
    camera_lut: Option<String>,
    show_lut: Option<String>,
    look_reference: Option<String>,
}

fn collect_picture_clip_groups(
    stack: &Stack,
    groups: &mut BTreeMap<String, Vec<ColorClipContext>>,
) {
    for child in &stack.children {
        match child {
            StackChild::Track(track) => {
                if track.kind == OtioTrackKind::Video {
                    collect_picture_track_clip_groups(track, groups);
                }
                for child in &track.children {
                    if let TrackChild::Stack(stack) = child {
                        collect_picture_clip_groups(stack, groups);
                    }
                }
            }
            StackChild::Stack(stack) => collect_picture_clip_groups(stack, groups),
            StackChild::Clip(clip) => collect_picture_clip_group(clip, groups),
            StackChild::Gap(_) => {}
        }
    }
}

fn collect_picture_track_clip_groups(
    track: &Track,
    groups: &mut BTreeMap<String, Vec<ColorClipContext>>,
) {
    for child in &track.children {
        match child {
            TrackChild::Clip(clip) => collect_picture_clip_group(clip, groups),
            TrackChild::Stack(stack) => collect_picture_clip_groups(stack, groups),
            TrackChild::Gap(_) | TrackChild::Transition(_) => {}
        }
    }
}

fn collect_picture_clip_group(clip: &Clip, groups: &mut BTreeMap<String, Vec<ColorClipContext>>) {
    if let MediaReference::External(reference) = &clip.media_reference {
        groups
            .entry(reference.target_url.clone())
            .or_default()
            .push(ColorClipContext {
                clip_id: assistant_clip_id(clip),
                camera_color_space: clip_extra_string(clip, "camera_color_space"),
                camera_lut: clip_extra_string(clip, "camera_lut"),
                show_lut: clip_extra_string(clip, "show_lut"),
                look_reference: clip_extra_string(clip, "look_reference"),
            });
    }
}

fn collect_picture_clip_sources(timeline: &Timeline) -> BTreeMap<String, String> {
    let mut sources = BTreeMap::new();
    collect_picture_clip_sources_from_stack(&timeline.tracks, &mut sources);
    sources
}

fn collect_picture_clip_sources_from_stack(stack: &Stack, sources: &mut BTreeMap<String, String>) {
    for child in &stack.children {
        match child {
            StackChild::Track(track) => {
                if track.kind == OtioTrackKind::Video {
                    collect_picture_clip_sources_from_track(track, sources);
                }
                for child in &track.children {
                    if let TrackChild::Stack(stack) = child {
                        collect_picture_clip_sources_from_stack(stack, sources);
                    }
                }
            }
            StackChild::Stack(stack) => collect_picture_clip_sources_from_stack(stack, sources),
            StackChild::Clip(clip) => collect_picture_clip_source(clip, sources),
            StackChild::Gap(_) => {}
        }
    }
}

fn collect_picture_clip_sources_from_track(track: &Track, sources: &mut BTreeMap<String, String>) {
    for child in &track.children {
        match child {
            TrackChild::Clip(clip) => collect_picture_clip_source(clip, sources),
            TrackChild::Stack(stack) => collect_picture_clip_sources_from_stack(stack, sources),
            TrackChild::Gap(_) | TrackChild::Transition(_) => {}
        }
    }
}

fn collect_picture_clip_source(clip: &Clip, sources: &mut BTreeMap<String, String>) {
    if let MediaReference::External(reference) = &clip.media_reference {
        sources.insert(assistant_clip_id(clip), reference.target_url.clone());
    }
}

fn color_source_asset_for_clip(
    clip_sources: &BTreeMap<String, String>,
    state: &ColorFinishingState,
    clip_id: &str,
) -> Option<String> {
    clip_sources.get(clip_id).cloned().or_else(|| {
        state
            .shot_groups
            .iter()
            .find(|group| group.clip_ids.iter().any(|id| id == clip_id))
            .map(|group| group.id.clone())
    })
}

fn color_lut_refs(state: &ColorFinishingState) -> Vec<String> {
    let mut refs = BTreeSet::new();
    for grade_stack in &state.grade_stacks {
        let (camera_lut, show_lut) = grade_stack_luts(grade_stack);
        if let Some(lut) = camera_lut {
            refs.insert(lut);
        }
        if let Some(lut) = show_lut {
            refs.insert(lut);
        }
    }
    refs.into_iter().collect()
}

fn grade_stack_clip_id(grade_stack: &GradeStack) -> String {
    grade_stack
        .id
        .strip_prefix("grade-")
        .unwrap_or(&grade_stack.id)
        .to_string()
}

fn grade_stack_luts(grade_stack: &GradeStack) -> (Option<String>, Option<String>) {
    let mut camera_lut = None;
    let mut show_lut = None;
    for stage in &grade_stack.stages {
        if camera_lut.is_none() {
            camera_lut = grade_stage_string_param(stage, "camera_lut");
        }
        if show_lut.is_none() {
            show_lut = grade_stage_string_param(stage, "show_lut");
        }
    }
    (camera_lut, show_lut)
}

fn grade_stage_string_param(stage: &GradeStage, key: &str) -> Option<String> {
    stage
        .params
        .get(key)
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn clip_extra_string(clip: &Clip, key: &str) -> Option<String> {
    clip.metadata
        .awidat
        .as_ref()
        .and_then(|metadata| metadata.extra.get(key))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn default_grade_stack_for_clip(
    clip: &ColorClipContext,
    input_color_space: &str,
    output_color_space: &str,
) -> GradeStack {
    let mut transform_params = HashMap::new();
    transform_params.insert(
        "input_color_space".to_string(),
        serde_json::Value::String(
            clip.camera_color_space
                .clone()
                .unwrap_or_else(|| input_color_space.to_string()),
        ),
    );
    transform_params.insert(
        "output_color_space".to_string(),
        serde_json::Value::String(output_color_space.to_string()),
    );
    if let Some(camera_lut) = &clip.camera_lut {
        transform_params.insert(
            "camera_lut".to_string(),
            serde_json::Value::String(camera_lut.clone()),
        );
    }
    if let Some(show_lut) = &clip.show_lut {
        transform_params.insert(
            "show_lut".to_string(),
            serde_json::Value::String(show_lut.clone()),
        );
    }

    GradeStack {
        id: format!("grade-{}", clip.clip_id),
        stages: vec![
            GradeStage {
                id: "color-space-transform".to_string(),
                kind: "color_space_transform".to_string(),
                params: transform_params,
            },
            GradeStage {
                id: "primary-balance".to_string(),
                kind: "primary_balance".to_string(),
                params: HashMap::new(),
            },
            GradeStage {
                id: "shot-match".to_string(),
                kind: "shot_match".to_string(),
                params: HashMap::new(),
            },
        ],
    }
}

/// Derive composition and tracking contracts for clips that need VFX work.
pub fn derive_vfx_pipeline_package(timeline: &Timeline) -> VfxPipelinePackage {
    let mut shots = Vec::new();
    collect_vfx_shots(&timeline.tracks, &mut shots);

    let composition_graphs = shots.iter().map(vfx_composition_graph).collect();
    let tracks = shots.iter().map(vfx_track_sidecar).collect();
    let masks = shots
        .iter()
        .filter(|shot| shot_requires_mask(shot))
        .map(vfx_mask_sidecar)
        .collect();
    let mattes = shots
        .iter()
        .filter(|shot| shot_requires_matte(shot))
        .map(vfx_matte_sidecar)
        .collect();
    let shot_manifests = shots.iter().map(vfx_shot_manifest).collect();

    VfxPipelinePackage {
        composition_graphs,
        tracking_package: TrackingPackage {
            tracks,
            masks,
            mattes,
        },
        shot_manifests,
    }
}

#[derive(Debug, Clone, PartialEq)]
struct VfxShot {
    clip_id: String,
    asset_id: String,
    effect_names: Vec<String>,
    vendor_shot_name: String,
    vendor: Option<String>,
    current_version: Option<String>,
    timeline_start_s: f64,
    timeline_end_s: f64,
    source_start_s: Option<f64>,
    source_end_s: Option<f64>,
    handle_in_s: Option<f64>,
    handle_out_s: Option<f64>,
}

fn collect_vfx_shots(stack: &Stack, shots: &mut Vec<VfxShot>) {
    collect_vfx_stack_shots(stack, 0.0, shots);
}

fn collect_vfx_stack_shots(stack: &Stack, base_start_s: f64, shots: &mut Vec<VfxShot>) {
    for child in &stack.children {
        match child {
            StackChild::Track(track) => {
                if track.kind == OtioTrackKind::Video {
                    collect_vfx_track_shots(track, base_start_s, shots);
                }
            }
            StackChild::Stack(stack) => collect_vfx_stack_shots(stack, base_start_s, shots),
            StackChild::Clip(clip) => collect_vfx_clip_shot(clip, base_start_s, shots),
            StackChild::Gap(_) => {}
        }
    }
}

fn collect_vfx_track_shots(track: &Track, base_start_s: f64, shots: &mut Vec<VfxShot>) {
    let mut cursor_s = base_start_s;
    for child in &track.children {
        match child {
            TrackChild::Clip(clip) => {
                collect_vfx_clip_shot(clip, cursor_s, shots);
                cursor_s += clip_duration_s(clip);
            }
            TrackChild::Stack(stack) => {
                collect_vfx_stack_shots(stack, cursor_s, shots);
                cursor_s += vfx_stack_duration_s(stack);
            }
            TrackChild::Gap(gap) => {
                cursor_s += gap.source_range.duration.to_seconds();
            }
            TrackChild::Transition(transition) => {
                cursor_s += transition.in_offset.to_seconds() + transition.out_offset.to_seconds();
            }
        }
    }
}

fn collect_vfx_clip_shot(clip: &Clip, timeline_start_s: f64, shots: &mut Vec<VfxShot>) {
    let effect_names = clip
        .effects
        .iter()
        .filter(|effect| effect_requires_vfx(&effect.effect_name))
        .map(|effect| effect.effect_name.clone())
        .collect::<Vec<_>>();
    if effect_names.is_empty() {
        return;
    }
    if let MediaReference::External(reference) = &clip.media_reference {
        let clip_id = assistant_clip_id(clip);
        let duration_s = clip_duration_s(clip);
        let source_start_s = clip
            .source_range
            .as_ref()
            .map(|range| range.start_time.to_seconds());
        let source_end_s = clip
            .source_range
            .as_ref()
            .map(|range| range.start_time.to_seconds() + range.duration.to_seconds());
        let source_available_start_s = reference
            .available_range
            .as_ref()
            .map(|range| range.start_time.to_seconds());
        let source_available_end_s = reference
            .available_range
            .as_ref()
            .map(|range| range.start_time.to_seconds() + range.duration.to_seconds());
        shots.push(VfxShot {
            vendor_shot_name: clip_extra_string(clip, "vfx_shot_name")
                .unwrap_or_else(|| format!("VFX_{}", slug_for_review_id(&clip_id).to_uppercase())),
            vendor: clip_extra_string(clip, "vfx_vendor"),
            current_version: clip_extra_string(clip, "vfx_version"),
            clip_id,
            asset_id: reference.target_url.clone(),
            effect_names,
            timeline_start_s,
            timeline_end_s: timeline_start_s + duration_s,
            source_start_s,
            source_end_s,
            handle_in_s: match (source_available_start_s, source_start_s) {
                (Some(available_start), Some(source_start)) => {
                    Some((source_start - available_start).max(0.0))
                }
                _ => None,
            },
            handle_out_s: match (source_available_end_s, source_end_s) {
                (Some(available_end), Some(source_end)) => {
                    Some((available_end - source_end).max(0.0))
                }
                _ => None,
            },
        });
    }
}

fn vfx_stack_duration_s(stack: &Stack) -> f64 {
    stack
        .children
        .iter()
        .map(|child| match child {
            StackChild::Track(track) => vfx_track_duration_s(track),
            StackChild::Stack(stack) => vfx_stack_duration_s(stack),
            StackChild::Clip(clip) => clip_duration_s(clip),
            StackChild::Gap(gap) => gap.source_range.duration.to_seconds(),
        })
        .fold(0.0_f64, f64::max)
}

fn vfx_track_duration_s(track: &Track) -> f64 {
    track
        .children
        .iter()
        .map(|child| match child {
            TrackChild::Clip(clip) => clip_duration_s(clip),
            TrackChild::Stack(stack) => vfx_stack_duration_s(stack),
            TrackChild::Gap(gap) => gap.source_range.duration.to_seconds(),
            TrackChild::Transition(transition) => {
                transition.in_offset.to_seconds() + transition.out_offset.to_seconds()
            }
        })
        .sum()
}

fn effect_requires_vfx(effect_name: &str) -> bool {
    let normalized = effect_name.to_ascii_lowercase();
    contains_any(
        &normalized,
        &[
            "vfx", "screen", "replace", "roto", "mask", "matte", "track", "composit", "remove",
            "paint", "key", "green",
        ],
    )
}

fn shot_requires_mask(shot: &VfxShot) -> bool {
    shot.effect_names
        .iter()
        .any(|effect| effect_requires_mask(effect))
}

fn effect_requires_mask(effect_name: &str) -> bool {
    let normalized = effect_name.to_ascii_lowercase();
    contains_any(&normalized, &["roto", "mask", "paint", "remove"])
}

fn shot_requires_matte(shot: &VfxShot) -> bool {
    shot.effect_names
        .iter()
        .any(|effect| effect_requires_matte(effect))
}

fn effect_requires_matte(effect_name: &str) -> bool {
    let normalized = effect_name.to_ascii_lowercase();
    contains_any(&normalized, &["key", "matte", "green", "screen"])
}

fn vfx_shot_manifest(shot: &VfxShot) -> VfxShotManifest {
    let version = shot
        .current_version
        .clone()
        .unwrap_or_else(|| "v001".to_string());
    VfxShotManifest {
        shot_id: format!("vfx-{}", shot.clip_id),
        clip_id: shot.clip_id.clone(),
        vendor_shot_name: shot.vendor_shot_name.clone(),
        vendor: shot.vendor.clone(),
        source_asset: shot.asset_id.clone(),
        timeline_start_s: shot.timeline_start_s,
        timeline_end_s: shot.timeline_end_s,
        source_start_s: shot.source_start_s,
        source_end_s: shot.source_end_s,
        handle_in_s: shot.handle_in_s,
        handle_out_s: shot.handle_out_s,
        pull_ref: format!("vfx/pulls/{}.mov", shot.vendor_shot_name),
        review_ref: format!("vfx/review/{}_{}.mov", shot.vendor_shot_name, version),
        current_version: shot.current_version.clone(),
        effect_names: shot.effect_names.clone(),
    }
}

fn vfx_composition_graph(shot: &VfxShot) -> CompositionGraph {
    let mut media_params = HashMap::new();
    media_params.insert(
        "asset_id".to_string(),
        serde_json::Value::String(shot.asset_id.clone()),
    );
    media_params.insert(
        "clip_id".to_string(),
        serde_json::Value::String(shot.clip_id.clone()),
    );
    let mut tracker_params = HashMap::new();
    tracker_params.insert(
        "track_id".to_string(),
        serde_json::Value::String(format!("track-{}", shot.clip_id)),
    );
    tracker_params.insert(
        "effect_names".to_string(),
        serde_json::Value::Array(
            shot.effect_names
                .iter()
                .map(|effect| serde_json::Value::String(effect.clone()))
                .collect(),
        ),
    );
    let mut nodes = vec![
        CompositionNode {
            id: "media".to_string(),
            node_type: CompositionNodeType::MediaInput,
            params: media_params,
        },
        CompositionNode {
            id: "tracker".to_string(),
            node_type: CompositionNodeType::TrackerBind,
            params: tracker_params,
        },
    ];
    let mut edges = vec![CompositionEdge {
        from: "media".to_string(),
        to: "tracker".to_string(),
        input: None,
    }];
    let mut tail = "tracker".to_string();
    if shot_requires_mask(shot) {
        let mut mask_params = HashMap::new();
        mask_params.insert(
            "mask_id".to_string(),
            serde_json::Value::String(format!("mask-{}", shot.clip_id)),
        );
        nodes.push(CompositionNode {
            id: "mask".to_string(),
            node_type: CompositionNodeType::Mask,
            params: mask_params,
        });
        edges.push(CompositionEdge {
            from: tail,
            to: "mask".to_string(),
            input: Some("source".to_string()),
        });
        tail = "mask".to_string();
    }
    if shot_requires_matte(shot) {
        let mut matte_params = HashMap::new();
        matte_params.insert(
            "matte_id".to_string(),
            serde_json::Value::String(format!("matte-{}", shot.clip_id)),
        );
        nodes.push(CompositionNode {
            id: "matte".to_string(),
            node_type: CompositionNodeType::Matte,
            params: matte_params,
        });
        edges.push(CompositionEdge {
            from: tail,
            to: "matte".to_string(),
            input: Some("mask".to_string()),
        });
        tail = "matte".to_string();
    }
    nodes.push(CompositionNode {
        id: "output".to_string(),
        node_type: CompositionNodeType::Output,
        params: HashMap::new(),
    });
    edges.push(CompositionEdge {
        from: tail,
        to: "output".to_string(),
        input: Some("source".to_string()),
    });

    CompositionGraph {
        id: format!("vfx-{}", shot.clip_id),
        nodes,
        edges,
        output_node_id: Some("output".to_string()),
    }
}

fn vfx_track_sidecar(shot: &VfxShot) -> TrackSidecar {
    TrackSidecar {
        id: format!("track-{}", shot.clip_id),
        asset_id: shot.asset_id.clone(),
        kind: VfxTrackKind::Planar,
        samples: vec![TrackSample {
            frame: 0,
            points: vec![[0.25, 0.25], [0.75, 0.25], [0.75, 0.75], [0.25, 0.75]],
            confidence: None,
        }],
        confidence: None,
        ..TrackSidecar::default()
    }
}

fn vfx_mask_sidecar(shot: &VfxShot) -> MaskSidecar {
    MaskSidecar {
        id: format!("mask-{}", shot.clip_id),
        track_id: Some(format!("track-{}", shot.clip_id)),
        attached_clip_id: Some(shot.clip_id.clone()),
        operation: MaskOperation::Add,
        keyframes: vec![MaskKeyframe {
            time_s: 0.0,
            points: vec![[0.2, 0.2], [0.8, 0.2], [0.8, 0.8], [0.2, 0.8]],
            feather: 0.0,
            opacity: 1.0,
        }],
    }
}

fn vfx_matte_sidecar(shot: &VfxShot) -> MatteSidecar {
    MatteSidecar {
        id: format!("matte-{}", shot.clip_id),
        alpha_source: shot.asset_id.clone(),
        confidence: None,
        review_thumbnails: vec![format!("vfx-{}", shot.clip_id)],
    }
}

/// Derive a secure review bundle for pending professional proposals.
pub fn derive_secure_review_package(
    metadata: &AwidatTimelineMetadata,
    reviewer: impl Into<String>,
    cut_id: impl Into<String>,
) -> SecureReviewPackage {
    let reviewer = normalize_review_field(reviewer.into(), "reviewer");
    let cut_id = normalize_review_field(cut_id.into(), "cut");
    let id = format!(
        "review-{}-{}",
        slug_for_review_id(&cut_id),
        slug_for_review_id(&reviewer)
    );
    let proposals = metadata
        .proposal_packages
        .iter()
        .filter(|package| package.status == ReviewStatus::Proposed)
        .cloned()
        .collect::<Vec<_>>();
    let artifact_refs = collect_review_artifact_refs(metadata, &proposals);
    let review_versions = derive_review_versions(&id, &cut_id, &artifact_refs, &proposals);
    let comments = derive_review_comments(&reviewer, &proposals);
    let watermark = format!("Awidat review / {cut_id} / {reviewer} / {id}");

    SecureReviewPackage {
        id,
        proposals,
        artifact_refs,
        review_versions,
        comments,
        security: ReviewSecurityPolicy {
            allowed_reviewers: vec![reviewer],
            require_auth: true,
            allow_download: false,
            watermark,
        },
    }
}

fn collect_review_artifact_refs(
    metadata: &AwidatTimelineMetadata,
    proposals: &[awidat_proto::professional::ProposalPackage],
) -> Vec<String> {
    let mut refs = BTreeSet::new();
    for proposal in proposals {
        for evidence in &proposal.evidence {
            refs.extend(review_comment_artifact_refs(&evidence.refs));
        }
        refs.extend(proposal.rollback_refs.iter().filter_map(|reference| {
            let trimmed = reference.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        }));
    }
    for report in &metadata.preflight_reports {
        let report_id = report.id.trim();
        if !report_id.is_empty() {
            refs.insert(format!("preflight:{report_id}"));
        }
    }
    for manifest in &metadata.package_manifests {
        for artifact in &manifest.artifacts {
            let trimmed = artifact.trim();
            if !trimmed.is_empty() {
                refs.insert(trimmed.to_string());
            }
        }
        for report in &manifest.validation_reports {
            let trimmed = report.trim();
            if !trimmed.is_empty() {
                refs.insert(trimmed.to_string());
            }
        }
    }
    refs.into_iter().collect()
}

fn derive_review_versions(
    package_id: &str,
    cut_id: &str,
    artifact_refs: &[String],
    proposals: &[awidat_proto::professional::ProposalPackage],
) -> Vec<ReviewVersion> {
    if artifact_refs.is_empty() && proposals.is_empty() {
        return Vec::new();
    }
    let rollback_refs = proposals
        .iter()
        .flat_map(|proposal| proposal.rollback_refs.iter())
        .filter_map(|reference| non_empty_trimmed(reference))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    vec![ReviewVersion {
        id: format!("{package_id}-v001"),
        package_id: package_id.to_string(),
        cut_id: cut_id.to_string(),
        artifact_refs: artifact_refs.to_vec(),
        rollback_refs,
    }]
}

fn derive_review_comments(
    reviewer: &str,
    proposals: &[awidat_proto::professional::ProposalPackage],
) -> Vec<ReviewComment> {
    let mut comments = Vec::new();
    for proposal in proposals {
        for (index, evidence) in proposal.evidence.iter().enumerate() {
            let body = evidence
                .reason
                .as_deref()
                .map(str::trim)
                .filter(|reason| !reason.is_empty())
                .unwrap_or(proposal.summary.trim());
            if body.is_empty() {
                continue;
            }
            let artifact_refs = review_comment_artifact_refs(&evidence.refs);
            comments.push(ReviewComment {
                id: format!(
                    "comment-{}-{}-{index}",
                    slug_for_review_id(&proposal.id),
                    slug_for_review_id(&evidence.source)
                ),
                proposal_id: proposal.id.clone(),
                reviewer: reviewer.to_string(),
                body: body.to_string(),
                frame: review_comment_frame(&evidence.refs),
                time_s: review_comment_time_s(&evidence.refs),
                artifact_refs,
            });
        }
    }
    comments
}

fn review_comment_artifact_refs(refs: &[String]) -> Vec<String> {
    refs.iter()
        .filter_map(|reference| {
            let trimmed = reference.trim();
            if trimmed.is_empty()
                || trimmed.strip_prefix("frame:").is_some()
                || trimmed.strip_prefix("time_s:").is_some()
                || trimmed.strip_prefix("time:").is_some()
            {
                None
            } else {
                Some(trimmed.to_string())
            }
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn review_comment_frame(refs: &[String]) -> Option<u64> {
    refs.iter().find_map(|reference| {
        reference
            .trim()
            .strip_prefix("frame:")
            .and_then(|frame| frame.trim().parse::<u64>().ok())
    })
}

fn review_comment_time_s(refs: &[String]) -> Option<f64> {
    refs.iter().find_map(|reference| {
        let trimmed = reference.trim();
        trimmed
            .strip_prefix("time_s:")
            .or_else(|| trimmed.strip_prefix("time:"))
            .and_then(|time| time.trim().parse::<f64>().ok())
            .filter(|time| time.is_finite() && *time >= 0.0)
    })
}

fn non_empty_trimmed(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// Derive final delivery, QC, and archive requirements from current manifests.
pub fn derive_delivery_master_package(
    metadata: &AwidatTimelineMetadata,
    audio: &AudioPostPackage,
) -> DeliveryMasterPackage {
    let profile = metadata
        .delivery_profiles
        .first()
        .cloned()
        .unwrap_or_else(DeliveryProfile::youtube_1080p);
    let package_artifacts = delivery_manifest_artifacts(metadata);
    let mut deliverables = vec![
        delivery_artifact(
            &profile.id,
            "prores_master",
            format!("deliveries/{}/final-master.mov", profile.id),
            true,
            &package_artifacts,
        ),
        delivery_artifact(
            &profile.id,
            "dcp",
            format!("deliveries/{}/final-master.dcp", profile.id),
            true,
            &package_artifacts,
        ),
        delivery_artifact(
            &profile.id,
            "imf",
            format!("deliveries/{}/final-master.imf", profile.id),
            true,
            &package_artifacts,
        ),
        delivery_artifact(
            &profile.id,
            "textless_master",
            format!("deliveries/{}/textless.mov", profile.id),
            true,
            &package_artifacts,
        ),
        delivery_artifact(
            &profile.id,
            "caption",
            delivery_caption_path(&profile.id, &package_artifacts),
            true,
            &package_artifacts,
        ),
        delivery_artifact(
            &profile.id,
            "me_mix",
            "audio/stems/me.wav".to_string(),
            true,
            &package_artifacts,
        ),
    ];
    let mut stems = audio.stem_deliverables.clone();
    stems.sort_by(|a, b| a.path.cmp(&b.path));
    for stem in stems {
        deliverables.push(delivery_artifact(
            &profile.id,
            "audio_stem",
            stem.path,
            true,
            &package_artifacts,
        ));
    }

    DeliveryMasterPackage {
        profile_id: profile.id.clone(),
        deliverables,
        qc_reports: delivery_qc_reports(metadata, &profile.id),
        archive_refs: delivery_archive_refs(&profile.id),
        blocked: delivery_blocked(&metadata.preflight_reports, &profile.id),
    }
}

fn delivery_manifest_artifacts(metadata: &AwidatTimelineMetadata) -> BTreeSet<String> {
    metadata
        .package_manifests
        .iter()
        .flat_map(|manifest| manifest.artifacts.iter())
        .filter_map(|artifact| non_empty_trimmed(artifact))
        .collect()
}

fn delivery_artifact(
    profile_id: &str,
    kind: &str,
    path: String,
    required: bool,
    package_artifacts: &BTreeSet<String>,
) -> DeliveryArtifact {
    DeliveryArtifact {
        id: format!("delivery-{}-{}", slug_for_review_id(profile_id), kind),
        kind: kind.to_string(),
        present: package_artifacts.contains(&path),
        path,
        required,
    }
}

fn delivery_caption_path(profile_id: &str, package_artifacts: &BTreeSet<String>) -> String {
    package_artifacts
        .iter()
        .find(|artifact| {
            artifact.starts_with(&format!("deliveries/{profile_id}/subtitles/"))
                || artifact.ends_with(".srt")
                || artifact.ends_with(".vtt")
                || artifact.ends_with(".itt")
        })
        .cloned()
        .unwrap_or_else(|| format!("deliveries/{profile_id}/subtitles/en.srt"))
}

fn delivery_qc_reports(metadata: &AwidatTimelineMetadata, profile_id: &str) -> Vec<String> {
    let mut reports = BTreeSet::new();
    for report in &metadata.preflight_reports {
        if report.profile_id == profile_id || metadata.delivery_profiles.is_empty() {
            reports.insert(format!("preflight:{}", report.id));
        }
    }
    for manifest in &metadata.package_manifests {
        reports.extend(
            manifest
                .validation_reports
                .iter()
                .filter_map(|report| non_empty_trimmed(report)),
        );
    }
    reports.into_iter().collect()
}

fn delivery_archive_refs(profile_id: &str) -> Vec<String> {
    vec![
        format!("archive/{profile_id}/delivery-manifest.json"),
        format!("archive/{profile_id}/project.otio"),
        format!("archive/{profile_id}/source-manifest.json"),
    ]
}

fn delivery_blocked(reports: &[PreflightReport], profile_id: &str) -> bool {
    reports.iter().any(|report| {
        (report.profile_id == profile_id || profile_id == "youtube_1080p")
            && report
                .findings
                .iter()
                .any(|finding| finding.severity == FindingSeverity::Error)
    })
}

fn normalize_review_field(value: String, fallback: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        fallback.to_string()
    } else {
        trimmed.to_string()
    }
}

fn slug_for_review_id(value: &str) -> String {
    let mut slug = String::new();
    let mut previous_dash = false;
    for character in value.chars().flat_map(char::to_lowercase) {
        if character.is_ascii_alphanumeric() {
            slug.push(character);
            previous_dash = false;
        } else if !previous_dash {
            slug.push('-');
            previous_dash = true;
        }
    }
    let slug = slug.trim_matches('-').to_string();
    if slug.is_empty() {
        "review".to_string()
    } else {
        slug
    }
}

/// Derive a timeline scale profile for large-project UI and agent-windowing decisions.
pub fn derive_timeline_scale_profile(
    timeline: &Timeline,
    max_visible_rows: usize,
    recommended_window_s: f64,
) -> TimelineScaleProfile {
    let mut metrics = TimelineScaleMetrics::default();
    let mut max_track_duration_s = 0.0;
    for child in &timeline.tracks.children {
        match child {
            StackChild::Track(track) => {
                metrics.track_count += 1;
                let track_duration_s = collect_track_scale_metrics(track, &mut metrics);
                if track_duration_s > max_track_duration_s {
                    max_track_duration_s = track_duration_s;
                }
            }
            StackChild::Stack(stack) => {
                metrics.item_count += 1;
                let stack_duration_s = collect_stack_scale_metrics(stack, &mut metrics);
                if stack_duration_s > max_track_duration_s {
                    max_track_duration_s = stack_duration_s;
                }
            }
            StackChild::Clip(clip) => {
                metrics.item_count += 1;
                metrics.clip_count += 1;
                max_track_duration_s = max_track_duration_s.max(clip_duration_s(clip));
            }
            StackChild::Gap(gap) => {
                metrics.item_count += 1;
                max_track_duration_s =
                    max_track_duration_s.max(gap.source_range.duration.to_seconds());
            }
        }
    }

    let max_visible_rows = max_visible_rows.max(1);
    let recommended_window_s = if recommended_window_s.is_finite() && recommended_window_s > 0.0 {
        recommended_window_s
    } else {
        60.0
    };
    let row_windows =
        timeline_row_windows(metrics.track_count, metrics.item_count, max_visible_rows);
    let time_windows = timeline_time_windows(max_track_duration_s, recommended_window_s);
    let agent_window_refs = timeline_agent_window_refs(&row_windows, &time_windows);
    let requires_virtualization =
        metrics.item_count > max_visible_rows || row_windows.len() > 1 || time_windows.len() > 1;
    TimelineScaleProfile {
        track_count: metrics.track_count,
        clip_count: metrics.clip_count,
        item_count: metrics.item_count,
        total_duration_s: max_track_duration_s,
        max_visible_rows,
        recommended_window_s,
        requires_virtualization,
        max_items_per_window: max_visible_rows,
        row_windows: row_windows.clone(),
        time_windows: time_windows.clone(),
        agent_window_refs,
        guidance: timeline_scale_guidance(
            metrics.item_count,
            max_visible_rows,
            recommended_window_s,
            row_windows.len(),
            time_windows.len(),
        ),
    }
}

#[derive(Debug, Clone, Default)]
struct TimelineScaleMetrics {
    track_count: usize,
    clip_count: usize,
    item_count: usize,
}

fn collect_stack_scale_metrics(stack: &Stack, metrics: &mut TimelineScaleMetrics) -> f64 {
    let mut duration_s: f64 = 0.0;
    for child in &stack.children {
        match child {
            StackChild::Track(track) => {
                metrics.track_count += 1;
                duration_s = duration_s.max(collect_track_scale_metrics(track, metrics));
            }
            StackChild::Stack(stack) => {
                metrics.item_count += 1;
                duration_s = duration_s.max(collect_stack_scale_metrics(stack, metrics));
            }
            StackChild::Clip(clip) => {
                metrics.item_count += 1;
                metrics.clip_count += 1;
                duration_s += clip_duration_s(clip);
            }
            StackChild::Gap(gap) => {
                metrics.item_count += 1;
                duration_s += gap.source_range.duration.to_seconds();
            }
        }
    }
    duration_s
}

fn collect_track_scale_metrics(track: &Track, metrics: &mut TimelineScaleMetrics) -> f64 {
    let mut duration_s = 0.0;
    for child in &track.children {
        metrics.item_count += 1;
        match child {
            TrackChild::Clip(clip) => {
                metrics.clip_count += 1;
                duration_s += clip_duration_s(clip);
            }
            TrackChild::Gap(gap) => {
                duration_s += gap.source_range.duration.to_seconds();
            }
            TrackChild::Transition(transition) => {
                duration_s +=
                    transition.in_offset.to_seconds() + transition.out_offset.to_seconds();
            }
            TrackChild::Stack(stack) => {
                duration_s += collect_stack_scale_metrics(stack, metrics);
            }
        }
    }
    duration_s
}

fn clip_duration_s(clip: &Clip) -> f64 {
    clip.source_range
        .as_ref()
        .map(|range| range.duration.to_seconds())
        .unwrap_or(0.0)
}

fn timeline_row_windows(
    track_count: usize,
    item_count: usize,
    max_visible_rows: usize,
) -> Vec<TimelineRowWindow> {
    let row_count = if track_count > 0 {
        track_count
    } else if item_count > 0 {
        1
    } else {
        0
    };
    let mut windows = Vec::new();
    let mut start_row = 0;
    while start_row < row_count {
        let end_row = (start_row + max_visible_rows).min(row_count);
        windows.push(TimelineRowWindow { start_row, end_row });
        start_row = end_row;
    }
    windows
}

fn timeline_time_windows(
    total_duration_s: f64,
    recommended_window_s: f64,
) -> Vec<TimelineTimeWindow> {
    if !total_duration_s.is_finite() || total_duration_s <= 0.0 {
        return Vec::new();
    }
    let mut windows = Vec::new();
    let mut start_s = 0.0;
    while start_s < total_duration_s {
        let end_s = (start_s + recommended_window_s).min(total_duration_s);
        windows.push(TimelineTimeWindow { start_s, end_s });
        start_s = end_s;
    }
    windows
}

fn timeline_agent_window_refs(
    row_windows: &[TimelineRowWindow],
    time_windows: &[TimelineTimeWindow],
) -> Vec<String> {
    let mut refs = Vec::new();
    for row in row_windows {
        let row_end_inclusive = row.end_row.saturating_sub(1);
        if time_windows.is_empty() {
            refs.push(format!(
                "timeline-window:r{}-{}",
                row.start_row, row_end_inclusive
            ));
            continue;
        }
        for time in time_windows {
            refs.push(format!(
                "timeline-window:r{}-{}:t{:.3}-{:.3}",
                row.start_row, row_end_inclusive, time.start_s, time.end_s
            ));
        }
    }
    refs
}

fn timeline_scale_guidance(
    item_count: usize,
    max_visible_rows: usize,
    recommended_window_s: f64,
    row_window_count: usize,
    time_window_count: usize,
) -> Vec<String> {
    let mut guidance = Vec::new();
    if item_count > max_visible_rows {
        guidance.push(format!(
            "Use virtualized timeline rows; render at most {max_visible_rows} visible items at once."
        ));
        guidance.push(format!(
            "Use windowed timeline queries around {:.3}s spans for agent and review surfaces.",
            recommended_window_s
        ));
        guidance.push(format!(
            "Plan timeline work across {row_window_count} row windows and {time_window_count} time windows."
        ));
    } else {
        guidance.push("Timeline is small enough for direct row rendering.".to_string());
    }
    guidance
}

/// Derive the main professional department surfaces for a timeline.
pub fn derive_professional_pipeline_snapshot(
    project_root: &Path,
    timeline: &Timeline,
    reviewer: impl Into<String>,
    cut_id: impl Into<String>,
) -> ProfessionalPipelineSnapshot {
    let mut asset_catalog = derive_assistant_asset_catalog(project_root, timeline);
    let media_operations = derive_media_operations_state(project_root, &asset_catalog);
    let audio_finishing = derive_audio_finishing_state(timeline);
    let color_finishing =
        derive_color_finishing_state(timeline, "camera_native", "Rec.709 Gamma 2.4");
    let vfx = derive_vfx_pipeline_package(timeline);
    let scale_profile = derive_timeline_scale_profile(timeline, 200, 60.0);
    let assistant_workflow = derive_assistant_workflow_state(timeline);
    annotate_catalog_select_usage(
        &mut asset_catalog,
        &assistant_workflow.source_review.selects,
    );

    let mut metadata = timeline.metadata.awidat.clone().unwrap_or_default();
    metadata.asset_catalog = Some(asset_catalog.clone());
    merge_assistant_source_review(&mut metadata, assistant_workflow.source_review.clone());
    metadata.audio_finishing = Some(audio_finishing.clone());
    metadata.color_finishing = Some(color_finishing.clone());
    metadata.composition_graphs = vfx.composition_graphs.clone();
    metadata.tracking_package = Some(vfx.tracking_package.clone());
    let audio_post = derive_audio_post_package(timeline, &audio_finishing);
    let delivery = derive_delivery_master_package(&metadata, &audio_post);
    let review = derive_secure_review_package(&metadata, reviewer, cut_id);
    let lens_snapshots = build_workflow_lens_snapshots(&metadata);

    ProfessionalPipelineSnapshot {
        asset_catalog,
        media_operations,
        assistant_workflow,
        audio_finishing,
        color_finishing,
        vfx,
        review,
        delivery,
        scale_profile,
        lens_snapshots,
    }
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

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::fs;
    use std::path::{Path, PathBuf};

    use awidat_proto::awidat_meta::{AwidatClipMetadata, AwidatMarkerMetadata};
    use awidat_proto::otio::{
        Clip, Effect, ExternalReference, Marker, MediaReference, Stack, StackChild, Timeline,
        Track, TrackChild, TrackKind,
    };
    use awidat_proto::otio::{RationalTime, TimeRange};
    use awidat_proto::professional::{
        AssetRole, CompositionNodeType, DeliveryProfile, EvidenceTrace, FindingSeverity,
        MaskOperation, PackageManifest, PreflightCheckKind, PreflightFinding, PreflightReport,
        ProposalPackage, ReadinessState, ReviewStatus,
    };

    use super::*;

    #[test]
    fn assistant_asset_catalog_groups_sources_and_tracks_timeline_usage() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("raw")).unwrap();
        fs::write(dir.path().join("raw/cam-a.mov"), b"video-source").unwrap();
        fs::write(dir.path().join("raw/dialogue.wav"), b"audio-source").unwrap();
        let camera_source = dir.path().join("raw/cam-a.mov");
        let camera_proxy = test_proxy_path_for(dir.path(), &camera_source);
        fs::create_dir_all(camera_proxy.parent().unwrap()).unwrap();
        fs::write(camera_proxy, b"proxy-source").unwrap();
        let camera_sidecar = dir.path().join("index/whisper/raw/cam-a.mov.json");
        fs::create_dir_all(camera_sidecar.parent().unwrap()).unwrap();
        fs::write(camera_sidecar, "{}").unwrap();

        let mut timeline = Timeline::empty("assistant catalog");
        let awidat = timeline.metadata.awidat.as_mut().unwrap();
        awidat.source_assets = vec![
            "raw/cam-a.mov".to_string(),
            "raw/dialogue.wav".to_string(),
            "raw/missing.mov".to_string(),
        ];

        let mut video = Track::empty("V1", TrackKind::Video);
        video.children.push(TrackChild::Clip(timeline_clip(
            "Camera A",
            "raw/cam-a.mov",
            "c-cam-a",
        )));
        let mut audio = Track::empty("A1", TrackKind::Audio);
        audio.children.push(TrackChild::Clip(timeline_clip(
            "Dialogue",
            "raw/dialogue.wav",
            "c-dialogue",
        )));
        timeline.tracks.children = vec![StackChild::Track(video), StackChild::Track(audio)];

        let catalog = derive_assistant_asset_catalog(dir.path(), &timeline);

        assert!(catalog.validate().is_empty());
        assert!(catalog.bins.iter().any(|bin| bin.id == "video"));
        assert!(catalog.bins.iter().any(|bin| bin.id == "audio"));

        let camera = catalog
            .assets
            .iter()
            .find(|asset| asset.path == "raw/cam-a.mov")
            .unwrap();
        assert_eq!(camera.role, AssetRole::Video);
        assert_eq!(camera.bin_id.as_deref(), Some("video"));
        assert_eq!(camera.readiness.online, ReadinessState::Ready);
        assert_eq!(camera.readiness.proxy, ReadinessState::Ready);
        assert_eq!(camera.readiness.index, ReadinessState::Ready);
        assert_eq!(camera.usage.clip_ids, vec!["c-cam-a"]);
        assert!(camera.tags.contains(&"source".to_string()));
        assert!(camera.tags.contains(&"timeline".to_string()));
        assert!(camera.provenance.as_ref().unwrap().checksum.is_some());

        let dialogue = catalog
            .assets
            .iter()
            .find(|asset| asset.path == "raw/dialogue.wav")
            .unwrap();
        assert_eq!(dialogue.role, AssetRole::Audio);
        assert_eq!(dialogue.bin_id.as_deref(), Some("audio"));
        assert_eq!(dialogue.readiness.proxy, ReadinessState::Pending);
        assert_eq!(dialogue.readiness.index, ReadinessState::Pending);
        assert_eq!(dialogue.usage.clip_ids, vec!["c-dialogue"]);

        let missing = catalog
            .assets
            .iter()
            .find(|asset| asset.path == "raw/missing.mov")
            .unwrap();
        assert_eq!(missing.readiness.online, ReadinessState::Blocked);
        assert_eq!(missing.readiness.proxy, ReadinessState::Blocked);
        assert_eq!(missing.readiness.index, ReadinessState::Pending);
        assert!(missing.usage.clip_ids.is_empty());
        assert!(missing.provenance.as_ref().unwrap().checksum.is_none());
    }

    #[test]
    fn media_operations_state_derives_proxy_index_relink_backup_and_archive_actions() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("raw")).unwrap();
        fs::write(dir.path().join("raw/cam-a.mov"), b"video-source").unwrap();
        fs::write(dir.path().join("raw/dialogue.wav"), b"audio-source").unwrap();
        let camera_source = dir.path().join("raw/cam-a.mov");
        let camera_proxy = test_proxy_path_for(dir.path(), &camera_source);
        fs::create_dir_all(camera_proxy.parent().unwrap()).unwrap();
        fs::write(camera_proxy, b"proxy-source").unwrap();
        let camera_sidecar = dir.path().join("index/whisper/raw/cam-a.mov.json");
        fs::create_dir_all(camera_sidecar.parent().unwrap()).unwrap();
        fs::write(camera_sidecar, "{}").unwrap();

        let mut timeline = Timeline::empty("media operations");
        timeline.metadata.awidat.as_mut().unwrap().source_assets = vec![
            "raw/cam-a.mov".to_string(),
            "raw/dialogue.wav".to_string(),
            "raw/missing.mov".to_string(),
        ];
        let catalog = derive_assistant_asset_catalog(dir.path(), &timeline);

        let state = derive_media_operations_state(dir.path(), &catalog);

        assert_eq!(state.asset_count, 3);
        assert_eq!(state.online_count, 2);
        assert_eq!(state.offline_count, 1);
        assert!(
            state
                .archive_refs
                .contains(&"archive/media/source-manifest.json".to_string())
        );
        assert!(
            state
                .archive_refs
                .contains(&"archive/media/checksums.sha256".to_string())
        );
        assert!(
            state
                .archive_refs
                .contains(&"archive/media/relink-map.json".to_string())
        );
        assert_media_action(
            &state,
            "raw/cam-a.mov",
            "verify_checksum",
            Some("raw/cam-a.mov"),
            Some("archive/media/checksums.sha256"),
            false,
        );
        assert_media_action(
            &state,
            "raw/dialogue.wav",
            "generate_proxy",
            Some("raw/dialogue.wav"),
            Some(".awidat/proxies/dialogue"),
            false,
        );
        assert_media_action(
            &state,
            "raw/dialogue.wav",
            "run_indexing",
            Some("raw/dialogue.wav"),
            Some("index/"),
            false,
        );
        assert_media_action(
            &state,
            "raw/dialogue.wav",
            "backup_source",
            Some("raw/dialogue.wav"),
            Some("backups/raw/dialogue.wav"),
            false,
        );
        assert_media_action(
            &state,
            "raw/missing.mov",
            "relink_offline_media",
            Some("raw/missing.mov"),
            Some("archive/media/relink-map.json"),
            true,
        );
    }

    #[test]
    fn audio_finishing_state_derives_role_buses_from_audio_tracks() {
        let mut timeline = Timeline::empty("audio finish");
        let mut dialogue_track = audio_track("A1 Dialogue Boom");
        let mut dialogue_clip = timeline_clip("Line 1", "raw/dialogue.wav", "c-dialogue");
        dialogue_clip.source_range = Some(TimeRange::new(
            RationalTime::zero(48_000.0),
            RationalTime::new(96_000.0, 48_000.0),
        ));
        let mut adr_marker = Marker::new(
            "ADR line 1",
            TimeRange::new(
                RationalTime::new(24_000.0, 48_000.0),
                RationalTime::new(48_000.0, 48_000.0),
            ),
        );
        adr_marker.comment = Some("replace noisy production line".into());
        adr_marker.metadata.awidat = Some(AwidatMarkerMetadata {
            category: Some("adr".into()),
            note: Some("boom rustle under dialogue".into()),
            ..AwidatMarkerMetadata::default()
        });
        dialogue_clip.markers.push(adr_marker);
        dialogue_track
            .children
            .push(TrackChild::Clip(dialogue_clip));
        let mut sfx_track = audio_track("SFX Impacts");
        let mut sfx_clip = timeline_clip("Door slam", "raw/door.wav", "c-door");
        sfx_clip.source_range = Some(TimeRange::new(
            RationalTime::zero(48_000.0),
            RationalTime::new(48_000.0, 48_000.0),
        ));
        let mut foley_marker = Marker::new(
            "Foley door",
            TimeRange::new(
                RationalTime::zero(48_000.0),
                RationalTime::new(24_000.0, 48_000.0),
            ),
        );
        foley_marker.comment = Some("sweeten door contact".into());
        foley_marker.metadata.awidat = Some(AwidatMarkerMetadata {
            category: Some("foley".into()),
            note: Some("add handle rattle".into()),
            ..AwidatMarkerMetadata::default()
        });
        sfx_clip.markers.push(foley_marker);
        sfx_track.children.push(TrackChild::Clip(sfx_clip));
        timeline.tracks.children = vec![
            StackChild::Track(dialogue_track),
            StackChild::Track(audio_track("MX Music")),
            StackChild::Track(sfx_track),
            StackChild::Track(Track::empty("V1 Picture", TrackKind::Video)),
        ];
        timeline.metadata.awidat.as_mut().unwrap().extra.insert(
            "audio_measurements".to_string(),
            serde_json::json!({
                "master": {
                    "integrated_lufs": -14.2,
                    "true_peak_db": -1.1,
                    "noise_floor_db": -62.0,
                    "clipping": false
                },
                "dialogue": {
                    "integrated_lufs": -18.0,
                    "true_peak_db": -3.4,
                    "noise_floor_db": -58.5,
                    "clipping": true
                }
            }),
        );

        let state = derive_audio_finishing_state(&timeline);

        let dialogue = state.buses.iter().find(|bus| bus.id == "dialogue").unwrap();
        assert_eq!(dialogue.inputs, vec!["A1 Dialogue Boom"]);
        let music = state.buses.iter().find(|bus| bus.id == "music").unwrap();
        assert_eq!(music.inputs, vec!["MX Music"]);
        let sfx = state.buses.iter().find(|bus| bus.id == "sfx").unwrap();
        assert_eq!(sfx.inputs, vec!["SFX Impacts"]);
        let master = state.buses.iter().find(|bus| bus.id == "master").unwrap();
        assert_eq!(master.inputs, vec!["dialogue", "music", "sfx"]);
        assert!(
            state
                .chains
                .iter()
                .any(|chain| chain.id == "dialogue_cleanup")
        );
        assert!(
            state
                .chains
                .iter()
                .any(|chain| chain.id == "master_delivery")
        );
        let ducking = state
            .automation
            .iter()
            .find(|lane| lane.target == "music" && lane.parameter == "gain_db")
            .unwrap();
        assert_eq!(ducking.keyframes.len(), 4);
        assert_eq!(ducking.keyframes[0].time_s, 0.0);
        assert_eq!(ducking.keyframes[0].value, 0.0);
        assert_eq!(ducking.keyframes[1].time_s, 0.0);
        assert_eq!(ducking.keyframes[1].value, -8.0);
        assert_eq!(ducking.keyframes[2].time_s, 2.0);
        assert_eq!(ducking.keyframes[2].value, -8.0);
        assert_eq!(ducking.keyframes[3].time_s, 2.0);
        assert_eq!(ducking.keyframes[3].value, 0.0);
        let master_meter = state
            .meters
            .iter()
            .find(|meter| meter.target == "master")
            .unwrap();
        assert_eq!(master_meter.integrated_lufs, Some(-14.2));
        assert_eq!(master_meter.true_peak_db, Some(-1.1));
        assert_eq!(master_meter.noise_floor_db, Some(-62.0));
        assert!(!master_meter.clipping);
        let dialogue_meter = state
            .meters
            .iter()
            .find(|meter| meter.target == "dialogue")
            .unwrap();
        assert_eq!(dialogue_meter.integrated_lufs, Some(-18.0));
        assert!(dialogue_meter.clipping);

        let package = derive_audio_post_package(&timeline, &state);
        assert_eq!(package.session_ref, "audio/audio-finish.ptx");
        assert!(
            package
                .stem_deliverables
                .iter()
                .any(|stem| stem.id == "stem-dialogue" && stem.path == "audio/stems/dialogue.wav")
        );
        assert!(
            package
                .stem_deliverables
                .iter()
                .any(|stem| stem.id == "stem-music" && stem.path == "audio/stems/music.wav")
        );
        assert!(
            package
                .stem_deliverables
                .iter()
                .any(|stem| stem.id == "stem-sfx" && stem.path == "audio/stems/sfx.wav")
        );
        let adr = package
            .cues
            .iter()
            .find(|cue| cue.id == "cue-c-dialogue-adr-line-1")
            .unwrap();
        assert_eq!(adr.kind, "adr");
        assert_eq!(adr.clip_id, "c-dialogue");
        assert_eq!(adr.asset_id.as_deref(), Some("raw/dialogue.wav"));
        assert_eq!(adr.timeline_start_s, 0.5);
        assert_eq!(adr.timeline_end_s, 1.5);
        assert_eq!(adr.note.as_deref(), Some("boom rustle under dialogue"));
        let foley = package
            .cues
            .iter()
            .find(|cue| cue.id == "cue-c-door-foley-door")
            .unwrap();
        assert_eq!(foley.kind, "foley");
        assert_eq!(foley.asset_id.as_deref(), Some("raw/door.wav"));
    }

    #[test]
    fn color_finishing_state_derives_groups_and_grade_stacks_from_picture_clips() {
        let mut timeline = Timeline::empty("color finish");
        let mut video = Track::empty("V1 Picture", TrackKind::Video);
        let mut wide_a = timeline_clip("Wide A", "raw/cam-a.mov", "c-a1");
        let wide_a_metadata = wide_a.metadata.awidat.as_mut().unwrap();
        wide_a_metadata.extra.insert(
            "camera_color_space".to_string(),
            serde_json::json!("ARRI LogC4"),
        );
        wide_a_metadata.extra.insert(
            "camera_lut".to_string(),
            serde_json::json!("luts/alexa-logc4-to-709.cube"),
        );
        wide_a_metadata.extra.insert(
            "show_lut".to_string(),
            serde_json::json!("luts/show-night.cube"),
        );
        wide_a_metadata.extra.insert(
            "look_reference".to_string(),
            serde_json::json!("refs/night-look.dpx"),
        );
        video.children.push(TrackChild::Clip(wide_a));
        video.children.push(TrackChild::Clip(timeline_clip(
            "Close A",
            "raw/cam-a.mov",
            "c-a2",
        )));
        video.children.push(TrackChild::Clip(timeline_clip(
            "Wide B",
            "raw/cam-b.mov",
            "c-b1",
        )));
        let mut audio = Track::empty("A1 Dialogue", TrackKind::Audio);
        audio.children.push(TrackChild::Clip(timeline_clip(
            "Boom",
            "raw/dialogue.wav",
            "c-dialogue",
        )));
        timeline.tracks.children = vec![StackChild::Track(video), StackChild::Track(audio)];

        let state = derive_color_finishing_state(&timeline, "ARRI LogC4", "Rec.709 Gamma 2.4");

        assert_eq!(
            state.color_management.as_ref().unwrap().input_color_space,
            "ARRI LogC4"
        );
        assert_eq!(
            state
                .shot_groups
                .iter()
                .find(|group| group.id == "raw/cam-a.mov")
                .unwrap()
                .clip_ids,
            vec!["c-a1", "c-a2"]
        );
        assert!(
            state
                .shot_groups
                .iter()
                .all(|group| !group.clip_ids.contains(&"c-dialogue".to_string()))
        );
        let grade = state
            .grade_stacks
            .iter()
            .find(|stack| stack.id == "grade-c-a1")
            .unwrap();
        assert_eq!(grade.stages[0].kind, "color_space_transform");
        assert_eq!(
            grade.stages[0].params.get("input_color_space"),
            Some(&serde_json::json!("ARRI LogC4"))
        );
        assert_eq!(
            grade.stages[0].params.get("camera_lut"),
            Some(&serde_json::json!("luts/alexa-logc4-to-709.cube"))
        );
        assert_eq!(
            grade.stages[0].params.get("show_lut"),
            Some(&serde_json::json!("luts/show-night.cube"))
        );
        assert_eq!(grade.stages[1].kind, "primary_balance");
        assert!(
            state
                .reference_stills
                .iter()
                .any(|still| still.source == "refs/night-look.dpx")
        );

        let package = derive_color_handoff_package(&timeline, &state);
        assert_eq!(package.timeline_ref, "color/resolve-timeline.xml");
        assert_eq!(package.cdl_path, "color/cdl/shot-grades.ccc");
        assert_eq!(
            package.lut_refs,
            vec![
                "luts/alexa-logc4-to-709.cube".to_string(),
                "luts/show-night.cube".to_string()
            ]
        );
        assert!(
            package
                .reference_stills
                .iter()
                .any(|still| still.source == "refs/night-look.dpx")
        );
        let manifest = package
            .shot_manifests
            .iter()
            .find(|manifest| manifest.clip_id == "c-a1")
            .unwrap();
        assert_eq!(manifest.grade_stack_id, "grade-c-a1");
        assert_eq!(manifest.source_asset, "raw/cam-a.mov");
        assert_eq!(
            manifest.camera_lut.as_deref(),
            Some("luts/alexa-logc4-to-709.cube")
        );
        assert_eq!(manifest.show_lut.as_deref(), Some("luts/show-night.cube"));
        assert_eq!(manifest.cdl_ref, "color/cdl/c-a1.cdl");
    }

    #[test]
    fn vfx_pipeline_package_derives_graphs_and_tracking_sidecars_for_vfx_clips() {
        let mut timeline = Timeline::empty("vfx package");
        let mut hero = timeline_clip("Phone Screen", "raw/phone-plate.mov", "c-phone");
        hero.source_range = Some(TimeRange::new(
            RationalTime::new(240.0, 24.0),
            RationalTime::new(48.0, 24.0),
        ));
        if let MediaReference::External(reference) = &mut hero.media_reference {
            reference.available_range = Some(TimeRange::new(
                RationalTime::new(192.0, 24.0),
                RationalTime::new(288.0, 24.0),
            ));
        }
        let hero_metadata = hero.metadata.awidat.as_mut().unwrap();
        hero_metadata
            .extra
            .insert("vfx_shot_name".into(), serde_json::json!("SC010_SH020"));
        hero_metadata
            .extra
            .insert("vfx_vendor".into(), serde_json::json!("Vendor A"));
        hero_metadata
            .extra
            .insert("vfx_version".into(), serde_json::json!("v003"));
        hero.effects.push(Effect::new("awidat.vfx.screen_replace"));
        hero.effects.push(Effect::new("awidat.vfx.roto_mask"));
        hero.effects.push(Effect::new("awidat.vfx.green_key"));
        let mut clean = timeline_clip("Clean Plate", "raw/clean.mov", "c-clean");
        clean.effects.push(Effect::new("awidat.color_correction"));

        let mut video = Track::empty("V1 Picture", TrackKind::Video);
        video.children.push(TrackChild::Clip(hero));
        video.children.push(TrackChild::Clip(clean));
        timeline.tracks.children = vec![StackChild::Track(video)];

        let package = derive_vfx_pipeline_package(&timeline);

        assert_eq!(package.composition_graphs.len(), 1);
        let graph = &package.composition_graphs[0];
        assert!(graph.validate().is_empty());
        assert_eq!(graph.id, "vfx-c-phone");
        assert!(
            graph
                .nodes
                .iter()
                .any(|node| node.node_type == CompositionNodeType::MediaInput)
        );
        assert!(
            graph
                .nodes
                .iter()
                .any(|node| node.node_type == CompositionNodeType::TrackerBind)
        );
        let mask_node = graph
            .nodes
            .iter()
            .find(|node| node.node_type == CompositionNodeType::Mask)
            .unwrap();
        assert_eq!(
            mask_node.params.get("mask_id"),
            Some(&serde_json::json!("mask-c-phone"))
        );
        let matte_node = graph
            .nodes
            .iter()
            .find(|node| node.node_type == CompositionNodeType::Matte)
            .unwrap();
        assert_eq!(
            matte_node.params.get("matte_id"),
            Some(&serde_json::json!("matte-c-phone"))
        );
        assert!(graph.edges.iter().any(|edge| {
            edge.from == "tracker" && edge.to == "mask" && edge.input.as_deref() == Some("source")
        }));
        assert!(graph.edges.iter().any(|edge| {
            edge.from == "mask" && edge.to == "matte" && edge.input.as_deref() == Some("mask")
        }));
        assert!(graph.edges.iter().any(|edge| {
            edge.from == "matte" && edge.to == "output" && edge.input.as_deref() == Some("source")
        }));
        assert_eq!(graph.output_node_id.as_deref(), Some("output"));

        assert_eq!(package.tracking_package.tracks.len(), 1);
        assert!(package.tracking_package.validate().is_empty());
        let track = &package.tracking_package.tracks[0];
        assert_eq!(track.id, "track-c-phone");
        assert_eq!(track.asset_id, "raw/phone-plate.mov");
        assert_eq!(track.samples[0].frame, 0);

        assert_eq!(package.tracking_package.masks.len(), 1);
        let mask = &package.tracking_package.masks[0];
        assert_eq!(mask.id, "mask-c-phone");
        assert_eq!(mask.operation, MaskOperation::Add);
        assert_eq!(mask.keyframes.len(), 1);
        assert_eq!(mask.keyframes[0].time_s, 0.0);

        assert_eq!(package.tracking_package.mattes.len(), 1);
        let matte = &package.tracking_package.mattes[0];
        assert_eq!(matte.id, "matte-c-phone");
        assert_eq!(matte.alpha_source, "raw/phone-plate.mov");
        assert!(matte.review_thumbnails.contains(&"vfx-c-phone".to_string()));

        assert_eq!(package.shot_manifests.len(), 1);
        let manifest = &package.shot_manifests[0];
        assert_eq!(manifest.shot_id, "vfx-c-phone");
        assert_eq!(manifest.clip_id, "c-phone");
        assert_eq!(manifest.vendor_shot_name, "SC010_SH020");
        assert_eq!(manifest.vendor.as_deref(), Some("Vendor A"));
        assert_eq!(manifest.source_asset, "raw/phone-plate.mov");
        assert_eq!(manifest.timeline_start_s, 0.0);
        assert_eq!(manifest.timeline_end_s, 2.0);
        assert_eq!(manifest.source_start_s, Some(10.0));
        assert_eq!(manifest.source_end_s, Some(12.0));
        assert_eq!(manifest.handle_in_s, Some(2.0));
        assert_eq!(manifest.handle_out_s, Some(8.0));
        assert_eq!(manifest.pull_ref, "vfx/pulls/SC010_SH020.mov");
        assert_eq!(manifest.review_ref, "vfx/review/SC010_SH020_v003.mov");
        assert_eq!(manifest.current_version.as_deref(), Some("v003"));
        assert!(
            manifest
                .effect_names
                .contains(&"awidat.vfx.screen_replace".to_string())
        );
    }

    #[test]
    fn secure_review_package_collects_pending_proposals_and_watermarks_reviewer() {
        let metadata = AwidatTimelineMetadata {
            proposal_packages: vec![
                ProposalPackage {
                    id: "proposal-color".into(),
                    area: CapabilityArea::ColorFinishing,
                    status: ReviewStatus::Proposed,
                    summary: "Review cooler night grade".into(),
                    evidence: vec![EvidenceTrace {
                        source: "color_analysis".into(),
                        refs: vec![
                            "grade-c-a1".into(),
                            "renders/look-review.json".into(),
                            "frame:1728".into(),
                            "time_s:72.0".into(),
                        ],
                        reason: Some("large lift change".into()),
                        confidence: Some(0.72),
                    }],
                    rollback_refs: vec!["timeline@before-color".into()],
                },
                ProposalPackage {
                    id: "proposal-audio-old".into(),
                    area: CapabilityArea::AudioFinishing,
                    status: ReviewStatus::Accepted,
                    summary: "Accepted music duck".into(),
                    ..ProposalPackage::default()
                },
            ],
            preflight_reports: vec![PreflightReport {
                id: "preflight-youtube".into(),
                profile_id: "youtube".into(),
                findings: Vec::new(),
            }],
            package_manifests: vec![PackageManifest {
                id: "package-youtube".into(),
                artifacts: vec!["deliveries/youtube.mp4".into()],
                validation_reports: vec!["qc/youtube.json".into()],
            }],
            ..AwidatTimelineMetadata::default()
        };

        let package =
            derive_secure_review_package(&metadata, "director@example.com", "studio-cut-17");

        assert_eq!(package.id, "review-studio-cut-17-director-example-com");
        assert_eq!(package.proposals.len(), 1);
        assert_eq!(package.proposals[0].id, "proposal-color");
        assert_eq!(package.review_versions.len(), 1);
        let version = &package.review_versions[0];
        assert_eq!(version.id, "review-studio-cut-17-director-example-com-v001");
        assert_eq!(version.cut_id, "studio-cut-17");
        assert_eq!(version.package_id, package.id);
        assert_eq!(
            version.artifact_refs,
            vec![
                "deliveries/youtube.mp4".to_string(),
                "grade-c-a1".to_string(),
                "preflight:preflight-youtube".to_string(),
                "qc/youtube.json".to_string(),
                "renders/look-review.json".to_string(),
                "timeline@before-color".to_string()
            ]
        );
        assert_eq!(version.rollback_refs, vec!["timeline@before-color"]);
        assert_eq!(package.comments.len(), 1);
        let comment = &package.comments[0];
        assert_eq!(comment.id, "comment-proposal-color-color-analysis-0");
        assert_eq!(comment.proposal_id, "proposal-color");
        assert_eq!(comment.reviewer, "director@example.com");
        assert_eq!(comment.body, "large lift change");
        assert_eq!(comment.frame, Some(1728));
        assert_eq!(comment.time_s, Some(72.0));
        assert_eq!(
            comment.artifact_refs,
            vec![
                "grade-c-a1".to_string(),
                "renders/look-review.json".to_string()
            ]
        );
        assert!(package.artifact_refs.contains(&"grade-c-a1".to_string()));
        assert!(
            package
                .artifact_refs
                .contains(&"renders/look-review.json".to_string())
        );
        assert!(
            package
                .artifact_refs
                .contains(&"timeline@before-color".to_string())
        );
        assert!(
            package
                .artifact_refs
                .contains(&"preflight:preflight-youtube".to_string())
        );
        assert!(
            package
                .artifact_refs
                .contains(&"deliveries/youtube.mp4".to_string())
        );
        assert!(
            package
                .artifact_refs
                .contains(&"qc/youtube.json".to_string())
        );
        assert!(package.security.require_auth);
        assert!(!package.security.allow_download);
        assert_eq!(
            package.security.allowed_reviewers,
            vec!["director@example.com"]
        );
        assert!(package.security.watermark.contains("director@example.com"));
        assert!(package.security.watermark.contains("studio-cut-17"));
    }

    #[test]
    fn delivery_master_package_derives_final_masters_qc_and_archive_refs() {
        let mut feature_profile = DeliveryProfile::youtube_1080p();
        feature_profile.id = "feature_theatrical".into();
        feature_profile.name = "Feature Theatrical".into();
        feature_profile.platform = Some("theatrical".into());
        feature_profile.width = 4096;
        feature_profile.height = 2160;
        feature_profile.video_bitrate_kbps = Some(220_000);
        feature_profile.preflight_checks = vec![
            PreflightCheckKind::AspectRatio,
            PreflightCheckKind::Duration,
            PreflightCheckKind::Loudness,
            PreflightCheckKind::Captions,
        ];
        let metadata = AwidatTimelineMetadata {
            delivery_profiles: vec![feature_profile],
            preflight_reports: vec![PreflightReport {
                id: "preflight-feature".into(),
                profile_id: "feature_theatrical".into(),
                findings: Vec::new(),
            }],
            package_manifests: vec![PackageManifest {
                id: "package-feature".into(),
                artifacts: vec![
                    "deliveries/feature_theatrical/final-master.mov".into(),
                    "deliveries/feature_theatrical/final-master.dcp".into(),
                    "deliveries/feature_theatrical/subtitles/en.srt".into(),
                    "audio/stems/dialogue.wav".into(),
                    "audio/stems/music.wav".into(),
                    "audio/stems/sfx.wav".into(),
                ],
                validation_reports: vec!["qc/feature_theatrical/video-qc.json".into()],
            }],
            ..AwidatTimelineMetadata::default()
        };
        let audio = AudioPostPackage {
            stem_deliverables: vec![
                AudioStemDeliverable {
                    id: "stem-dialogue".into(),
                    bus_id: "dialogue".into(),
                    path: "audio/stems/dialogue.wav".into(),
                },
                AudioStemDeliverable {
                    id: "stem-music".into(),
                    bus_id: "music".into(),
                    path: "audio/stems/music.wav".into(),
                },
                AudioStemDeliverable {
                    id: "stem-sfx".into(),
                    bus_id: "sfx".into(),
                    path: "audio/stems/sfx.wav".into(),
                },
            ],
            ..AudioPostPackage::default()
        };

        let package = derive_delivery_master_package(&metadata, &audio);

        assert_eq!(package.profile_id, "feature_theatrical");
        assert!(!package.blocked);
        assert_eq!(
            package.qc_reports,
            vec![
                "preflight:preflight-feature".to_string(),
                "qc/feature_theatrical/video-qc.json".to_string()
            ]
        );
        assert!(
            package
                .archive_refs
                .contains(&"archive/feature_theatrical/project.otio".to_string())
        );
        assert!(
            package
                .archive_refs
                .contains(&"archive/feature_theatrical/source-manifest.json".to_string())
        );
        let prores = package
            .deliverables
            .iter()
            .find(|artifact| artifact.kind == "prores_master")
            .unwrap();
        assert_eq!(
            prores.path,
            "deliveries/feature_theatrical/final-master.mov"
        );
        assert!(prores.required);
        assert!(prores.present);
        assert!(
            package
                .deliverables
                .iter()
                .any(|artifact| artifact.kind == "dcp" && artifact.present)
        );
        assert!(
            package
                .deliverables
                .iter()
                .any(|artifact| artifact.kind == "imf" && !artifact.present)
        );
        assert!(
            package
                .deliverables
                .iter()
                .any(|artifact| artifact.kind == "textless_master" && !artifact.present)
        );
        assert!(
            package
                .deliverables
                .iter()
                .any(|artifact| artifact.kind == "caption" && artifact.present)
        );
        assert!(
            package
                .deliverables
                .iter()
                .any(|artifact| artifact.kind == "me_mix" && !artifact.present)
        );
        assert!(
            package
                .deliverables
                .iter()
                .any(|artifact| artifact.kind == "audio_stem"
                    && artifact.path == "audio/stems/dialogue.wav"
                    && artifact.present)
        );
    }

    #[test]
    fn delivery_master_package_blocks_on_error_preflight_findings() {
        let metadata = AwidatTimelineMetadata {
            preflight_reports: vec![PreflightReport {
                id: "preflight-streaming".into(),
                profile_id: "youtube_1080p".into(),
                findings: vec![PreflightFinding {
                    check: PreflightCheckKind::Loudness,
                    severity: FindingSeverity::Error,
                    message: "loudness measurement is missing".into(),
                    fix_ref: None,
                }],
            }],
            ..AwidatTimelineMetadata::default()
        };

        let package = derive_delivery_master_package(&metadata, &AudioPostPackage::default());

        assert_eq!(package.profile_id, "youtube_1080p");
        assert!(package.blocked);
        assert!(
            package
                .qc_reports
                .contains(&"preflight:preflight-streaming".to_string())
        );
    }

    #[test]
    fn timeline_scale_profile_flags_large_timelines_for_windowed_virtualized_ui() {
        let mut timeline = Timeline::empty("large timeline");
        let mut video = Track::empty("V1 Picture", TrackKind::Video);
        for index in 0..1_200 {
            let mut clip = timeline_clip(
                &format!("Shot {index}"),
                &format!("raw/shot-{index:04}.mov"),
                &format!("c-{index:04}"),
            );
            clip.source_range = Some(TimeRange::new(
                RationalTime::zero(24.0),
                RationalTime::new(24.0, 24.0),
            ));
            video.children.push(TrackChild::Clip(clip));
        }
        timeline.tracks.children = vec![StackChild::Track(video)];

        let profile = derive_timeline_scale_profile(&timeline, 200, 60.0);

        assert_eq!(profile.track_count, 1);
        assert_eq!(profile.clip_count, 1_200);
        assert_eq!(profile.item_count, 1_200);
        assert_eq!(profile.total_duration_s, 1_200.0);
        assert!(profile.requires_virtualization);
        assert_eq!(profile.max_visible_rows, 200);
        assert_eq!(profile.recommended_window_s, 60.0);
        assert!(
            profile
                .guidance
                .iter()
                .any(|note| note.contains("virtualized"))
        );
    }

    #[test]
    fn timeline_scale_profile_counts_nested_stack_duration_for_windowing() {
        let mut timeline = Timeline::empty("nested scale");
        let mut nested = Stack::empty("Scene 1");
        let mut video = Track::empty("V1 Picture", TrackKind::Video);
        let mut clip = timeline_clip("Nested Shot", "raw/nested.mov", "c-nested");
        clip.source_range = Some(TimeRange::new(
            RationalTime::zero(24.0),
            RationalTime::new(48.0, 24.0),
        ));
        video.children.push(TrackChild::Clip(clip));
        let mut audio = Track::empty("A1 Dialogue", TrackKind::Audio);
        audio.children.push(TrackChild::Clip(timeline_clip(
            "Nested Dialogue",
            "raw/nested.wav",
            "c-nested-audio",
        )));
        nested.children.push(StackChild::Track(video));
        nested.children.push(StackChild::Track(audio));
        timeline.tracks.children = vec![StackChild::Stack(nested)];

        let profile = derive_timeline_scale_profile(&timeline, 200, 60.0);

        assert_eq!(profile.track_count, 2);
        assert_eq!(profile.clip_count, 2);
        assert_eq!(profile.item_count, 3);
        assert_eq!(profile.total_duration_s, 2.0);
    }

    #[test]
    fn timeline_scale_profile_builds_concrete_row_time_windows_for_large_projects() {
        let mut timeline = Timeline::empty("industrial scale");
        let source_range = TimeRange::new(
            RationalTime::zero(24.0),
            RationalTime::new(180.0 * 24.0, 24.0),
        );
        for index in 0..450 {
            let mut track = Track::empty(format!("V{index}"), TrackKind::Video);
            let mut clip = timeline_clip(
                &format!("Shot {index}"),
                &format!("raw/shot-{index:04}.mov"),
                &format!("c-{index:04}"),
            );
            clip.source_range = Some(source_range.clone());
            track.children.push(TrackChild::Clip(clip));
            timeline.tracks.children.push(StackChild::Track(track));
        }

        let profile = derive_timeline_scale_profile(&timeline, 200, 60.0);

        assert!(profile.requires_virtualization);
        assert_eq!(profile.max_items_per_window, 200);
        assert_eq!(profile.row_windows.len(), 3);
        assert_eq!(profile.row_windows[0].start_row, 0);
        assert_eq!(profile.row_windows[0].end_row, 200);
        assert_eq!(profile.row_windows[2].start_row, 400);
        assert_eq!(profile.row_windows[2].end_row, 450);
        assert_eq!(profile.time_windows.len(), 3);
        assert_eq!(profile.time_windows[0].start_s, 0.0);
        assert_eq!(profile.time_windows[0].end_s, 60.0);
        assert_eq!(profile.time_windows[2].start_s, 120.0);
        assert_eq!(profile.time_windows[2].end_s, 180.0);
        assert!(
            profile
                .agent_window_refs
                .contains(&"timeline-window:r0-199:t0.000-60.000".to_string())
        );
        assert!(
            profile
                .guidance
                .iter()
                .any(|note| note.contains("3 row windows") && note.contains("3 time windows"))
        );
    }

    #[test]
    fn professional_pipeline_snapshot_derives_department_surfaces_together() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("raw")).unwrap();
        fs::write(dir.path().join("raw/hero.mov"), b"hero").unwrap();
        fs::write(dir.path().join("raw/dialogue.wav"), b"dialogue").unwrap();

        let mut timeline = Timeline::empty("pipeline snapshot");
        timeline.metadata.awidat.as_mut().unwrap().source_assets =
            vec!["raw/hero.mov".into(), "raw/dialogue.wav".into()];
        let mut hero = timeline_clip("Hero", "raw/hero.mov", "c-hero");
        hero.effects.push(Effect::new("awidat.vfx.screen_replace"));
        hero.source_range = Some(TimeRange::new(
            RationalTime::zero(24.0),
            RationalTime::new(48.0, 24.0),
        ));
        let hero_metadata = hero.metadata.awidat.as_mut().unwrap();
        hero_metadata.extra.insert(
            "sync_group_id".into(),
            serde_json::json!("scene-12a-take-3"),
        );
        hero_metadata
            .extra
            .insert("scene".into(), serde_json::json!("12A"));
        hero_metadata
            .extra
            .insert("take".into(), serde_json::json!("3"));
        let mut select_marker = Marker::new(
            "keeper take",
            TimeRange::new(RationalTime::new(12.0, 24.0), RationalTime::new(24.0, 24.0)),
        );
        select_marker.comment = Some("best performance".into());
        select_marker.metadata.awidat = Some(AwidatMarkerMetadata {
            category: Some("favorite".into()),
            note: Some("director circled this take".into()),
            ..AwidatMarkerMetadata::default()
        });
        hero.markers.push(select_marker);
        let mut dialogue = timeline_clip("Dialogue", "raw/dialogue.wav", "c-dialogue");
        dialogue.source_range = Some(TimeRange::new(
            RationalTime::zero(48_000.0),
            RationalTime::new(96_000.0, 48_000.0),
        ));
        let dialogue_metadata = dialogue.metadata.awidat.as_mut().unwrap();
        dialogue_metadata.extra.insert(
            "sync_group_id".into(),
            serde_json::json!("scene-12a-take-3"),
        );
        dialogue_metadata
            .extra
            .insert("scene".into(), serde_json::json!("12A"));
        dialogue_metadata
            .extra
            .insert("take".into(), serde_json::json!("3"));
        let mut video = Track::empty("V1 Picture", TrackKind::Video);
        video.children.push(TrackChild::Clip(hero));
        let mut audio = Track::empty("A1 Dialogue", TrackKind::Audio);
        audio.children.push(TrackChild::Clip(dialogue));
        timeline.tracks.children = vec![StackChild::Track(video), StackChild::Track(audio)];
        timeline
            .metadata
            .awidat
            .as_mut()
            .unwrap()
            .proposal_packages
            .push(ProposalPackage {
                id: "proposal-review".into(),
                area: CapabilityArea::EditorialIntentAndReview,
                status: ReviewStatus::Proposed,
                summary: "Review first cut".into(),
                evidence: vec![EvidenceTrace {
                    source: "edit_review".into(),
                    refs: vec!["c-hero".into()],
                    reason: Some("story beat changed".into()),
                    confidence: Some(0.8),
                }],
                rollback_refs: vec!["timeline@before-review".into()],
            });

        let snapshot = derive_professional_pipeline_snapshot(
            dir.path(),
            &timeline,
            "director@example.com",
            "cut-01",
        );

        assert_eq!(snapshot.asset_catalog.assets.len(), 2);
        assert_eq!(snapshot.media_operations.asset_count, 2);
        assert!(
            snapshot
                .media_operations
                .actions
                .iter()
                .any(|action| action.kind == "verify_checksum")
        );
        assert!(
            snapshot
                .audio_finishing
                .buses
                .iter()
                .any(|bus| bus.id == "dialogue")
        );
        assert_eq!(
            snapshot
                .color_finishing
                .color_management
                .unwrap()
                .output_color_space,
            "Rec.709 Gamma 2.4"
        );
        assert_eq!(snapshot.vfx.composition_graphs.len(), 1);
        assert_eq!(snapshot.review.proposals.len(), 1);
        assert_eq!(snapshot.delivery.profile_id, "youtube_1080p");
        assert!(
            snapshot
                .delivery
                .deliverables
                .iter()
                .any(|artifact| artifact.kind == "prores_master")
        );
        assert_eq!(snapshot.scale_profile.clip_count, 2);
        let hero_asset = snapshot
            .asset_catalog
            .assets
            .iter()
            .find(|asset| asset.path == "raw/hero.mov")
            .unwrap();
        assert_eq!(
            hero_asset.usage.select_ids,
            vec!["select-c-hero-keeper-take".to_string()]
        );
        assert_eq!(snapshot.assistant_workflow.sync_groups.len(), 1);
        let sync_group = &snapshot.assistant_workflow.sync_groups[0];
        assert_eq!(sync_group.id, "scene-12a-take-3");
        assert_eq!(sync_group.scene.as_deref(), Some("12A"));
        assert_eq!(sync_group.take.as_deref(), Some("3"));
        assert_eq!(
            sync_group.clip_ids,
            vec!["c-dialogue".to_string(), "c-hero".to_string()]
        );
        assert_eq!(
            sync_group.asset_ids,
            vec!["raw/dialogue.wav".to_string(), "raw/hero.mov".to_string()]
        );
        assert_eq!(sync_group.readiness, ReadinessState::Ready);
        let selects_lens = snapshot
            .lens_snapshots
            .iter()
            .find(|lens| lens.lens == WorkflowLens::Selects)
            .unwrap();
        assert_eq!(selects_lens.readiness, ReadinessState::Ready);
        assert_eq!(
            selects_lens.artifacts,
            vec!["select-c-hero-keeper-take".to_string()]
        );
        assert!(
            snapshot
                .lens_snapshots
                .iter()
                .any(|lens| lens.lens == WorkflowLens::Vfx)
        );
    }

    fn test_proxy_path_for(project_root: &Path, asset_path: &Path) -> PathBuf {
        let stem = asset_path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("asset");
        project_root.join(".awidat").join("proxies").join(format!(
            "{stem}-{:08x}.mp4",
            test_stable_path_hash(asset_path)
        ))
    }

    fn test_stable_path_hash(path: &Path) -> u32 {
        let mut hash = 0x811c9dc5_u32;
        for byte in path.to_string_lossy().as_bytes() {
            hash ^= u32::from(*byte);
            hash = hash.wrapping_mul(0x01000193);
        }
        hash
    }

    fn assert_media_action(
        state: &MediaOperationsState,
        asset_id: &str,
        kind: &str,
        source_contains: Option<&str>,
        target_contains: Option<&str>,
        blocked: bool,
    ) {
        let action = state
            .actions
            .iter()
            .find(|action| action.asset_id == asset_id && action.kind == kind)
            .unwrap_or_else(|| panic!("missing media action {kind} for {asset_id}"));
        assert_eq!(action.blocked, blocked);
        if let Some(source_contains) = source_contains {
            assert!(
                action.source.contains(source_contains),
                "source {:?} did not contain {source_contains:?}",
                action.source
            );
        }
        if let Some(target_contains) = target_contains {
            assert!(
                action
                    .target
                    .as_deref()
                    .unwrap_or_default()
                    .contains(target_contains),
                "target {:?} did not contain {target_contains:?}",
                action.target
            );
        }
    }

    fn timeline_clip(name: &str, asset: &str, clip_uuid: &str) -> Clip {
        let mut extra = HashMap::new();
        extra.insert(
            "clip_uuid".to_string(),
            serde_json::Value::String(clip_uuid.to_string()),
        );
        let mut clip = Clip::empty(name);
        clip.media_reference = MediaReference::External(ExternalReference::new(asset));
        clip.metadata.awidat = Some(AwidatClipMetadata {
            extra,
            ..AwidatClipMetadata::default()
        });
        clip
    }

    fn audio_track(name: &str) -> Track {
        Track::empty(name, TrackKind::Audio)
    }
}
