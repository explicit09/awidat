//! Tests for professional render-side engines and lowerers.

use std::collections::BTreeMap;
use std::path::PathBuf;

use awidat_proto::awidat_meta::AwidatTimelineMetadata;
use awidat_proto::professional::{
    AnimationTarget, AudioAutomationLane, AudioBus, AudioChainPreset, AudioFinishingState,
    AudioMeterReading, AudioRole, ColorFinishingState, CompositionGraph, CompositionNode,
    DeliveryPreflightInput, DeliveryProfile, ExportPreset, ExpressionLink, ExpressionSource,
    ExtrapolationMode, FindingSeverity, GradeStack, GradeStage, Keyframe, MaskKeyframe,
    MaskOperation, MaskSidecar, MotionGraphicsTemplate, MotionPackage, ParameterAnimation,
    ReframeKeyframe, ReframePath, ReframeSmoothing, ReviewStatus, SafeAreaRule,
    StreamExportContract, StreamExportMode, StreamExportSpec, StreamKind, TemplateSlot,
    TemplateSlotKind, TrackKind, TrackSample, TrackSidecar, TrackingPackage,
};
use awidat_render::professional::{
    DeliveryQueueRequest, MotionPackageDecision, MotionTemplateTiming, TemplateAnimation,
    TrackCorrection, TrackingEvidence, apply_delivery_profile_to_spec, apply_export_preset_to_spec,
    apply_motion_package, built_in_motion_templates, diagnose_effect_parameter_animation,
    effect_parameter_capability_matrix, evaluate_expression_links, fill_motion_template,
    generate_tracking_package, inspect_composition_graph, lower_audio_finishing,
    lower_composition_graph, lower_grade_stack, lower_motion_template, lower_reframe_path,
    lower_track_bound_overlay, motion_package_summary, plan_delivery_queue_item,
    plan_stream_export_args, summarize_color_finishing,
};
use awidat_render::{RenderJobSpec, TitleAnimation, TitlePosition};
use serde_json::json;

#[test]
fn tracking_engine_generates_confident_sidecar_from_motion_evidence() {
    let package = generate_tracking_package(TrackingEvidence {
        asset_id: "cam-a".into(),
        kind: TrackKind::Point,
        frame_count: 4,
        width: 1920,
        height: 1080,
        motion_signal: vec![0.02, 0.04, 0.03, 0.05],
    });

    let Some(track) = package.tracks.first() else {
        panic!("track sidecar");
    };
    assert_eq!(track.asset_id, "cam-a");
    assert_eq!(track.samples.len(), 4);
    let Some(confidence) = track.confidence else {
        panic!("track confidence");
    };
    assert!(confidence > 0.90);
    assert_eq!(track.samples[0].points[0], [0.5, 0.5]);
    assert!(package.validate().is_empty());
}

#[test]
fn tracking_corrections_replace_samples_and_rescore_quality() {
    let mut package = generate_tracking_package(TrackingEvidence {
        asset_id: "cam-a".into(),
        kind: TrackKind::Surface,
        frame_count: 2,
        width: 100,
        height: 100,
        motion_signal: vec![0.90, 0.95],
    });
    let Some(initial) = package.tracks[0].confidence else {
        panic!("initial confidence");
    };

    if let Err(err) = (TrackCorrection {
        track_id: package.tracks[0].id.clone(),
        samples: vec![
            (
                0,
                vec![[0.1, 0.1], [0.9, 0.1], [0.9, 0.9], [0.1, 0.9]],
                0.99,
            ),
            (
                1,
                vec![[0.2, 0.1], [0.8, 0.1], [0.8, 0.9], [0.2, 0.9]],
                0.98,
            ),
        ],
    })
    .apply(&mut package)
    {
        panic!("correction applies: {err}");
    }

    let Some(corrected) = package.tracks[0].confidence else {
        panic!("corrected confidence");
    };
    assert!(corrected > initial);
    assert_eq!(package.tracks[0].samples[0].points.len(), 4);
}

#[test]
fn tracking_package_validates_sample_order_and_low_confidence() {
    let package = TrackingPackage {
        tracks: vec![TrackSidecar {
            id: "track-a".into(),
            asset_id: "clip-a".into(),
            kind: TrackKind::Point,
            samples: vec![
                TrackSample {
                    frame: 2,
                    points: vec![[0.5, 0.5]],
                    confidence: Some(0.3),
                },
                TrackSample {
                    frame: 1,
                    points: vec![[0.4, 0.4]],
                    confidence: Some(0.4),
                },
            ],
            confidence: Some(0.4),
            ..TrackSidecar::default()
        }],
        ..TrackingPackage::default()
    };

    let diagnostics = package.validate();

    assert!(diagnostics.iter().any(|diagnostic| {
        diagnostic.severity == FindingSeverity::Error
            && diagnostic.message.contains("sample frames must be sorted")
    }));
    assert!(diagnostics.iter().any(|diagnostic| {
        diagnostic.severity == FindingSeverity::Warning
            && diagnostic.message.contains("low confidence")
    }));
}

#[test]
fn track_bound_overlay_lowers_only_when_track_exists() {
    let package = TrackingPackage {
        tracks: vec![TrackSidecar {
            id: "track-a".into(),
            asset_id: "clip-a".into(),
            kind: TrackKind::Point,
            samples: vec![TrackSample {
                frame: 0,
                points: vec![[0.25, 0.75]],
                confidence: Some(0.95),
            }],
            confidence: Some(0.95),
            ..TrackSidecar::default()
        }],
        masks: vec![MaskSidecar {
            id: "mask-a".into(),
            operation: MaskOperation::Add,
            track_id: Some("track-a".into()),
            attached_clip_id: Some("overlay-a".into()),
            keyframes: vec![MaskKeyframe {
                time_s: 0.0,
                points: vec![[0.2, 0.7], [0.3, 0.7], [0.3, 0.8], [0.2, 0.8]],
                feather: 0.02,
                opacity: 1.0,
            }],
            ..MaskSidecar::default()
        }],
        ..TrackingPackage::default()
    };

    let lowering = match lower_track_bound_overlay(&package, "track-a", "overlay-a") {
        Ok(lowering) => lowering,
        Err(err) => panic!("track-bound overlay lowers: {err}"),
    };

    assert_eq!(lowering.track_id, "track-a");
    assert_eq!(lowering.overlay_clip_id, "overlay-a");
    assert!(lowering.expression.contains("x=0.25"));
    assert!(lowering.expression.contains("y=0.75"));
    assert!(lowering.expression.contains("mask-a"));
}

#[test]
fn track_bound_overlay_fails_loudly_when_track_is_missing() {
    let package = TrackingPackage::default();

    let err = match lower_track_bound_overlay(&package, "missing-track", "overlay-a") {
        Ok(_) => panic!("missing track should fail"),
        Err(err) => err,
    };

    assert_eq!(err.to_string(), "track missing-track not found");
}

#[test]
fn reframe_path_lowers_to_deterministic_crop_expression() {
    let package = TrackingPackage {
        reframe_paths: vec![ReframePath {
            id: "vertical-speaker".into(),
            clip_id: "clip-a".into(),
            aspect_ratio: "9:16".into(),
            source_width: 1920,
            source_height: 1080,
            target_width: 1080,
            target_height: 1920,
            keyframes: vec![
                ReframeKeyframe {
                    time_s: 0.0,
                    center: [0.45, 0.5],
                    scale: 1.15,
                    confidence: Some(0.92),
                },
                ReframeKeyframe {
                    time_s: 2.0,
                    center: [0.55, 0.48],
                    scale: 1.25,
                    confidence: Some(0.88),
                },
            ],
            smoothing: ReframeSmoothing::Gentle,
            evidence_track_id: Some("track-speaker".into()),
            safe_area: Some("mobile".into()),
        }],
        ..TrackingPackage::default()
    };

    let lowering = match lower_reframe_path(&package, "vertical-speaker") {
        Ok(lowering) => lowering,
        Err(err) => panic!("reframe path lowers: {err}"),
    };

    assert_eq!(lowering.reframe_id, "vertical-speaker");
    assert_eq!(lowering.clip_id, "clip-a");
    assert_eq!(lowering.aspect_ratio, "9:16");
    assert_eq!(lowering.smoothing, ReframeSmoothing::Gentle);
    assert!(lowering.expression.contains("crop"));
    assert!(lowering.expression.contains("center=0.45,0.5"));
    assert!(lowering.expression.contains("scale=1.15"));
    assert!(lowering.expression.contains("safe_area=mobile"));
}

#[test]
fn expression_link_cycle_is_rejected() {
    let links = vec![
        expression_link(
            "a",
            "clip-a",
            "overlay.scale",
            ExpressionSource::Parameter {
                clip_id: "clip-a".into(),
                parameter: "overlay.opacity".into(),
            },
            "source",
        ),
        expression_link(
            "b",
            "clip-a",
            "overlay.opacity",
            ExpressionSource::Parameter {
                clip_id: "clip-a".into(),
                parameter: "overlay.scale".into(),
            },
            "source",
        ),
    ];

    let evaluation = evaluate_expression_links(&links, &map([]), &map([]), 0.0);

    assert!(evaluation.values.is_empty());
    assert!(evaluation.limitations.iter().any(|limitation| {
        limitation.message.contains("cycle") && limitation.severity == FindingSeverity::Error
    }));
}

#[test]
fn missing_expression_signal_surfaces_limitation() {
    let links = vec![expression_link(
        "scale-audio",
        "clip-a",
        "overlay.scale",
        ExpressionSource::Signal {
            signal: "audio_energy".into(),
        },
        "1 + source * 0.5",
    )];

    let evaluation = evaluate_expression_links(&links, &map([]), &map([]), 0.0);

    assert!(evaluation.values.is_empty());
    assert!(evaluation.limitations.iter().any(|limitation| {
        limitation.message.contains("missing signal audio_energy")
            && limitation.severity == FindingSeverity::Warning
    }));
}

#[test]
fn audio_energy_expression_drives_expected_scale_samples() {
    let links = vec![expression_link(
        "scale-audio",
        "clip-a",
        "overlay.scale",
        ExpressionSource::Signal {
            signal: "audio_energy".into(),
        },
        "clamp(1 + source * 0.5, 1, 1.5)",
    )];
    let signals = map([("audio_energy", json!(0.8))]);

    let evaluation = evaluate_expression_links(&links, &signals, &map([]), 0.0);

    assert!(
        evaluation.limitations.is_empty(),
        "{:?}",
        evaluation.limitations
    );
    assert_eq!(evaluation.values.get("clip-a/overlay.scale"), Some(&1.4));
}

#[test]
fn composition_lowering_emits_supported_filters_and_explicit_limitations() {
    let graph = awidat_proto::professional::CompositionGraph {
        id: "comp-1".into(),
        nodes: vec![
            node("input", "media_input", json!({"asset_id": "clip-a"})),
            node(
                "text",
                "text",
                json!({"text": "Hello", "start_s": 0.0, "end_s": 2.0}),
            ),
            node("blur", "blur", json!({"radius": 6.0})),
            node(
                "tracker",
                "tracker_bind",
                json!({"track_id": "track-cam-a"}),
            ),
            node("output", "output", json!({})),
        ],
        edges: vec![],
        output_node_id: Some("output".into()),
    };

    let lowering = lower_composition_graph(&graph);

    assert!(
        lowering
            .limitations
            .iter()
            .all(|l| l.severity != FindingSeverity::Error)
    );
    assert!(
        lowering
            .steps
            .iter()
            .any(|s| s.expression.contains("drawtext"))
    );
    assert!(
        lowering
            .steps
            .iter()
            .any(|s| s.expression.contains("boxblur"))
    );
    assert!(
        lowering
            .steps
            .iter()
            .any(|s| s.expression.contains("metadata=track-cam-a"))
    );
}

#[test]
fn composition_graph_cycle_fails_validation() {
    let graph = awidat_proto::professional::CompositionGraph {
        id: "cycle-graph".into(),
        nodes: vec![
            node("a", "transform", json!({})),
            node("b", "merge", json!({})),
            node("output", "output", json!({})),
        ],
        edges: vec![edge("a", "b"), edge("b", "a"), edge("b", "output")],
        output_node_id: Some("output".into()),
    };

    let diagnostics = graph.validate();

    assert!(diagnostics.iter().any(|diagnostic| {
        diagnostic.severity == FindingSeverity::Error && diagnostic.message.contains("cycle")
    }));
}

#[test]
fn unsupported_composition_node_persists_with_limitation() {
    let graph = awidat_proto::professional::CompositionGraph {
        id: "unsupported-graph".into(),
        nodes: vec![
            awidat_proto::professional::CompositionNode {
                id: "plugin-node".into(),
                node_type: awidat_proto::professional::CompositionNodeType::Unsupported,
                params: map([("plugin", json!("third-party-glow"))]),
            },
            node("output", "output", json!({})),
        ],
        edges: vec![edge("plugin-node", "output")],
        output_node_id: Some("output".into()),
    };

    let lowering = lower_composition_graph(&graph);

    assert!(lowering.steps.iter().any(|step| step.node_id == "output"));
    assert!(lowering.limitations.iter().any(|limitation| {
        limitation.node_id == "plugin-node"
            && limitation.message.contains("no current render lowering")
    }));
}

#[test]
fn particle_and_scene3d_nodes_persist_with_explicit_limitations() {
    let graph = CompositionGraph {
        id: "particles-graph".into(),
        nodes: vec![
            CompositionNode {
                id: "scene".into(),
                node_type: awidat_proto::professional::CompositionNodeType::Scene3d,
                ..CompositionNode::default()
            },
            CompositionNode {
                id: "sparks".into(),
                node_type: awidat_proto::professional::CompositionNodeType::ParticleEmitter,
                ..CompositionNode::default()
            },
            CompositionNode {
                id: "output".into(),
                node_type: awidat_proto::professional::CompositionNodeType::Output,
                ..CompositionNode::default()
            },
        ],
        edges: vec![edge("scene", "sparks"), edge("sparks", "output")],
        output_node_id: Some("output".into()),
    };

    let lowering = lower_composition_graph(&graph);
    let inspection = inspect_composition_graph(&graph);

    assert_eq!(lowering.steps.len(), 1);
    assert_eq!(lowering.limitations.len(), 2);
    assert_eq!(inspection.unsupported_nodes, vec!["scene", "sparks"]);
}

#[test]
fn composition_graph_inspection_returns_compact_review_summary() {
    let graph = awidat_proto::professional::CompositionGraph {
        id: "inspect-graph".into(),
        nodes: vec![
            node("input", "media_input", json!({"asset_id": "clip-a"})),
            awidat_proto::professional::CompositionNode {
                id: "plugin-node".into(),
                node_type: awidat_proto::professional::CompositionNodeType::Unsupported,
                params: map([("plugin", json!("third-party-glow"))]),
            },
            node("output", "output", json!({})),
        ],
        edges: vec![edge("input", "plugin-node"), edge("plugin-node", "output")],
        output_node_id: Some("output".into()),
    };

    let inspection = inspect_composition_graph(&graph);

    assert_eq!(inspection.nodes, vec!["input", "plugin-node", "output"]);
    assert_eq!(
        inspection.edges,
        vec!["input -> plugin-node", "plugin-node -> output"]
    );
    assert_eq!(inspection.unsupported_nodes, vec!["plugin-node"]);
    assert!(inspection.render_plan_summary.contains("2 supported steps"));
    assert!(inspection.render_plan_summary.contains("1 limitations"));
}

#[test]
fn motion_template_fill_validates_slots_and_lowers_text_reveal_titles() {
    let template = MotionGraphicsTemplate {
        id: "lower-third".into(),
        name: "Lower Third".into(),
        slots: vec![
            TemplateSlot {
                id: "name".into(),
                kind: TemplateSlotKind::Text,
                required: true,
                ..TemplateSlot::default()
            },
            TemplateSlot {
                id: "title".into(),
                kind: TemplateSlotKind::Text,
                required: false,
                ..TemplateSlot::default()
            },
        ],
        safe_areas: vec![SafeAreaRule {
            profile: "broadcast".into(),
            margin_pct: 0.10,
        }],
        platform_variants: Vec::new(),
    };
    let mut values = BTreeMap::new();
    values.insert("name".into(), json!("Ada Lovelace"));
    values.insert("title".into(), json!("Host"));

    let filled = match fill_motion_template(&template, values) {
        Ok(filled) => filled,
        Err(err) => panic!("filled template: {err}"),
    };
    let render = lower_motion_template(
        &filled,
        MotionTemplateTiming {
            start_s: 1.0,
            end_s: 4.0,
            animation: TemplateAnimation::TextReveal,
        },
    );

    assert!(render.safe_area_violations.is_empty());
    assert!(
        render.titles.len() > 3,
        "text reveal creates staged title plans"
    );
    assert_eq!(render.titles[0].position, TitlePosition::Bottom);
    assert_eq!(render.titles[0].animation, TitleAnimation::None);
}

#[test]
fn text_reveal_keeps_combining_mark_graphemes_together() {
    let template = match built_in_motion_templates()
        .into_iter()
        .find(|template| template.id == "title-reveal")
    {
        Some(template) => template,
        None => panic!("title reveal template"),
    };
    let mut values = BTreeMap::new();
    values.insert("text".into(), json!("Cafe\u{301}"));
    values.insert("target_clip".into(), json!("clip-a"));
    values.insert("safe_area".into(), json!("16:9"));
    let filled = match fill_motion_template(&template, values) {
        Ok(filled) => filled,
        Err(err) => panic!("filled template: {err}"),
    };

    let render = lower_motion_template(
        &filled,
        MotionTemplateTiming {
            start_s: 0.0,
            end_s: 1.0,
            animation: TemplateAnimation::TextReveal,
        },
    );
    let texts = render
        .titles
        .iter()
        .map(|title| title.text.as_str())
        .collect::<Vec<_>>();

    assert_eq!(texts, vec!["C", "Ca", "Caf", "Cafe\u{301}"]);
}

#[test]
fn text_reveal_keeps_zwj_emoji_graphemes_together() {
    let template = match built_in_motion_templates()
        .into_iter()
        .find(|template| template.id == "title-reveal")
    {
        Some(template) => template,
        None => panic!("title reveal template"),
    };
    let mut values = BTreeMap::new();
    values.insert("text".into(), json!("👩\u{200d}💻"));
    values.insert("target_clip".into(), json!("clip-a"));
    values.insert("safe_area".into(), json!("16:9"));
    let filled = match fill_motion_template(&template, values) {
        Ok(filled) => filled,
        Err(err) => panic!("filled template: {err}"),
    };

    let render = lower_motion_template(
        &filled,
        MotionTemplateTiming {
            start_s: 0.0,
            end_s: 1.0,
            animation: TemplateAnimation::TextReveal,
        },
    );
    let texts = render
        .titles
        .iter()
        .map(|title| title.text.as_str())
        .collect::<Vec<_>>();

    assert_eq!(texts, vec!["👩\u{200d}💻"]);
}

#[test]
fn effect_parameter_registry_reports_units_and_validation() {
    let matrix = effect_parameter_capability_matrix();
    let saturation = match matrix
        .iter()
        .find(|entry| entry.effect == "awidat.color_correction" && entry.parameter == "saturation")
    {
        Some(entry) => entry,
        None => panic!("saturation capability"),
    };

    assert_eq!(saturation.unit, "multiplier");
    assert!(saturation.previewable);
    assert!(saturation.renderable);

    let invalid = ParameterAnimation {
        id: "anim-bad-scale".into(),
        target: AnimationTarget::ClipParameter {
            clip_id: "clip-a".into(),
            parameter: "awidat.video_overlay.scale".into(),
        },
        keyframes: vec![Keyframe::linear(0.0, 0.0)],
        pre_extrapolation: ExtrapolationMode::Hold,
        post_extrapolation: ExtrapolationMode::Hold,
        motion_path: None,
        metadata_only: false,
        rationale: None,
    };
    let diagnostic = match diagnose_effect_parameter_animation(&invalid) {
        Some(diagnostic) => diagnostic,
        None => panic!("invalid scale should be diagnosed"),
    };

    assert_eq!(diagnostic.kind, "invalid_effect_parameter_value");
    assert!(diagnostic.message.contains("awidat.video_overlay.scale"));
}

#[test]
fn built_in_motion_template_catalog_covers_phase_3b_templates() {
    let catalog = built_in_motion_templates();
    let ids = catalog
        .iter()
        .map(|template| template.id.as_str())
        .collect::<Vec<_>>();

    for expected in [
        "lower-third",
        "callout",
        "punch-in-zoom",
        "focus-highlight",
        "title-reveal",
        "pip-emphasis",
        "product-insert-emphasis",
    ] {
        assert!(ids.contains(&expected), "missing template {expected}");
    }
    let Some(lower_third) = catalog.iter().find(|template| template.id == "lower-third") else {
        panic!("lower-third template");
    };
    assert!(
        lower_third
            .slots
            .iter()
            .any(|slot| slot.kind == TemplateSlotKind::TargetClip && slot.required)
    );
    assert!(
        lower_third
            .slots
            .iter()
            .any(|slot| slot.kind == TemplateSlotKind::SafeAreaProfile)
    );
}

#[test]
fn lower_third_template_lowers_to_title_opacity_and_y_animations() {
    let template = built_in_template("lower-third");
    let filled = match fill_motion_template(
        &template,
        btree_map([
            ("target_clip", json!("title-clip")),
            ("text", json!("Ada Lovelace")),
            ("subtitle", json!("Host")),
            ("color", json!("#FFFFFF")),
            ("safe_area", json!("16:9")),
        ]),
    ) {
        Ok(filled) => filled,
        Err(err) => panic!("lower-third fill: {err}"),
    };

    let render = lower_motion_template(
        &filled,
        MotionTemplateTiming {
            start_s: 1.0,
            end_s: 4.0,
            animation: TemplateAnimation::Transform,
        },
    );

    let parameters = render
        .parameter_animations
        .iter()
        .map(|animation| match &animation.target {
            awidat_proto::professional::AnimationTarget::ClipParameter { clip_id, parameter } => {
                format!("{clip_id}/{parameter}")
            }
            other => format!("{other:?}"),
        })
        .collect::<Vec<_>>();
    assert!(parameters.contains(&"title-clip/title.opacity".to_string()));
    assert!(parameters.contains(&"title-clip/title.y".to_string()));
}

#[test]
fn focus_highlight_template_lowers_to_overlay_scale_and_opacity() {
    let template = built_in_template("focus-highlight");
    let filled = match fill_motion_template(
        &template,
        btree_map([
            ("target_clip", json!("overlay-clip")),
            ("intensity", json!(1.2)),
            ("safe_area", json!("9:16")),
        ]),
    ) {
        Ok(filled) => filled,
        Err(err) => panic!("focus highlight fill: {err}"),
    };

    let render = lower_motion_template(
        &filled,
        MotionTemplateTiming {
            start_s: 2.0,
            end_s: 3.5,
            animation: TemplateAnimation::Transform,
        },
    );

    let parameters = render
        .parameter_animations
        .iter()
        .map(|animation| match &animation.target {
            awidat_proto::professional::AnimationTarget::ClipParameter { parameter, .. } => {
                parameter.as_str()
            }
            _ => "",
        })
        .collect::<Vec<_>>();
    assert!(parameters.contains(&"overlay.scale"));
    assert!(parameters.contains(&"overlay.opacity"));
}

#[test]
fn missing_required_motion_template_slot_fails() {
    let template = built_in_template("lower-third");

    let err = match fill_motion_template(
        &template,
        btree_map([
            ("target_clip", json!("title-clip")),
            ("subtitle", json!("Host")),
        ]),
    ) {
        Ok(_) => panic!("missing required text slot should fail"),
        Err(err) => err,
    };

    assert_eq!(
        err.to_string(),
        "motion template lower-third missing required slot text"
    );
}

#[test]
fn motion_template_safe_area_violation_surfaces_diagnostic() {
    let template = MotionGraphicsTemplate {
        id: "unsafe-template".into(),
        name: "Unsafe Template".into(),
        slots: vec![
            TemplateSlot {
                id: "target_clip".into(),
                kind: TemplateSlotKind::TargetClip,
                required: true,
                ..TemplateSlot::default()
            },
            TemplateSlot {
                id: "text".into(),
                kind: TemplateSlotKind::Text,
                required: true,
                ..TemplateSlot::default()
            },
        ],
        safe_areas: vec![SafeAreaRule {
            profile: "9:16".into(),
            margin_pct: 0.75,
        }],
        platform_variants: vec!["9:16".into()],
    };
    let filled = match fill_motion_template(
        &template,
        btree_map([
            ("target_clip", json!("title-clip")),
            ("text", json!("Unsafe")),
        ]),
    ) {
        Ok(filled) => filled,
        Err(err) => panic!("template fill: {err}"),
    };

    let render = lower_motion_template(
        &filled,
        MotionTemplateTiming {
            start_s: 0.0,
            end_s: 2.0,
            animation: TemplateAnimation::Opacity,
        },
    );

    assert_eq!(render.safe_area_violations.len(), 1);
    assert_eq!(
        render.safe_area_violations[0].severity,
        FindingSeverity::Warning
    );
    assert!(
        render.safe_area_violations[0]
            .message
            .contains("safe area 9:16")
    );
}

#[test]
fn accepting_motion_package_writes_explicit_animation_records() {
    let mut metadata = AwidatTimelineMetadata::default();
    let package = motion_package("motion-pkg-a", "clip-a", ReviewStatus::Proposed);

    match apply_motion_package(&mut metadata, package, MotionPackageDecision::Accept) {
        Ok(()) => {}
        Err(err) => panic!("motion package applies: {err}"),
    }

    assert_eq!(metadata.motion_packages.len(), 1);
    assert_eq!(metadata.motion_packages[0].status, ReviewStatus::Accepted);
    assert_eq!(metadata.parameter_animations.len(), 1);
    assert_eq!(metadata.learning_signals.len(), 1);
    assert_eq!(metadata.learning_signals[0].status, ReviewStatus::Accepted);
}

#[test]
fn rejecting_motion_package_preserves_project_records_and_learning_signal() {
    let mut metadata = AwidatTimelineMetadata::default();
    let package = motion_package("motion-pkg-a", "clip-a", ReviewStatus::Proposed);

    match apply_motion_package(&mut metadata, package, MotionPackageDecision::Reject) {
        Ok(()) => {}
        Err(err) => panic!("motion package rejects: {err}"),
    }

    assert_eq!(metadata.motion_packages.len(), 1);
    assert_eq!(metadata.motion_packages[0].status, ReviewStatus::Rejected);
    assert!(metadata.parameter_animations.is_empty());
    assert_eq!(metadata.learning_signals.len(), 1);
    assert_eq!(metadata.learning_signals[0].status, ReviewStatus::Rejected);
}

#[test]
fn motion_package_conflict_is_reported_before_apply() {
    let mut metadata = AwidatTimelineMetadata {
        parameter_animations: vec![package_animation("existing", "clip-a", "title.opacity")],
        ..AwidatTimelineMetadata::default()
    };
    let package = motion_package("motion-pkg-a", "clip-a", ReviewStatus::Proposed);

    let err = match apply_motion_package(&mut metadata, package, MotionPackageDecision::Accept) {
        Ok(()) => panic!("conflicting package should not apply"),
        Err(err) => err,
    };

    assert!(
        err.to_string()
            .contains("motion package motion-pkg-a conflicts")
    );
    assert_eq!(metadata.parameter_animations.len(), 1);
    assert!(metadata.motion_packages.is_empty());
}

#[test]
fn motion_package_summary_mentions_generated_changes_and_limitations() {
    let mut package = motion_package("motion-pkg-a", "clip-a", ReviewStatus::Proposed);
    package
        .limitations
        .push("unsupported Bezier omitted".into());

    let summary = motion_package_summary(&package);

    assert!(summary.contains("adds title.opacity on clip-a"));
    assert!(summary.contains("unsupported Bezier omitted"));
}

#[test]
fn color_grade_stack_lowers_to_current_color_correction_plan() {
    let stack = GradeStack {
        id: "grade-a".into(),
        stages: vec![
            GradeStage {
                id: "primary".into(),
                kind: "primary".into(),
                params: map([
                    ("exposure_ev", json!(0.5)),
                    ("contrast", json!(1.15)),
                    ("saturation", json!(0.9)),
                ]),
            },
            GradeStage {
                id: "balance".into(),
                kind: "white_balance".into(),
                params: map([("temperature", json!(0.2)), ("tint", json!(-0.1))]),
            },
        ],
    };

    let plan = match lower_grade_stack(&stack) {
        Ok(plan) => plan,
        Err(err) => panic!("grade stack lowers: {err}"),
    };
    assert_eq!(plan.exposure_ev, Some(0.5));
    assert_eq!(plan.contrast, Some(1.15));
    assert_eq!(plan.saturation, Some(0.9));
    assert_eq!(plan.temperature, Some(0.2));
    assert_eq!(plan.tint, Some(-0.1));
}

#[test]
fn color_review_package_summarizes_groups_and_contact_sheet_artifacts() {
    let state = ColorFinishingState {
        reference_stills: vec![awidat_proto::professional::ReferenceStill {
            id: "ref-1".into(),
            source: "stills/ref-1.jpg".into(),
        }],
        shot_groups: vec![awidat_proto::professional::ShotGroup {
            id: "scene-1".into(),
            clip_ids: vec!["a".into(), "b".into()],
        }],
        grade_stacks: vec![GradeStack {
            id: "grade-a".into(),
            stages: vec![GradeStage {
                id: "primary".into(),
                kind: "primary".into(),
                params: map([("contrast", json!(1.1))]),
            }],
        }],
        color_management: None,
    };

    let review = summarize_color_finishing(&state, "reviews/color");
    assert_eq!(review.reference_stills.len(), 1);
    assert!(
        review
            .contact_sheet_path
            .ends_with("reviews/color/before-after-contact-sheet.json")
    );
    assert!(review.consistency_summaries[0].contains("scene-1"));
}

#[test]
fn audio_finishing_lowers_buses_chains_and_reports_meter_findings() {
    let state = AudioFinishingState {
        buses: vec![AudioBus {
            id: "dialogue".into(),
            role: AudioRole::Dialogue,
            inputs: vec!["a1".into()],
        }],
        automation: vec![AudioAutomationLane {
            target: "dialogue".into(),
            parameter: "volume_db".into(),
            keyframes: vec![Keyframe::linear(0.0, -3.0), Keyframe::linear(2.0, -6.0)],
        }],
        chains: vec![AudioChainPreset {
            id: "dialogue".into(),
            processors: vec!["high_pass".into(), "compressor".into(), "limiter".into()],
        }],
        meters: vec![AudioMeterReading {
            target: "dialogue".into(),
            integrated_lufs: Some(-24.0),
            true_peak_db: Some(-0.2),
            noise_floor_db: Some(-48.0),
            clipping: true,
        }],
    };

    let lowering = lower_audio_finishing(&state);
    assert_eq!(lowering.track_plans.len(), 1);
    assert_eq!(lowering.track_plans[0].role, "dialogue");
    assert!(lowering.track_plans[0].audio_fx.is_some());
    assert!(lowering.findings.iter().any(|f| f.kind == "clipping"));
    assert!(
        lowering
            .findings
            .iter()
            .any(|f| f.kind == "loudness_out_of_range")
    );
}

#[test]
fn audio_volume_automation_lowers_to_expression_and_reports_ducking_conflict() {
    let state = AudioFinishingState {
        buses: vec![AudioBus {
            id: "music".into(),
            role: AudioRole::Music,
            inputs: vec!["a2".into()],
        }],
        automation: vec![
            AudioAutomationLane {
                target: "music".into(),
                parameter: "volume_db".into(),
                keyframes: vec![Keyframe::linear(0.0, -12.0), Keyframe::linear(2.0, -6.0)],
            },
            AudioAutomationLane {
                target: "music".into(),
                parameter: "ducking_db".into(),
                keyframes: vec![Keyframe::linear(0.0, -9.0), Keyframe::linear(2.0, -3.0)],
            },
        ],
        ..AudioFinishingState::default()
    };

    let lowering = lower_audio_finishing(&state);
    let automation = match &lowering.track_plans[0].volume_automation {
        Some(automation) => automation,
        None => panic!("volume automation should lower"),
    };

    assert!(automation.expression.contains("pow(10"));
    assert!(automation.expression.contains("/20"));
    assert_eq!(automation.keyframes.len(), 2);
    assert!(lowering.findings.iter().any(|finding| {
        finding.kind == "ducking_automation_conflict" && finding.message.contains("music")
    }));
}

#[test]
fn delivery_profile_updates_render_spec_and_queue_manifest() {
    let profile = DeliveryProfile::youtube_1080p();
    let spec = RenderJobSpec {
        args: vec!["-y".into(), "renders/timeline.mp4".into()],
        total_duration_s: Some(10.0),
        cwd: None,
        output_path: PathBuf::from("renders/timeline.mp4"),
        limitations: Vec::new(),
    };
    let profiled = apply_delivery_profile_to_spec(spec, &profile);

    assert!(profiled.args.windows(2).any(|w| w == ["-s:v", "1920x1080"]));
    assert!(profiled.args.windows(2).any(|w| w == ["-b:v", "12000k"]));

    let queued = plan_delivery_queue_item(DeliveryQueueRequest {
        profile,
        preflight_input: DeliveryPreflightInput {
            aspect_ratio: "4:3".into(),
            integrated_lufs: None,
            has_captions: false,
            has_required_metadata: false,
            ..DeliveryPreflightInput::default()
        },
        output_path: PathBuf::from("renders/timeline.mp4"),
    });

    assert!(
        queued
            .preflight
            .findings
            .iter()
            .any(|f| f.fix_ref.is_some())
    );
    assert_eq!(queued.manifest.artifacts, vec!["renders/timeline.mp4"]);
    assert_eq!(
        queued.manifest.validation_reports,
        vec![queued.preflight.id.clone()]
    );
}

#[test]
fn export_preset_lowers_codecs_container_and_audio_settings() {
    let preset = ExportPreset::vertical_short_form();
    let spec = RenderJobSpec {
        args: vec!["-y".into(), "renders/timeline.mp4".into()],
        total_duration_s: Some(10.0),
        cwd: None,
        output_path: PathBuf::from("renders/timeline.mp4"),
        limitations: Vec::new(),
    };

    let profiled = match apply_export_preset_to_spec(spec, &preset) {
        Ok(spec) => spec,
        Err(err) => panic!("preset lowers: {err}"),
    };

    assert!(profiled.args.windows(2).any(|w| w == ["-s:v", "1080x1920"]));
    assert!(profiled.args.windows(2).any(|w| w == ["-c:v", "libx264"]));
    assert!(profiled.args.windows(2).any(|w| w == ["-b:v", "12000k"]));
    assert!(profiled.args.windows(2).any(|w| w == ["-c:a", "aac"]));
    assert!(profiled.args.windows(2).any(|w| w == ["-b:a", "192k"]));
    assert!(profiled.args.windows(2).any(|w| w == ["-ar", "48000"]));
    assert!(profiled.args.windows(2).any(|w| w == ["-ac", "2"]));
    assert!(profiled.args.windows(2).any(|w| w == ["-f", "mp4"]));
}

#[test]
fn stream_export_contract_lowers_to_ffmpeg_stream_args() {
    let contract = StreamExportContract {
        id: "stream-master".into(),
        container: "mp4".into(),
        streams: vec![
            StreamExportSpec {
                id: "video".into(),
                kind: StreamKind::Video,
                source_index: 0,
                mode: StreamExportMode::Copy,
                disposition: vec!["default".into()],
                ..StreamExportSpec::default()
            },
            StreamExportSpec {
                id: "english-audio".into(),
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

    let args = match plan_stream_export_args(
        PathBuf::from("raw/source.mov").as_path(),
        &contract,
        PathBuf::from("renders/stream-master.mp4").as_path(),
    ) {
        Ok(args) => args,
        Err(err) => panic!("stream export lowers: {err}"),
    };

    assert_eq!(args[0], "-y");
    assert!(args.windows(2).any(|w| w == ["-map", "0:0"]));
    assert!(args.windows(2).any(|w| w == ["-map", "0:1"]));
    assert!(args.windows(2).any(|w| w == ["-c:0", "copy"]));
    assert!(args.windows(2).any(|w| w == ["-c:1", "aac"]));
    assert!(
        args.windows(2)
            .any(|w| w == ["-metadata:s:1", "language=en"])
    );
    assert!(args.windows(2).any(|w| w == ["-disposition:0", "default"]));
    assert!(args.windows(2).any(|w| w == ["-f", "mp4"]));
    assert_eq!(
        args.last().map(String::as_str),
        Some("renders/stream-master.mp4")
    );
}

fn node(
    id: &str,
    kind: &str,
    params: serde_json::Value,
) -> awidat_proto::professional::CompositionNode {
    awidat_proto::professional::CompositionNode {
        id: id.into(),
        node_type: match kind {
            "media_input" => awidat_proto::professional::CompositionNodeType::MediaInput,
            "transform" => awidat_proto::professional::CompositionNodeType::Transform,
            "merge" => awidat_proto::professional::CompositionNodeType::Merge,
            "mask" => awidat_proto::professional::CompositionNodeType::Mask,
            "matte" => awidat_proto::professional::CompositionNodeType::Matte,
            "text" => awidat_proto::professional::CompositionNodeType::Text,
            "blur" => awidat_proto::professional::CompositionNodeType::Blur,
            "color" => awidat_proto::professional::CompositionNodeType::Color,
            "tracker_bind" => awidat_proto::professional::CompositionNodeType::TrackerBind,
            "output" => awidat_proto::professional::CompositionNodeType::Output,
            other => panic!("node kind {other}"),
        },
        params: match params.as_object() {
            Some(object) => object.clone().into_iter().collect(),
            None => panic!("object params"),
        },
    }
}

fn edge(from: &str, to: &str) -> awidat_proto::professional::CompositionEdge {
    awidat_proto::professional::CompositionEdge {
        from: from.into(),
        to: to.into(),
        input: None,
    }
}

fn map<const N: usize>(
    values: [(&str, serde_json::Value); N],
) -> std::collections::HashMap<String, serde_json::Value> {
    values
        .into_iter()
        .map(|(key, value)| (key.to_string(), value))
        .collect()
}

fn built_in_template(id: &str) -> MotionGraphicsTemplate {
    let Some(template) = built_in_motion_templates()
        .into_iter()
        .find(|template| template.id == id)
    else {
        panic!("built-in template {id}");
    };
    template
}

fn motion_package(id: &str, clip_id: &str, status: ReviewStatus) -> MotionPackage {
    MotionPackage {
        id: id.into(),
        intent: "adds lower-third fade".into(),
        affected_clips: vec![clip_id.into()],
        generated_animations: vec![package_animation("pkg-anim", clip_id, "title.opacity")],
        rationale: Some("introduce speaker".into()),
        status,
        ..MotionPackage::default()
    }
}

fn package_animation(
    id: &str,
    clip_id: &str,
    parameter: &str,
) -> awidat_proto::professional::ParameterAnimation {
    awidat_proto::professional::ParameterAnimation {
        id: id.into(),
        target: awidat_proto::professional::AnimationTarget::ClipParameter {
            clip_id: clip_id.into(),
            parameter: parameter.into(),
        },
        keyframes: vec![Keyframe::linear(0.0, 0.0), Keyframe::linear(1.0, 1.0)],
        pre_extrapolation: ExtrapolationMode::Hold,
        post_extrapolation: ExtrapolationMode::Hold,
        motion_path: None,
        metadata_only: false,
        rationale: None,
    }
}

fn expression_link(
    id: &str,
    clip_id: &str,
    parameter: &str,
    source: ExpressionSource,
    expression: &str,
) -> ExpressionLink {
    ExpressionLink {
        id: id.into(),
        target_clip_id: clip_id.into(),
        target_parameter: parameter.into(),
        source,
        expression: expression.into(),
        enabled: true,
        clamp: None,
    }
}

fn btree_map<const N: usize>(
    values: [(&str, serde_json::Value); N],
) -> BTreeMap<String, serde_json::Value> {
    values
        .into_iter()
        .map(|(key, value)| (key.to_string(), value))
        .collect()
}
