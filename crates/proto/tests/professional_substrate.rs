//! Professional substrate schema acceptance tests.

use awidat_proto::awidat_meta::AwidatTimelineMetadata;
use awidat_proto::professional::{
    AssetCatalog, AssetQuery, AssetReadiness, AssetRecord, AssetRole, AudioBus,
    AudioFinishingState, CapabilityArea, CapabilityRegistry, CapabilityStatus, ColorFinishingState,
    CompositionGraph, CompositionNode, CompositionNodeType, DeliveryPreflightInput,
    DeliveryProfile, GradeStack, Keyframe, MotionGraphicsTemplate, ParameterAnimation,
    PipelineReadinessReport, ReadinessState, SelectDecision, SourceRange, SourceSelect, Stringout,
    TemplateSlot, TrackingPackage, WorkflowLens,
};

#[test]
fn timeline_metadata_carries_all_professional_substrate_documents() {
    let metadata = AwidatTimelineMetadata {
        asset_catalog: Some(AssetCatalog {
            assets: vec![AssetRecord {
                id: "asset-a".into(),
                path: "raw/a.mov".into(),
                role: AssetRole::Video,
                tags: vec!["interview".into()],
                readiness: AssetReadiness {
                    proxy: ReadinessState::Ready,
                    index: ReadinessState::Blocked,
                    online: ReadinessState::Ready,
                },
                ..AssetRecord::default()
            }],
            ..AssetCatalog::default()
        }),
        selects: vec![SourceSelect {
            id: "sel-a".into(),
            asset_id: "asset-a".into(),
            range: SourceRange {
                start_s: 10.0,
                end_s: 18.0,
            },
            decision: SelectDecision::Select,
            reason: Some("clean answer".into()),
            evidence_refs: vec!["whisper:asset-a:10-18".into()],
            ..SourceSelect::default()
        }],
        stringouts: vec![Stringout {
            id: "stringout-a".into(),
            select_ids: vec!["sel-a".into()],
            ..Stringout::default()
        }],
        delivery_profiles: vec![DeliveryProfile::youtube_1080p()],
        workflow_lenses: vec![WorkflowLens::Assembly],
        capability_registry: Some(CapabilityRegistry::professional_substrate_v1()),
        pipeline_readiness: Some(PipelineReadinessReport::from_registry(
            CapabilityRegistry::professional_substrate_v1(),
        )),
        ..AwidatTimelineMetadata::default()
    };

    let json = match serde_json::to_string(&metadata) {
        Ok(json) => json,
        Err(error) => panic!("serialize metadata: {error}"),
    };
    let roundtrip: AwidatTimelineMetadata = match serde_json::from_str(&json) {
        Ok(metadata) => metadata,
        Err(error) => panic!("deserialize metadata: {error}"),
    };

    let asset_catalog = match roundtrip.asset_catalog {
        Some(asset_catalog) => asset_catalog,
        None => panic!("asset catalog missing"),
    };
    let asset = match asset_catalog.assets.first() {
        Some(asset) => asset,
        None => panic!("asset missing"),
    };

    assert_eq!(asset.readiness.index, ReadinessState::Blocked);
    assert_eq!(roundtrip.selects[0].decision, SelectDecision::Select);
    assert_eq!(roundtrip.stringouts[0].select_ids, vec!["sel-a"]);
    assert_eq!(
        roundtrip.delivery_profiles[0].platform.as_deref(),
        Some("youtube")
    );
    assert!(roundtrip.workflow_lenses.contains(&WorkflowLens::Assembly));
    let capability_registry = match roundtrip.capability_registry {
        Some(capability_registry) => capability_registry,
        None => panic!("capability registry missing"),
    };
    assert!(
        capability_registry
            .capabilities
            .iter()
            .any(|capability| capability.area == CapabilityArea::CompositionGraph)
    );
}

#[test]
fn readiness_report_marks_unavailable_capabilities_as_blocked() {
    let report = PipelineReadinessReport::from_registry(CapabilityRegistry {
        capabilities: vec![
            CapabilityStatus {
                area: CapabilityArea::AssetCatalog,
                available: true,
                previewable: true,
                renderable: false,
                preflighted: true,
                safe_for_autopilot: true,
                blocker: None,
            },
            CapabilityStatus {
                area: CapabilityArea::TrackingMasksMattes,
                available: false,
                previewable: false,
                renderable: false,
                preflighted: false,
                safe_for_autopilot: false,
                blocker: Some("tracker sidecar missing".into()),
            },
        ],
    });

    assert_eq!(
        match report.stage(CapabilityArea::AssetCatalog) {
            Some(state) => state,
            None => panic!("asset stage missing"),
        },
        ReadinessState::Ready
    );
    assert_eq!(
        match report.stage(CapabilityArea::TrackingMasksMattes) {
            Some(state) => state,
            None => panic!("tracking stage missing"),
        },
        ReadinessState::Blocked
    );
    assert_eq!(report.blockers, vec!["tracker sidecar missing"]);
}

#[test]
fn asset_catalog_query_filters_by_professional_review_fields() {
    let catalog = AssetCatalog {
        assets: vec![
            AssetRecord {
                id: "asset-a".into(),
                path: "raw/a.mov".into(),
                role: AssetRole::Video,
                bin_id: Some("interviews".into()),
                tags: vec!["hero".into(), "interview".into()],
                readiness: AssetReadiness {
                    proxy: ReadinessState::Ready,
                    index: ReadinessState::Ready,
                    online: ReadinessState::Ready,
                },
                ..AssetRecord::default()
            },
            AssetRecord {
                id: "asset-b".into(),
                path: "raw/b.wav".into(),
                role: AssetRole::Audio,
                bin_id: Some("audio".into()),
                tags: vec!["wildtrack".into()],
                readiness: AssetReadiness {
                    proxy: ReadinessState::Pending,
                    index: ReadinessState::Blocked,
                    online: ReadinessState::Ready,
                },
                ..AssetRecord::default()
            },
        ],
        ..AssetCatalog::default()
    };

    let results = catalog.query(&AssetQuery {
        bin_id: Some("interviews".into()),
        role: Some(AssetRole::Video),
        tags: vec!["hero".into()],
        readiness: Some(ReadinessState::Ready),
    });

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].id, "asset-a");
}

#[test]
fn substrate_validation_catches_cross_stage_pipeline_issues() {
    let metadata = AwidatTimelineMetadata {
        asset_catalog: Some(AssetCatalog {
            assets: vec![AssetRecord {
                id: "asset-a".into(),
                path: "raw/a.mov".into(),
                ..AssetRecord::default()
            }],
            ..AssetCatalog::default()
        }),
        selects: vec![SourceSelect {
            id: "select-bad".into(),
            asset_id: "missing-asset".into(),
            range: SourceRange {
                start_s: 12.0,
                end_s: 4.0,
            },
            ..SourceSelect::default()
        }],
        stringouts: vec![Stringout {
            id: "stringout-bad".into(),
            select_ids: vec!["missing-select".into()],
            ..Stringout::default()
        }],
        parameter_animations: vec![ParameterAnimation {
            id: "anim-bad".into(),
            keyframes: vec![Keyframe::linear(2.0, 1.0), Keyframe::linear(1.0, 0.0)],
            ..ParameterAnimation::default()
        }],
        motion_templates: vec![MotionGraphicsTemplate {
            id: "template-bad".into(),
            name: "Lower Third".into(),
            slots: vec![TemplateSlot {
                id: "name".into(),
                required: true,
                ..TemplateSlot::default()
            }],
            ..MotionGraphicsTemplate::default()
        }],
        composition_graphs: vec![CompositionGraph {
            id: "comp-bad".into(),
            nodes: vec![CompositionNode {
                id: "media".into(),
                node_type: CompositionNodeType::MediaInput,
                ..CompositionNode::default()
            }],
            output_node_id: Some("missing-output".into()),
            ..CompositionGraph::default()
        }],
        ..AwidatTimelineMetadata::default()
    };

    let diagnostics = metadata.validate_professional_substrate();
    let messages: Vec<&str> = diagnostics
        .iter()
        .map(|diagnostic| diagnostic.message.as_str())
        .collect();

    assert!(
        messages
            .iter()
            .any(|message| message.contains("missing-asset"))
    );
    assert!(messages.iter().any(|message| message.contains("start_s")));
    assert!(
        messages
            .iter()
            .any(|message| message.contains("missing-select"))
    );
    assert!(messages.iter().any(|message| message.contains("anim-bad")));
    assert!(
        messages
            .iter()
            .any(|message| message.contains("template-bad"))
    );
    assert!(
        messages
            .iter()
            .any(|message| message.contains("missing-output"))
    );
}

#[test]
fn delivery_profile_preflight_returns_actionable_findings() {
    let profile = DeliveryProfile::youtube_1080p();
    let report = profile.run_preflight(DeliveryPreflightInput {
        aspect_ratio: "9:16".into(),
        duration_s: Some(61.0),
        video_bitrate_kbps: Some(2_000),
        integrated_lufs: Some(-24.0),
        has_captions: false,
        has_required_metadata: false,
        has_thumbnail: false,
        safe_area_violations: 2,
    });

    assert_eq!(report.profile_id, "youtube_1080p");
    assert!(
        report
            .findings
            .iter()
            .any(|finding| finding.message.contains("aspect ratio"))
    );
    assert!(
        report
            .findings
            .iter()
            .any(|finding| finding.message.contains("loudness"))
    );
    assert!(
        report
            .findings
            .iter()
            .any(|finding| finding.message.contains("captions"))
    );
    assert!(
        report
            .findings
            .iter()
            .any(|finding| finding.message.contains("metadata"))
    );
}

#[test]
fn metadata_builds_readiness_across_all_thirteen_capability_areas() {
    let metadata = AwidatTimelineMetadata {
        asset_catalog: Some(AssetCatalog {
            assets: vec![AssetRecord {
                id: "asset-a".into(),
                path: "raw/a.mov".into(),
                ..AssetRecord::default()
            }],
            ..AssetCatalog::default()
        }),
        selects: vec![SourceSelect {
            id: "select-a".into(),
            asset_id: "asset-a".into(),
            range: SourceRange {
                start_s: 1.0,
                end_s: 2.0,
            },
            ..SourceSelect::default()
        }],
        stringouts: vec![Stringout {
            id: "stringout-a".into(),
            select_ids: vec!["select-a".into()],
            ..Stringout::default()
        }],
        cut_boundaries: std::collections::HashMap::from([(
            "clip-a::clip-b".into(),
            awidat_proto::awidat_meta::SemanticCutSpec {
                cut_type: awidat_proto::awidat_meta::CutType::HardCut,
                intent: "speaker_handoff".into(),
                energy: None,
                audio_relation: awidat_proto::awidat_meta::AudioRelation::Sync,
                confidence: None,
                reason: None,
                extra: std::collections::HashMap::new(),
            },
        )]),
        parameter_animations: vec![ParameterAnimation {
            id: "anim-a".into(),
            keyframes: vec![Keyframe::linear(0.0, 0.0), Keyframe::linear(1.0, 1.0)],
            ..ParameterAnimation::default()
        }],
        motion_templates: vec![MotionGraphicsTemplate {
            id: "template-a".into(),
            name: "Lower Third".into(),
            ..MotionGraphicsTemplate::default()
        }],
        composition_graphs: vec![CompositionGraph::single_output("comp-a")],
        tracking_package: Some(TrackingPackage::default()),
        color_finishing: Some(ColorFinishingState {
            grade_stacks: vec![GradeStack {
                id: "grade-a".into(),
                ..GradeStack::default()
            }],
            ..ColorFinishingState::default()
        }),
        audio_finishing: Some(AudioFinishingState {
            buses: vec![AudioBus {
                id: "dialogue".into(),
                ..AudioBus::default()
            }],
            ..AudioFinishingState::default()
        }),
        delivery_profiles: vec![DeliveryProfile::youtube_1080p()],
        workflow_lenses: vec![WorkflowLens::Media],
        capability_registry: Some(CapabilityRegistry::professional_substrate_v1()),
        ..AwidatTimelineMetadata::default()
    };

    let report = metadata.build_professional_readiness_report();

    assert_eq!(report.stages.len(), 13);
    assert_eq!(
        report.stage(CapabilityArea::AssetCatalog),
        Some(ReadinessState::Ready)
    );
    assert_eq!(
        report.stage(CapabilityArea::SourceReviewSelects),
        Some(ReadinessState::Ready)
    );
    assert_eq!(
        report.stage(CapabilityArea::PreAutonomyOrchestrationContract),
        Some(ReadinessState::Ready)
    );
}
