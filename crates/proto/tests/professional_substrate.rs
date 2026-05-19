//! Professional substrate schema acceptance tests.

use awidat_proto::awidat_meta::AwidatTimelineMetadata;
use awidat_proto::professional::{
    AssetCatalog, AssetQuery, AssetReadiness, AssetRecord, AssetRole, AudioBus,
    AudioFinishingState, CapabilityArea, CapabilityRegistry, CapabilityStatus, ColorFinishingState,
    CompositionGraph, CompositionNode, CompositionNodeType, DeliveryPreflightInput,
    DeliveryProfile, ExportMode, ExportOutputSettings, ExportPreset, ExportRange, ExpressionLink,
    ExpressionSource, FindingSeverity, GradeStack, HardwareAccelerationPolicy, Keyframe,
    MaskQualityScorecard, MaskReviewDecision, MaskSidecar, MotionGraphicsTemplate, MotionPackage,
    ParameterAnimation, PipelineReadinessReport, PreflightCheckKind, ReadinessState,
    ReframeKeyframe, ReframePath, ReframeSmoothing, SegmentationIntent, SegmentationPrompt,
    SegmentationPromptKind, SegmentationPromptLabel, SegmentationPromptPackage, SelectDecision,
    SourceRange, SourceSelect, Stringout, TemplateSlot, TrackingPackage, WorkflowLens,
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
            .any(|diagnostic| diagnostic.message.contains("anim-x")
                && diagnostic.message.contains("non-finite keyframe")),
        "{diagnostics:?}"
    );
}
