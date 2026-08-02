//! Tests for professional workflow lens and orchestration helpers.

use montage_core::professional::{
    LensCorrectionAction, build_workflow_lens_snapshots, inspect_pre_autonomy_readiness,
};
use montage_proto::montage_meta::MontageTimelineMetadata;
use montage_proto::professional::{
    AssetCatalog, AssetRecord, CapabilityArea, CapabilityRegistry, CompositionGraph,
    DeliveryProfile, PipelineConflict, PlannerPassContract, ReadinessState, WorkflowLens,
};

#[test]
fn workflow_lenses_expose_readiness_artifacts_findings_and_actions() {
    let metadata = MontageTimelineMetadata {
        asset_catalog: Some(AssetCatalog {
            assets: vec![AssetRecord {
                id: "cam-a".into(),
                path: "raw/cam-a.mp4".into(),
                ..AssetRecord::default()
            }],
            ..AssetCatalog::default()
        }),
        composition_graphs: vec![CompositionGraph::single_output("comp")],
        delivery_profiles: vec![DeliveryProfile::youtube_1080p()],
        workflow_lenses: vec![
            WorkflowLens::Media,
            WorkflowLens::Vfx,
            WorkflowLens::Delivery,
        ],
        ..MontageTimelineMetadata::default()
    };

    let snapshots = build_workflow_lens_snapshots(&metadata);
    let Some(media) = snapshots
        .iter()
        .find(|snapshot| snapshot.lens == WorkflowLens::Media)
    else {
        panic!("media lens");
    };
    let Some(vfx) = snapshots
        .iter()
        .find(|snapshot| snapshot.lens == WorkflowLens::Vfx)
    else {
        panic!("vfx lens");
    };

    assert_eq!(media.readiness, ReadinessState::Ready);
    assert!(
        media
            .artifacts
            .iter()
            .any(|artifact| artifact.contains("raw/cam-a.mp4"))
    );
    assert!(
        vfx.correction_actions
            .contains(&LensCorrectionAction::OpenReview)
    );
}

#[test]
fn orchestration_inspection_uses_registry_and_detects_cross_stage_conflicts() {
    let metadata = MontageTimelineMetadata {
        capability_registry: Some(CapabilityRegistry::professional_substrate_v1()),
        planner_passes: vec![
            PlannerPassContract {
                id: "color-pass".into(),
                area: CapabilityArea::ColorFinishing,
                inputs: vec!["timeline".into()],
                outputs: vec!["render/look.mov".into()],
                conflicts: vec![PipelineConflict {
                    id: "color-vs-delivery".into(),
                    areas: vec![
                        CapabilityArea::ColorFinishing,
                        CapabilityArea::DeliveryProfilesAndPreflight,
                    ],
                    message: "grade output does not match delivery color space".into(),
                }],
            },
            PlannerPassContract {
                id: "delivery-pass".into(),
                area: CapabilityArea::DeliveryProfilesAndPreflight,
                inputs: vec!["render/look.mov".into()],
                outputs: vec!["deliverable/youtube.mp4".into()],
                conflicts: Vec::new(),
            },
        ],
        ..MontageTimelineMetadata::default()
    };

    let inspection = inspect_pre_autonomy_readiness(&metadata);
    assert_eq!(
        inspection
            .readiness
            .stage(CapabilityArea::PreAutonomyOrchestrationContract),
        Some(ReadinessState::Ready)
    );
    assert_eq!(inspection.conflicts.len(), 1);
    assert!(
        inspection.planner_edges.iter().any(|edge| {
            edge.from_pass_id == "color-pass" && edge.to_pass_id == "delivery-pass"
        })
    );
}
