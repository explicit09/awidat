//! Professional substrate schema acceptance tests.

use std::collections::BTreeMap;

use awidat_proto::awidat_meta::{AwidatTimelineMetadata, BeatMarker, BeatMarkerRole};
use awidat_proto::professional::{
    AssetCatalog, AssetQuery, AssetReadiness, AssetRecord, AssetRole, AudioBus,
    AudioFinishingState, CapabilityArea, CapabilityRegistry, CapabilityStatus, ColorFinishingState,
    CompositionGraph, CompositionNode, CompositionNodeType, DeliveryPreflightInput,
    DeliveryProfile, ExportMode, ExportOutputSettings, ExportPreset, ExportRange, ExpressionLink,
    ExpressionSource, FindingSeverity, GradeStack, GroundingBoxFormat, GroundingDetection,
    GroundingEvidence, GroundingEvidenceStatus, HardwareAccelerationPolicy, Keyframe,
    MaskArtifactKind, MaskArtifactProfile, MaskQualityScorecard, MaskReviewDecision, MaskSidecar,
    MatteGenerationFallback, MatteGenerationOutput, MatteGenerationRecipe, MatteGenerationSettings,
    MotionGraphicsTemplate, MotionPackage, ParameterAnimation, PipelineReadinessReport,
    PreflightCheckKind, ReadinessState, ReframeKeyframe, ReframePath, ReframeSmoothing,
    SegmentationIntent, SegmentationPrompt, SegmentationPromptKind, SegmentationPromptLabel,
    SegmentationPromptPackage, SegmentationRuntimeStatus, SegmentationSessionOperation,
    SegmentationSessionOperationKind, SelectDecision, SourceRange, SourceSelect,
    StreamExportContract, StreamExportMode, StreamExportSpec, StreamKind, Stringout, TemplateSlot,
    TrackingPackage, WorkflowLens,
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
        motion_packages: vec![MotionPackage {
            id: "motion-package-a".into(),
            intent: "adds lower-third fade".into(),
            affected_clips: vec!["clip-a".into()],
            generated_animations: vec![ParameterAnimation {
                id: "motion-package-a-opacity".into(),
                target: awidat_proto::professional::AnimationTarget::ClipParameter {
                    clip_id: "clip-a".into(),
                    parameter: "title.opacity".into(),
                },
                keyframes: vec![Keyframe::linear(0.0, 0.0), Keyframe::linear(1.0, 1.0)],
                rationale: None,
            }],
            ..MotionPackage::default()
        }],
        expression_links: vec![ExpressionLink {
            id: "expr-audio-scale".into(),
            target_clip_id: "clip-a".into(),
            target_parameter: "overlay.scale".into(),
            source: ExpressionSource::Signal {
                signal: "audio_energy".into(),
            },
            expression: "1 + source * 0.2".into(),
            ..ExpressionLink::default()
        }],
        tracking_package: Some(TrackingPackage {
            masks: vec![MaskSidecar {
                id: "mask-a".into(),
                track_id: Some("track-a".into()),
                attached_clip_id: Some("clip-a".into()),
                ..MaskSidecar::default()
            }],
            ..TrackingPackage::default()
        }),
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
    assert_eq!(roundtrip.motion_packages[0].id, "motion-package-a");
    assert_eq!(roundtrip.motion_packages[0].affected_clips, vec!["clip-a"]);
    assert_eq!(roundtrip.expression_links[0].id, "expr-audio-scale");
    assert_eq!(roundtrip.expression_links[0].target_clip_id, "clip-a");
    let tracking_package = match roundtrip.tracking_package {
        Some(package) => package,
        None => panic!("tracking package missing"),
    };
    assert_eq!(
        tracking_package.masks[0].track_id.as_deref(),
        Some("track-a")
    );
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
fn timeline_metadata_carries_durable_beat_markers() {
    let metadata = AwidatTimelineMetadata {
        beat_markers: vec![
            BeatMarker {
                id: "beat-001".into(),
                time_s: 1.25,
                role: BeatMarkerRole::Downbeat,
                bar: Some(1),
                beat: Some(1),
                tempo_bpm: Some(118.0),
                confidence: Some(0.92),
                strength: Some(0.81),
                source: "audio-energy".into(),
                source_ref: Some("analysis/audio-energy.json#beats/0".into()),
                selection_reason: Some("strong opening accent".into()),
            },
            BeatMarker {
                id: "beat-002".into(),
                time_s: 1.76,
                role: BeatMarkerRole::CutCandidate,
                bar: Some(1),
                beat: Some(2),
                tempo_bpm: Some(118.0),
                confidence: Some(0.88),
                strength: Some(0.76),
                source: "audio-energy".into(),
                source_ref: Some("analysis/audio-energy.json#beats/1".into()),
                selection_reason: Some("keeps cut cadence on musical pulse".into()),
            },
        ],
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

    assert_eq!(roundtrip.beat_markers.len(), 2);
    assert_eq!(roundtrip.beat_markers[0].role, BeatMarkerRole::Downbeat);
    assert_eq!(
        roundtrip.beat_markers[1].source_ref.as_deref(),
        Some("analysis/audio-energy.json#beats/1")
    );
    assert!(roundtrip.validate_professional_substrate().is_empty());
}

#[test]
fn invalid_beat_markers_block_timeline_readiness() {
    let metadata = AwidatTimelineMetadata {
        beat_markers: vec![
            BeatMarker {
                id: "beat-001".into(),
                time_s: 2.0,
                role: BeatMarkerRole::Beat,
                confidence: Some(1.2),
                strength: Some(-0.1),
                source: String::new(),
                ..BeatMarker::default()
            },
            BeatMarker {
                id: "beat-001".into(),
                time_s: 1.5,
                role: BeatMarkerRole::Beat,
                bar: Some(0),
                beat: Some(0),
                tempo_bpm: Some(0.0),
                source: "audio-energy".into(),
                ..BeatMarker::default()
            },
        ],
        ..AwidatTimelineMetadata::default()
    };

    let diagnostics = metadata.validate_professional_substrate();
    assert!(
        diagnostics
            .iter()
            .all(|diagnostic| diagnostic.area == CapabilityArea::AssemblyAndTimelineOperations)
    );
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("duplicate beat marker id"))
    );
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("time_s must be sorted"))
    );
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("source is required"))
    );
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("confidence"))
    );
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("strength"))
    );
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("tempo_bpm"))
    );
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("bar must be positive"))
    );
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("beat must be positive"))
    );

    let report = metadata.build_professional_readiness_report();
    assert_eq!(
        report.stage(CapabilityArea::AssemblyAndTimelineOperations),
        Some(ReadinessState::Blocked)
    );
}

#[test]
fn expression_link_dependency_cycle_is_reported_once() {
    let metadata = AwidatTimelineMetadata {
        expression_links: vec![
            ExpressionLink {
                id: "expr-a".into(),
                target_clip_id: "clip-a".into(),
                target_parameter: "overlay.scale".into(),
                source: ExpressionSource::Parameter {
                    clip_id: "clip-b".into(),
                    parameter: "overlay.opacity".into(),
                },
                expression: "source".into(),
                ..ExpressionLink::default()
            },
            ExpressionLink {
                id: "expr-b".into(),
                target_clip_id: "clip-b".into(),
                target_parameter: "overlay.opacity".into(),
                source: ExpressionSource::Parameter {
                    clip_id: "clip-a".into(),
                    parameter: "overlay.scale".into(),
                },
                expression: "source".into(),
                ..ExpressionLink::default()
            },
        ],
        ..AwidatTimelineMetadata::default()
    };

    let diagnostics = metadata.validate_professional_substrate();
    let cycle_diagnostics: Vec<_> = diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.message.contains("expression link cycle"))
        .collect();

    assert_eq!(cycle_diagnostics.len(), 1);
    assert_eq!(
        cycle_diagnostics[0].area,
        CapabilityArea::ParameterAnimation
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
fn tracking_package_validates_reframe_paths() {
    let package = TrackingPackage {
        reframe_paths: vec![ReframePath {
            id: "reframe-bad".into(),
            clip_id: "clip-a".into(),
            aspect_ratio: "9:16".into(),
            source_width: 1920,
            source_height: 1080,
            target_width: 1080,
            target_height: 1920,
            keyframes: vec![
                ReframeKeyframe {
                    time_s: 2.0,
                    center: [0.5, 0.5],
                    scale: 1.2,
                    confidence: Some(0.8),
                },
                ReframeKeyframe {
                    time_s: 1.0,
                    center: [1.2, 0.5],
                    scale: 0.4,
                    confidence: Some(1.2),
                },
            ],
            smoothing: ReframeSmoothing::Moderate,
            evidence_track_id: Some("track-speaker".into()),
            safe_area: Some("mobile".into()),
        }],
        ..TrackingPackage::default()
    };

    let diagnostics = package.validate();
    let messages: Vec<&str> = diagnostics
        .iter()
        .map(|diagnostic| diagnostic.message.as_str())
        .collect();

    assert!(
        messages
            .iter()
            .any(|message| message.contains("reframe-bad") && message.contains("sorted"))
    );
    assert!(
        messages
            .iter()
            .any(|message| message.contains("center") && message.contains("0..=1"))
    );
    assert!(
        messages
            .iter()
            .any(|message| message.contains("scale") && message.contains(">= 1"))
    );
    assert!(
        messages
            .iter()
            .any(|message| message.contains("confidence"))
    );
}

#[test]
fn tracking_package_validates_segmentation_prompt_packages() {
    let package = TrackingPackage {
        prompt_packages: vec![SegmentationPromptPackage {
            id: "seg-main-speaker".into(),
            clip_id: "clip-a".into(),
            range: Some(SourceRange {
                start_s: 2.0,
                end_s: 5.0,
            }),
            target_object_id: "speaker".into(),
            subject_label: Some("main speaker".into()),
            intent: SegmentationIntent::SubjectMatte,
            prompts: vec![
                SegmentationPrompt {
                    frame: 12,
                    kind: SegmentationPromptKind::Point,
                    label: SegmentationPromptLabel::Positive,
                    points: vec![[0.42, 0.31]],
                    ..SegmentationPrompt::default()
                },
                SegmentationPrompt {
                    frame: 12,
                    kind: SegmentationPromptKind::Box,
                    label: SegmentationPromptLabel::Positive,
                    box_xyxy: Some([0.2, 0.1, 0.8, 0.9]),
                    ..SegmentationPrompt::default()
                },
            ],
            grounding: None,
            output_mask_id: Some("mask-speaker".into()),
            output_matte_id: Some("matte-speaker".into()),
            status: awidat_proto::professional::ReviewStatus::Accepted,
        }],
        ..TrackingPackage::default()
    };

    let json = match serde_json::to_string(&package) {
        Ok(json) => json,
        Err(error) => panic!("serialize prompt package: {error}"),
    };
    let roundtrip: TrackingPackage = match serde_json::from_str(&json) {
        Ok(package) => package,
        Err(error) => panic!("deserialize prompt package: {error}"),
    };
    assert_eq!(roundtrip.prompt_packages[0].target_object_id, "speaker");
    assert_eq!(roundtrip.prompt_packages[0].prompts.len(), 2);

    assert!(roundtrip.validate().is_empty());

    let invalid = TrackingPackage {
        prompt_packages: vec![SegmentationPromptPackage {
            id: "".into(),
            clip_id: "".into(),
            target_object_id: "".into(),
            prompts: vec![
                SegmentationPrompt {
                    frame: 3,
                    kind: SegmentationPromptKind::Point,
                    label: SegmentationPromptLabel::Positive,
                    points: vec![[1.4, 0.2]],
                    ..SegmentationPrompt::default()
                },
                SegmentationPrompt {
                    frame: 2,
                    kind: SegmentationPromptKind::Box,
                    label: SegmentationPromptLabel::Negative,
                    box_xyxy: Some([0.8, 0.1, 0.2, 0.9]),
                    ..SegmentationPrompt::default()
                },
            ],
            ..SegmentationPromptPackage::default()
        }],
        ..TrackingPackage::default()
    };

    let messages: Vec<String> = invalid
        .validate()
        .into_iter()
        .map(|diagnostic| diagnostic.message)
        .collect();

    assert!(messages.iter().any(|message| message.contains("empty id")));
    assert!(
        messages
            .iter()
            .any(|message| message.contains("empty clip id"))
    );
    assert!(
        messages
            .iter()
            .any(|message| message.contains("target object"))
    );
    assert!(messages.iter().any(|message| message.contains("sorted")));
    assert!(messages.iter().any(|message| message.contains("point")));
    assert!(messages.iter().any(|message| message.contains("box")));
}

#[test]
fn tracking_package_validates_grounding_evidence() {
    let package = TrackingPackage {
        prompt_packages: vec![SegmentationPromptPackage {
            id: "seg-main-speaker".into(),
            clip_id: "clip-a".into(),
            target_object_id: "speaker".into(),
            subject_label: Some("main speaker".into()),
            intent: SegmentationIntent::TextBehindSubject,
            grounding: Some(GroundingEvidence {
                text_query: "main speaker.".into(),
                source: "local_detector_v1".into(),
                box_format: GroundingBoxFormat::XyxyPixels,
                image_width: Some(1920),
                image_height: Some(1080),
                box_threshold: Some(0.25),
                text_threshold: Some(0.3),
                status: GroundingEvidenceStatus::AcceptedDetections,
                status_reason: None,
                detections: vec![GroundingDetection {
                    class_name: "main speaker".into(),
                    bbox_xyxy: [240.0, 80.0, 1420.0, 1040.0],
                    score: Some(0.87),
                    frame: Some(12),
                    mask_ref: Some("generated/masks/speaker-001.rle".into()),
                }],
            }),
            prompts: vec![SegmentationPrompt {
                frame: 12,
                kind: SegmentationPromptKind::Box,
                label: SegmentationPromptLabel::Positive,
                box_xyxy: Some([0.125, 0.074, 0.74, 0.963]),
                ..SegmentationPrompt::default()
            }],
            ..SegmentationPromptPackage::default()
        }],
        ..TrackingPackage::default()
    };

    let json = match serde_json::to_string(&package) {
        Ok(json) => json,
        Err(error) => panic!("serialize grounding evidence: {error}"),
    };
    let roundtrip: TrackingPackage = match serde_json::from_str(&json) {
        Ok(package) => package,
        Err(error) => panic!("deserialize grounding evidence: {error}"),
    };

    let grounding = match roundtrip.prompt_packages[0].grounding.as_ref() {
        Some(grounding) => grounding,
        None => panic!("grounding evidence should roundtrip"),
    };
    assert_eq!(grounding.detections[0].class_name, "main speaker");
    assert_eq!(grounding.detections[0].score, Some(0.87));
    assert!(roundtrip.validate().is_empty());

    let invalid = TrackingPackage {
        prompt_packages: vec![SegmentationPromptPackage {
            id: "seg-bad".into(),
            clip_id: "clip-a".into(),
            target_object_id: "speaker".into(),
            grounding: Some(GroundingEvidence {
                text_query: "Main Speaker".into(),
                source: "".into(),
                box_format: GroundingBoxFormat::XyxyNormalized,
                image_width: Some(0),
                image_height: Some(1080),
                box_threshold: Some(1.2),
                text_threshold: Some(f64::NAN),
                status: GroundingEvidenceStatus::NotEvaluated,
                status_reason: None,
                detections: vec![GroundingDetection {
                    class_name: "".into(),
                    bbox_xyxy: [0.8, 0.1, 0.2, 0.9],
                    score: Some(-0.1),
                    frame: None,
                    mask_ref: Some(" ".into()),
                }],
            }),
            prompts: vec![SegmentationPrompt {
                frame: 12,
                kind: SegmentationPromptKind::Box,
                label: SegmentationPromptLabel::Positive,
                box_xyxy: Some([0.1, 0.1, 0.4, 0.4]),
                ..SegmentationPrompt::default()
            }],
            ..SegmentationPromptPackage::default()
        }],
        ..TrackingPackage::default()
    };

    let diagnostics = invalid.validate();
    let messages: Vec<String> = diagnostics
        .iter()
        .map(|diagnostic| diagnostic.message.clone())
        .collect();

    assert!(messages.iter().any(|message| message.contains("lowercase")));
    assert!(
        messages
            .iter()
            .any(|message| message.contains("empty grounding source"))
    );
    assert!(messages.iter().any(|message| message.contains("width")));
    assert!(
        messages
            .iter()
            .any(|message| message.contains("box threshold"))
    );
    assert!(
        messages
            .iter()
            .any(|message| message.contains("text threshold"))
    );
    assert!(
        messages
            .iter()
            .any(|message| message.contains("class name"))
    );
    assert!(messages.iter().any(|message| message.contains("bbox")));
    assert!(messages.iter().any(|message| message.contains("score")));
    assert!(messages.iter().any(|message| message.contains("mask ref")));
}

#[test]
fn tracking_package_distinguishes_grounding_evidence_statuses() {
    let package = TrackingPackage {
        prompt_packages: vec![
            SegmentationPromptPackage {
                id: "seg-accepted".into(),
                clip_id: "clip-a".into(),
                target_object_id: "speaker".into(),
                grounding: Some(GroundingEvidence {
                    text_query: "main speaker.".into(),
                    source: "local_detector_v1".into(),
                    status: GroundingEvidenceStatus::AcceptedDetections,
                    detections: vec![GroundingDetection {
                        class_name: "main speaker".into(),
                        bbox_xyxy: [0.1, 0.1, 0.8, 0.9],
                        score: Some(0.91),
                        ..GroundingDetection::default()
                    }],
                    ..GroundingEvidence::default()
                }),
                prompts: vec![SegmentationPrompt {
                    frame: 0,
                    kind: SegmentationPromptKind::Box,
                    box_xyxy: Some([0.1, 0.1, 0.8, 0.9]),
                    ..SegmentationPrompt::default()
                }],
                ..SegmentationPromptPackage::default()
            },
            SegmentationPromptPackage {
                id: "seg-none".into(),
                clip_id: "clip-b".into(),
                target_object_id: "speaker".into(),
                grounding: Some(GroundingEvidence {
                    text_query: "main speaker.".into(),
                    source: "local_detector_v1".into(),
                    status: GroundingEvidenceStatus::NoDetections,
                    status_reason: Some("detector returned no boxes above threshold".into()),
                    detections: Vec::new(),
                    ..GroundingEvidence::default()
                }),
                prompts: vec![SegmentationPrompt {
                    frame: 0,
                    kind: SegmentationPromptKind::Box,
                    box_xyxy: Some([0.1, 0.1, 0.8, 0.9]),
                    ..SegmentationPrompt::default()
                }],
                ..SegmentationPromptPackage::default()
            },
            SegmentationPromptPackage {
                id: "seg-low".into(),
                clip_id: "clip-c".into(),
                target_object_id: "speaker".into(),
                grounding: Some(GroundingEvidence {
                    text_query: "main speaker.".into(),
                    source: "local_detector_v1".into(),
                    status: GroundingEvidenceStatus::LowConfidence,
                    status_reason: Some("best score 0.18 is below review threshold".into()),
                    detections: vec![GroundingDetection {
                        class_name: "main speaker".into(),
                        bbox_xyxy: [0.1, 0.1, 0.8, 0.9],
                        score: Some(0.18),
                        ..GroundingDetection::default()
                    }],
                    ..GroundingEvidence::default()
                }),
                prompts: vec![SegmentationPrompt {
                    frame: 0,
                    kind: SegmentationPromptKind::Box,
                    box_xyxy: Some([0.1, 0.1, 0.8, 0.9]),
                    ..SegmentationPrompt::default()
                }],
                ..SegmentationPromptPackage::default()
            },
            SegmentationPromptPackage {
                id: "seg-missing".into(),
                clip_id: "clip-d".into(),
                target_object_id: "speaker".into(),
                grounding: Some(GroundingEvidence {
                    text_query: "main speaker.".into(),
                    source: "local_detector_v1".into(),
                    status: GroundingEvidenceStatus::MissingRuntimeEvidence,
                    status_reason: Some("detector runtime was not available".into()),
                    detections: Vec::new(),
                    ..GroundingEvidence::default()
                }),
                prompts: vec![SegmentationPrompt {
                    frame: 0,
                    kind: SegmentationPromptKind::Box,
                    box_xyxy: Some([0.1, 0.1, 0.8, 0.9]),
                    ..SegmentationPrompt::default()
                }],
                ..SegmentationPromptPackage::default()
            },
        ],
        ..TrackingPackage::default()
    };

    let json = match serde_json::to_string(&package) {
        Ok(json) => json,
        Err(error) => panic!("serialize grounding statuses: {error}"),
    };
    assert!(json.contains("accepted_detections"));
    assert!(json.contains("no_detections"));
    assert!(json.contains("low_confidence"));
    assert!(json.contains("missing_runtime_evidence"));

    let roundtrip: TrackingPackage = match serde_json::from_str(&json) {
        Ok(package) => package,
        Err(error) => panic!("deserialize grounding statuses: {error}"),
    };
    assert!(roundtrip.validate().is_empty());

    let invalid = TrackingPackage {
        prompt_packages: vec![
            SegmentationPromptPackage {
                id: "seg-accepted-empty".into(),
                clip_id: "clip-a".into(),
                target_object_id: "speaker".into(),
                grounding: Some(GroundingEvidence {
                    text_query: "main speaker.".into(),
                    source: "local_detector_v1".into(),
                    status: GroundingEvidenceStatus::AcceptedDetections,
                    detections: Vec::new(),
                    ..GroundingEvidence::default()
                }),
                ..SegmentationPromptPackage::default()
            },
            SegmentationPromptPackage {
                id: "seg-missing-detail".into(),
                clip_id: "clip-b".into(),
                target_object_id: "speaker".into(),
                grounding: Some(GroundingEvidence {
                    text_query: "main speaker.".into(),
                    source: "local_detector_v1".into(),
                    status: GroundingEvidenceStatus::MissingRuntimeEvidence,
                    detections: Vec::new(),
                    ..GroundingEvidence::default()
                }),
                ..SegmentationPromptPackage::default()
            },
            SegmentationPromptPackage {
                id: "seg-none-with-box".into(),
                clip_id: "clip-c".into(),
                target_object_id: "speaker".into(),
                grounding: Some(GroundingEvidence {
                    text_query: "main speaker.".into(),
                    source: "local_detector_v1".into(),
                    status: GroundingEvidenceStatus::NoDetections,
                    status_reason: Some("no boxes".into()),
                    detections: vec![GroundingDetection {
                        class_name: "main speaker".into(),
                        bbox_xyxy: [0.1, 0.1, 0.8, 0.9],
                        ..GroundingDetection::default()
                    }],
                    ..GroundingEvidence::default()
                }),
                ..SegmentationPromptPackage::default()
            },
        ],
        ..TrackingPackage::default()
    };

    let messages: Vec<String> = invalid
        .validate()
        .into_iter()
        .map(|diagnostic| diagnostic.message)
        .collect();

    assert!(
        messages
            .iter()
            .any(|message| message.contains("accepted detections"))
    );
    assert!(
        messages
            .iter()
            .any(|message| message.contains("missing runtime evidence"))
    );
    assert!(
        messages
            .iter()
            .any(|message| message.contains("no detections"))
    );
}

#[test]
fn tracking_package_validates_mask_quality_scorecards() {
    let package = TrackingPackage {
        masks: vec![MaskSidecar {
            id: "mask-speaker".into(),
            quality: Some(MaskQualityScorecard {
                bbox_xyxy: Some([0.2, 0.1, 0.8, 0.9]),
                area_px: Some(120_000),
                coverage: Some(0.24),
                stability: Some(0.92),
                confidence: Some(0.88),
                overlap_conflict: Some(false),
                frame_coverage: Some(0.96),
                decision: MaskReviewDecision::Accepted,
                notes: vec!["clean silhouette".into()],
            }),
            ..MaskSidecar::default()
        }],
        mattes: vec![awidat_proto::professional::MatteSidecar {
            id: "matte-speaker".into(),
            alpha_source: "generated/mattes/speaker-alpha.webm".into(),
            quality: Some(MaskQualityScorecard {
                bbox_xyxy: Some([0.22, 0.1, 0.79, 0.92]),
                area_px: Some(118_000),
                coverage: Some(0.23),
                stability: Some(0.9),
                confidence: Some(0.86),
                overlap_conflict: Some(false),
                frame_coverage: Some(0.94),
                decision: MaskReviewDecision::NeedsReview,
                notes: vec!["minor hair edge shimmer".into()],
            }),
            ..awidat_proto::professional::MatteSidecar::default()
        }],
        ..TrackingPackage::default()
    };

    let json = match serde_json::to_string(&package) {
        Ok(json) => json,
        Err(error) => panic!("serialize mask quality: {error}"),
    };
    let roundtrip: TrackingPackage = match serde_json::from_str(&json) {
        Ok(package) => package,
        Err(error) => panic!("deserialize mask quality: {error}"),
    };

    assert_eq!(
        roundtrip
            .masks
            .first()
            .and_then(|mask| mask.quality.as_ref())
            .map(|quality| quality.decision),
        Some(MaskReviewDecision::Accepted)
    );
    assert!(roundtrip.validate().is_empty());

    let invalid = TrackingPackage {
        masks: vec![MaskSidecar {
            id: "mask-bad".into(),
            quality: Some(MaskQualityScorecard {
                bbox_xyxy: Some([0.8, 0.1, 0.2, 0.9]),
                area_px: Some(0),
                coverage: Some(1.4),
                stability: Some(-0.1),
                confidence: Some(1.2),
                overlap_conflict: Some(true),
                frame_coverage: Some(f64::NAN),
                decision: MaskReviewDecision::Rejected,
                notes: Vec::new(),
            }),
            ..MaskSidecar::default()
        }],
        mattes: vec![awidat_proto::professional::MatteSidecar {
            id: "matte-bad".into(),
            alpha_source: "".into(),
            quality: Some(MaskQualityScorecard {
                bbox_xyxy: Some([0.0, 0.0, 1.0, 1.0]),
                area_px: Some(12),
                coverage: Some(0.2),
                stability: Some(0.1),
                confidence: Some(0.2),
                overlap_conflict: Some(false),
                frame_coverage: Some(0.0),
                decision: MaskReviewDecision::Rejected,
                notes: Vec::new(),
            }),
            ..awidat_proto::professional::MatteSidecar::default()
        }],
        ..TrackingPackage::default()
    };

    let messages: Vec<String> = invalid
        .validate()
        .into_iter()
        .map(|diagnostic| diagnostic.message)
        .collect();

    assert!(messages.iter().any(|message| message.contains("bbox")));
    assert!(messages.iter().any(|message| message.contains("area")));
    assert!(messages.iter().any(|message| message.contains("coverage")));
    assert!(messages.iter().any(|message| message.contains("stability")));
    assert!(
        messages
            .iter()
            .any(|message| message.contains("confidence"))
    );
    assert!(
        messages
            .iter()
            .any(|message| message.contains("frame coverage"))
    );
    assert!(messages.iter().any(|message| message.contains("conflict")));
    assert!(
        messages
            .iter()
            .any(|message| message.contains("alpha source"))
    );
}

#[test]
fn tracking_package_validates_matte_generation_recipe() {
    let package = TrackingPackage {
        mattes: vec![awidat_proto::professional::MatteSidecar {
            id: "matte-speaker".into(),
            alpha_source: "generated/mattes/speaker-alpha.png".into(),
            generation: Some(MatteGenerationRecipe {
                source: "local_matte_tool".into(),
                model: Some("portrait-v1".into()),
                output: MatteGenerationOutput::AlphaMatte,
                settings: MatteGenerationSettings {
                    alpha_matting: true,
                    foreground_threshold: Some(240),
                    background_threshold: Some(10),
                    erode_size: Some(12),
                    post_process_mask: true,
                    background_color_rgba: Some([0, 0, 0, 0]),
                    fallback: Some(MatteGenerationFallback::SimpleCutout),
                },
                options: BTreeMap::from([("sam_prompt".into(), serde_json::json!("speaker"))]),
            }),
            ..awidat_proto::professional::MatteSidecar::default()
        }],
        ..TrackingPackage::default()
    };

    let json = match serde_json::to_string(&package) {
        Ok(json) => json,
        Err(error) => panic!("serialize matte generation: {error}"),
    };
    let roundtrip: TrackingPackage = match serde_json::from_str(&json) {
        Ok(package) => package,
        Err(error) => panic!("deserialize matte generation: {error}"),
    };

    let generation = match roundtrip.mattes[0].generation.as_ref() {
        Some(generation) => generation,
        None => panic!("matte generation recipe should roundtrip"),
    };
    assert_eq!(generation.model.as_deref(), Some("portrait-v1"));
    assert_eq!(
        generation.settings.fallback,
        Some(MatteGenerationFallback::SimpleCutout)
    );
    assert!(roundtrip.validate().is_empty());

    let invalid = TrackingPackage {
        mattes: vec![awidat_proto::professional::MatteSidecar {
            id: "matte-bad".into(),
            alpha_source: "generated/mattes/bad.png".into(),
            generation: Some(MatteGenerationRecipe {
                source: "".into(),
                model: Some(" ".into()),
                output: MatteGenerationOutput::AlphaMatte,
                settings: MatteGenerationSettings {
                    alpha_matting: true,
                    foreground_threshold: Some(300),
                    background_threshold: Some(260),
                    erode_size: Some(0),
                    post_process_mask: false,
                    background_color_rgba: Some([0, 0, 0, 300]),
                    fallback: Some(MatteGenerationFallback::OriginalFrame),
                },
                options: BTreeMap::from([(" ".into(), serde_json::json!(true))]),
            }),
            ..awidat_proto::professional::MatteSidecar::default()
        }],
        ..TrackingPackage::default()
    };

    let messages: Vec<String> = invalid
        .validate()
        .into_iter()
        .map(|diagnostic| diagnostic.message)
        .collect();

    assert!(messages.iter().any(|message| message.contains("source")));
    assert!(messages.iter().any(|message| message.contains("model")));
    assert!(
        messages
            .iter()
            .any(|message| message.contains("foreground threshold"))
    );
    assert!(
        messages
            .iter()
            .any(|message| message.contains("background threshold"))
    );
    assert!(messages.iter().any(|message| message.contains("erode")));
    assert!(
        messages
            .iter()
            .any(|message| message.contains("background color"))
    );
    assert!(messages.iter().any(|message| message.contains("fallback")));
    assert!(
        messages
            .iter()
            .any(|message| message.contains("option key"))
    );
}

#[test]
fn tracking_package_validates_segmentation_session_operations() {
    let package = TrackingPackage {
        session_operations: vec![
            SegmentationSessionOperation {
                id: "op-start".into(),
                session_id: "seg-session-1".into(),
                kind: SegmentationSessionOperationKind::Start,
                frame: Some(12),
                target_object_id: Some("speaker".into()),
                status: SegmentationRuntimeStatus::Ready,
                message: Some("session initialized on cpu".into()),
                ..SegmentationSessionOperation::default()
            },
            SegmentationSessionOperation {
                id: "op-prompt".into(),
                session_id: "seg-session-1".into(),
                kind: SegmentationSessionOperationKind::AddPrompt,
                frame: Some(12),
                target_object_id: Some("speaker".into()),
                prompt_package_id: Some("seg-main-speaker".into()),
                status: SegmentationRuntimeStatus::Complete,
                artifact_refs: vec!["masks/speaker-000012.rle".into()],
                ..SegmentationSessionOperation::default()
            },
            SegmentationSessionOperation {
                id: "op-persist".into(),
                session_id: "seg-session-1".into(),
                kind: SegmentationSessionOperationKind::PersistOutput,
                output_mask_id: Some("mask-speaker".into()),
                status: SegmentationRuntimeStatus::Complete,
                artifact_refs: vec!["masks/speaker-packed.png".into()],
                ..SegmentationSessionOperation::default()
            },
        ],
        ..TrackingPackage::default()
    };

    let json = match serde_json::to_string(&package) {
        Ok(json) => json,
        Err(error) => panic!("serialize segmentation session operations: {error}"),
    };
    let roundtrip: TrackingPackage = match serde_json::from_str(&json) {
        Ok(package) => package,
        Err(error) => panic!("deserialize segmentation session operations: {error}"),
    };
    assert_eq!(
        roundtrip.session_operations[1].kind,
        SegmentationSessionOperationKind::AddPrompt
    );
    assert!(roundtrip.validate().is_empty());

    let invalid = TrackingPackage {
        session_operations: vec![
            SegmentationSessionOperation {
                id: "".into(),
                session_id: "".into(),
                kind: SegmentationSessionOperationKind::AddPrompt,
                status: SegmentationRuntimeStatus::Complete,
                ..SegmentationSessionOperation::default()
            },
            SegmentationSessionOperation {
                id: "op-remove".into(),
                session_id: "seg-session-1".into(),
                kind: SegmentationSessionOperationKind::RemoveObject,
                status: SegmentationRuntimeStatus::Failed,
                ..SegmentationSessionOperation::default()
            },
            SegmentationSessionOperation {
                id: "op-persist".into(),
                session_id: "seg-session-1".into(),
                kind: SegmentationSessionOperationKind::PersistOutput,
                status: SegmentationRuntimeStatus::UnsupportedDevice,
                output_mask_id: Some(" ".into()),
                artifact_refs: vec![],
                ..SegmentationSessionOperation::default()
            },
        ],
        ..TrackingPackage::default()
    };

    let messages: Vec<String> = invalid
        .validate()
        .into_iter()
        .map(|diagnostic| diagnostic.message)
        .collect();

    assert!(messages.iter().any(|message| message.contains("empty id")));
    assert!(
        messages
            .iter()
            .any(|message| message.contains("session id"))
    );
    assert!(
        messages
            .iter()
            .any(|message| message.contains("prompt package"))
    );
    assert!(
        messages
            .iter()
            .any(|message| message.contains("target object"))
    );
    assert!(
        messages
            .iter()
            .any(|message| message.contains("diagnostic message"))
    );
    assert!(
        messages
            .iter()
            .any(|message| message.contains("output mask"))
    );
    assert!(
        messages
            .iter()
            .any(|message| message.contains("artifact ref"))
    );
}

#[test]
fn tracking_package_validates_mask_artifact_profiles() {
    let package = TrackingPackage {
        mask_artifacts: vec![
            MaskArtifactProfile {
                id: "artifact-packed".into(),
                kind: MaskArtifactKind::PackedPng,
                path: "masks/speaker-packed.png".into(),
                object_ids: vec!["speaker".into()],
                frame_range: Some(SourceRange {
                    start_s: 1.0,
                    end_s: 4.0,
                }),
                frame_count: Some(90),
                width: Some(1920),
                height: Some(1080),
                checksum: Some("sha256:abc123".into()),
            },
            MaskArtifactProfile {
                id: "artifact-rle".into(),
                kind: MaskArtifactKind::Rle,
                path: "masks/speaker-000012.rle.json".into(),
                object_ids: vec!["speaker".into()],
                frame_count: Some(1),
                width: Some(1920),
                height: Some(1080),
                checksum: None,
                frame_range: None,
            },
        ],
        ..TrackingPackage::default()
    };

    let json = match serde_json::to_string(&package) {
        Ok(json) => json,
        Err(error) => panic!("serialize mask artifact profiles: {error}"),
    };
    let roundtrip: TrackingPackage = match serde_json::from_str(&json) {
        Ok(package) => package,
        Err(error) => panic!("deserialize mask artifact profiles: {error}"),
    };
    assert_eq!(
        roundtrip.mask_artifacts[0].kind,
        MaskArtifactKind::PackedPng
    );
    assert!(roundtrip.validate().is_empty());

    let invalid = TrackingPackage {
        mask_artifacts: vec![
            MaskArtifactProfile {
                id: "".into(),
                kind: MaskArtifactKind::PerObjectPng,
                path: "".into(),
                object_ids: vec![" ".into()],
                frame_count: Some(0),
                width: Some(0),
                height: Some(1080),
                checksum: Some(" ".into()),
                frame_range: Some(SourceRange {
                    start_s: 4.0,
                    end_s: 1.0,
                }),
            },
            MaskArtifactProfile {
                id: "artifact-binary".into(),
                kind: MaskArtifactKind::BinaryMask,
                path: "masks/binary.bin".into(),
                object_ids: vec![],
                ..MaskArtifactProfile::default()
            },
        ],
        ..TrackingPackage::default()
    };

    let messages: Vec<String> = invalid
        .validate()
        .into_iter()
        .map(|diagnostic| diagnostic.message)
        .collect();

    assert!(
        messages
            .iter()
            .any(|message| message.contains("artifact") && message.contains("empty id"))
    );
    assert!(messages.iter().any(|message| message.contains("path")));
    assert!(messages.iter().any(|message| message.contains("object id")));
    assert!(
        messages
            .iter()
            .any(|message| message.contains("frame count"))
    );
    assert!(messages.iter().any(|message| message.contains("width")));
    assert!(messages.iter().any(|message| message.contains("range")));
    assert!(messages.iter().any(|message| message.contains("checksum")));
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
fn delivery_profile_preflight_flags_missing_required_measurements() {
    let profile = DeliveryProfile {
        id: "strict".into(),
        name: "Strict".into(),
        aspect_ratio: "16:9".into(),
        width: 1920,
        height: 1080,
        video_bitrate_kbps: Some(12_000),
        loudness_lufs: Some(-14.0),
        preflight_checks: vec![
            PreflightCheckKind::Bitrate,
            PreflightCheckKind::Loudness,
            PreflightCheckKind::Duration,
        ],
        ..DeliveryProfile::default()
    };

    let report = profile.run_preflight(DeliveryPreflightInput {
        aspect_ratio: "16:9".into(),
        ..DeliveryPreflightInput::default()
    });

    assert!(
        report
            .findings
            .iter()
            .any(|finding| finding.check == PreflightCheckKind::Bitrate
                && finding.severity == FindingSeverity::Error
                && finding.message.contains("measurement is missing"))
    );
    assert!(
        report
            .findings
            .iter()
            .any(|finding| finding.check == PreflightCheckKind::Loudness
                && finding.severity == FindingSeverity::Error
                && finding.message.contains("measurement is missing"))
    );
    assert!(
        report
            .findings
            .iter()
            .any(|finding| finding.check == PreflightCheckKind::Duration
                && finding.severity == FindingSeverity::Error
                && finding.message.contains("measurement is missing"))
    );
}

#[test]
fn export_presets_validate_social_audio_and_image_sequence_targets() {
    let short = ExportPreset::vertical_short_form();
    let audio = ExportPreset::podcast_audio();
    let frames = ExportPreset::image_sequence();

    assert!(short.validate().is_empty());
    assert!(audio.validate().is_empty());
    assert!(frames.validate().is_empty());
    assert_eq!(short.profile.aspect_ratio, "9:16");
    assert_eq!(short.output.extension, "mp4");
    assert_eq!(audio.mode, ExportMode::AudioOnly);
    assert!(audio.video.is_none());
    assert_eq!(frames.mode, ExportMode::ImageSequence);
    assert!(frames.audio.is_none());
}

#[test]
fn stream_export_contract_validates_copy_transcode_and_dispositions() {
    let contract = StreamExportContract {
        id: "stream-master".into(),
        container: "mp4".into(),
        streams: vec![
            StreamExportSpec {
                id: "v-main".into(),
                kind: StreamKind::Video,
                source_index: 0,
                mode: StreamExportMode::Copy,
                disposition: vec!["default".into()],
                ..StreamExportSpec::default()
            },
            StreamExportSpec {
                id: "a-main".into(),
                kind: StreamKind::Audio,
                source_index: 1,
                mode: StreamExportMode::Transcode,
                codec: Some("aac".into()),
                language: Some("en".into()),
                disposition: vec!["default".into()],
                ..StreamExportSpec::default()
            },
        ],
        ..StreamExportContract::default()
    };

    assert!(contract.validate().is_empty());

    let invalid = StreamExportContract {
        id: "bad-stream-master".into(),
        container: "mp4".into(),
        streams: vec![StreamExportSpec {
            id: "a-bad".into(),
            kind: StreamKind::Audio,
            source_index: 0,
            mode: StreamExportMode::Transcode,
            disposition: vec!["loud".into()],
            ..StreamExportSpec::default()
        }],
        ..StreamExportContract::default()
    };

    let diagnostics = invalid.validate();
    assert!(diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("transcode stream a-bad needs codec")
    }));
    assert!(diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("stream a-bad has unsupported disposition loud")
    }));
}

#[test]
fn export_preset_validation_flags_codec_range_and_ratio_mismatches() {
    let preset = ExportPreset {
        id: String::new(),
        name: "Broken".into(),
        mode: ExportMode::AudioVideo,
        profile: DeliveryProfile {
            id: "bad_profile".into(),
            name: "Bad Profile".into(),
            aspect_ratio: "9:16".into(),
            width: 1920,
            height: 1080,
            ..DeliveryProfile::default()
        },
        output: ExportOutputSettings {
            extension: "mov".into(),
            container: String::new(),
            hardware_acceleration: HardwareAccelerationPolicy::Auto,
        },
        video: None,
        audio: None,
        range: ExportRange {
            start_s: Some(10.0),
            end_s: Some(4.0),
            ..ExportRange::default()
        },
    };

    let messages: Vec<String> = preset
        .validate()
        .into_iter()
        .map(|diagnostic| diagnostic.message)
        .collect();

    assert!(messages.iter().any(|message| message.contains("empty id")));
    assert!(
        messages
            .iter()
            .any(|message| message.contains("aspect ratio"))
    );
    assert!(messages.iter().any(|message| message.contains("container")));
    assert!(
        messages
            .iter()
            .any(|message| message.contains("video settings"))
    );
    assert!(
        messages
            .iter()
            .any(|message| message.contains("audio settings"))
    );
    assert!(
        messages
            .iter()
            .any(|message| message.contains("end_s must be greater"))
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

#[test]
fn metadata_readiness_blocks_stages_with_validation_errors() {
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
                start_s: 3.0,
                end_s: 1.0,
            },
            ..SourceSelect::default()
        }],
        stringouts: vec![Stringout {
            id: "stringout-a".into(),
            select_ids: vec!["select-bad".into()],
            ..Stringout::default()
        }],
        parameter_animations: vec![ParameterAnimation {
            id: "anim-bad".into(),
            keyframes: vec![Keyframe::linear(2.0, 1.0), Keyframe::linear(1.0, 0.0)],
            ..ParameterAnimation::default()
        }],
        ..AwidatTimelineMetadata::default()
    };

    let report = metadata.build_professional_readiness_report();

    assert_eq!(
        report.stage(CapabilityArea::SourceReviewSelects),
        Some(ReadinessState::Blocked)
    );
    assert_eq!(
        report.stage(CapabilityArea::ParameterAnimation),
        Some(ReadinessState::Blocked)
    );
    assert!(
        report
            .blockers
            .iter()
            .any(|blocker| blocker.contains("missing-asset"))
    );
    assert!(
        report
            .blockers
            .iter()
            .any(|blocker| blocker.contains("anim-bad"))
    );
}

#[test]
fn duplicate_parameter_animations_report_one_deterministic_conflict() {
    let metadata = AwidatTimelineMetadata {
        parameter_animations: vec![
            ParameterAnimation {
                id: "anim-a".into(),
                target: awidat_proto::professional::AnimationTarget::ClipParameter {
                    clip_id: "clip-a".into(),
                    parameter: "title.opacity".into(),
                },
                keyframes: vec![Keyframe::linear(0.0, 0.0), Keyframe::linear(1.0, 1.0)],
                rationale: None,
            },
            ParameterAnimation {
                id: "anim-b".into(),
                target: awidat_proto::professional::AnimationTarget::ClipParameter {
                    clip_id: "clip-a".into(),
                    parameter: "title.opacity".into(),
                },
                keyframes: vec![Keyframe::linear(0.0, 1.0), Keyframe::linear(1.0, 0.0)],
                rationale: None,
            },
        ],
        ..AwidatTimelineMetadata::default()
    };

    let diagnostics = metadata.validate_professional_substrate();
    let conflicts: Vec<_> = diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.message.contains("animation conflict"))
        .collect();

    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0].area, CapabilityArea::ParameterAnimation);
    assert_eq!(conflicts[0].severity, FindingSeverity::Error);
    assert!(conflicts[0].message.contains("clip-a/title.opacity"));
    assert!(conflicts[0].message.contains("anim-a"));
    assert!(conflicts[0].message.contains("anim-b"));
}

#[test]
fn parameter_animation_value_validation_rejects_invalid_phase_3a_values() {
    let metadata = AwidatTimelineMetadata {
        parameter_animations: vec![
            ParameterAnimation {
                id: "anim-opacity".into(),
                target: awidat_proto::professional::AnimationTarget::ClipParameter {
                    clip_id: "clip-a".into(),
                    parameter: "overlay.opacity".into(),
                },
                keyframes: vec![Keyframe::linear(0.0, 2.0)],
                rationale: None,
            },
            ParameterAnimation {
                id: "anim-scale".into(),
                target: awidat_proto::professional::AnimationTarget::ClipParameter {
                    clip_id: "clip-a".into(),
                    parameter: "overlay.scale".into(),
                },
                keyframes: vec![Keyframe::linear(0.0, 0.0)],
                rationale: None,
            },
            ParameterAnimation {
                id: "anim-font-size".into(),
                target: awidat_proto::professional::AnimationTarget::ClipParameter {
                    clip_id: "clip-a".into(),
                    parameter: "title.font_size".into(),
                },
                keyframes: vec![Keyframe::linear(0.0, 0.0)],
                rationale: None,
            },
            ParameterAnimation {
                id: "anim-x".into(),
                target: awidat_proto::professional::AnimationTarget::ClipParameter {
                    clip_id: "clip-a".into(),
                    parameter: "overlay.x".into(),
                },
                keyframes: vec![Keyframe::linear(0.0, f64::NAN)],
                rationale: None,
            },
        ],
        ..AwidatTimelineMetadata::default()
    };

    let diagnostics = metadata.validate_professional_substrate();
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("overlay.opacity")
                && diagnostic.message.contains("[0, 1]")),
        "{diagnostics:?}"
    );
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("overlay.scale")
                && diagnostic.message.contains("positive")),
        "{diagnostics:?}"
    );
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("title.font_size")
                && diagnostic.message.contains("positive")),
        "{diagnostics:?}"
    );
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("anim-x")
                && diagnostic.message.contains("non-finite keyframe")),
        "{diagnostics:?}"
    );
}
