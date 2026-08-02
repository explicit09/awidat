//! Live professional workflow lenses, audio finishing, and readiness inspection.

use std::collections::{HashMap, HashSet};

use montage_proto::montage_meta::MontageTimelineMetadata;
use montage_proto::otio::{
    Clip, Stack, StackChild, Timeline, Track, TrackChild, TrackKind as OtioTrackKind,
};
use montage_proto::professional::{
    AudioAutomationLane, AudioBus, AudioChainPreset, AudioFinishingState, AudioMeterReading,
    AudioRole, CapabilityArea, CapabilityRegistry, FindingSeverity, Keyframe, PipelineConflict,
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
    metadata: &MontageTimelineMetadata,
) -> Vec<WorkflowLensSnapshot> {
    let readiness = metadata.build_professional_readiness_report();
    all_lenses()
        .into_iter()
        .map(|lens| lens_snapshot(metadata, &readiness, lens))
        .collect()
}

fn lens_snapshot(
    metadata: &MontageTimelineMetadata,
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

fn lens_artifacts(metadata: &MontageTimelineMetadata, lens: WorkflowLens) -> Vec<String> {
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
        .montage
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

fn clip_duration_s(clip: &Clip) -> f64 {
    clip.source_range
        .as_ref()
        .map(|range| range.duration.to_seconds())
        .unwrap_or(0.0)
}

/// Inspect registry, planner pass data-flow, and conflicts for autopilot gates.
pub fn inspect_pre_autonomy_readiness(
    metadata: &MontageTimelineMetadata,
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
    metadata: &MontageTimelineMetadata,
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

#[cfg(test)]
mod tests {
    use montage_proto::otio::{
        Clip, RationalTime, StackChild, TimeRange, Timeline, Track, TrackChild, TrackKind,
    };

    use super::*;

    #[test]
    fn audio_finishing_state_derives_live_buses_automation_and_meters() {
        let mut timeline = Timeline::empty("audio finish");
        let mut dialogue = Track::empty("A1 Dialogue Boom", TrackKind::Audio);
        let mut dialogue_clip = Clip::empty("Line 1");
        dialogue_clip.source_range = Some(TimeRange::new(
            RationalTime::zero(48_000.0),
            RationalTime::new(96_000.0, 48_000.0),
        ));
        dialogue.children.push(TrackChild::Clip(dialogue_clip));
        timeline.tracks.children = vec![
            StackChild::Track(dialogue),
            StackChild::Track(Track::empty("MX Music", TrackKind::Audio)),
            StackChild::Track(Track::empty("SFX Impacts", TrackKind::Audio)),
            StackChild::Track(Track::empty("V1 Picture", TrackKind::Video)),
        ];
        timeline.metadata.montage.as_mut().unwrap().extra.insert(
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
        assert_eq!(ducking.keyframes[1].value, -8.0);
        assert_eq!(ducking.keyframes[2].time_s, 2.0);

        let master_meter = state
            .meters
            .iter()
            .find(|meter| meter.target == "master")
            .unwrap();
        assert_eq!(master_meter.integrated_lufs, Some(-14.2));
        assert_eq!(master_meter.true_peak_db, Some(-1.1));
        assert!(!master_meter.clipping);
        let dialogue_meter = state
            .meters
            .iter()
            .find(|meter| meter.target == "dialogue")
            .unwrap();
        assert_eq!(dialogue_meter.integrated_lufs, Some(-18.0));
        assert!(dialogue_meter.clipping);
    }
}
