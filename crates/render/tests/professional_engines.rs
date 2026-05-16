//! Tests for professional render-side engines and lowerers.

use std::collections::BTreeMap;
use std::path::PathBuf;

use awidat_proto::professional::{
    AudioAutomationLane, AudioBus, AudioChainPreset, AudioFinishingState, AudioMeterReading,
    AudioRole, ColorFinishingState, DeliveryPreflightInput, DeliveryProfile, FindingSeverity,
    GradeStack, GradeStage, Keyframe, MotionGraphicsTemplate, SafeAreaRule, TemplateSlot,
    TemplateSlotKind, TrackKind,
};
use awidat_render::professional::{
    DeliveryQueueRequest, MotionTemplateTiming, TemplateAnimation, TrackCorrection,
    TrackingEvidence, apply_delivery_profile_to_spec, fill_motion_template,
    generate_tracking_package, lower_audio_finishing, lower_composition_graph, lower_grade_stack,
    lower_motion_template, plan_delivery_queue_item, summarize_color_finishing,
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
fn delivery_profile_updates_render_spec_and_queue_manifest() {
    let profile = DeliveryProfile::youtube_1080p();
    let spec = RenderJobSpec {
        args: vec!["-y".into(), "renders/timeline.mp4".into()],
        total_duration_s: Some(10.0),
        cwd: None,
        output_path: PathBuf::from("renders/timeline.mp4"),
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

fn map<const N: usize>(
    values: [(&str, serde_json::Value); N],
) -> std::collections::HashMap<String, serde_json::Value> {
    values
        .into_iter()
        .map(|(key, value)| (key.to_string(), value))
        .collect()
}
