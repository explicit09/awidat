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
    DeliveryQueueRequest, MotionPackageDecision, MotionTemplateTiming, SubjectReframeRequest,
    TemplateAnimation, TrackCorrection, TrackedInsertRequest, TrackerObservation, TrackerRegion,
    TrackingEvidence, TrackingRequest, apply_delivery_profile_to_spec, apply_export_preset_to_spec,
    apply_motion_package, author_subject_reframe_path, author_subject_reframe_path_from_track,
    author_tracked_insert, built_in_motion_templates, diagnose_effect_parameter_animation,
    effect_parameter_capability_matrix, ensure_tracker, ensure_tracker_from_observations,
    evaluate_expression_links, fill_motion_template, generate_tracking_package,
    inspect_composition_graph, lower_audio_finishing, lower_composition_graph, lower_grade_stack,
    lower_motion_template, lower_reframe_path, lower_surface_track_corner_pin,
    lower_surface_track_corner_pin_bindings, lower_track_bound_overlay,
    lower_tracker_parameter_bindings, motion_package_summary, plan_delivery_queue_item,
    plan_stream_export_args, summarize_color_finishing, summarize_tracking_package,
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
fn ensure_tracker_creates_stable_region_handle_and_track_samples() {
    let mut package = TrackingPackage::default();
    let request = TrackingRequest {
        clip_id: "clip-a".into(),
        kind: TrackKind::Surface,
        region: TrackerRegion::from_xywh(0.20, 0.30, 0.40, 0.20),
        start_frame: 10,
        end_frame: 12,
    };

    let handle = match ensure_tracker(&mut package, request.clone()) {
        Ok(handle) => handle,
        Err(err) => panic!("tracker creation should succeed: {err}"),
    };
    let duplicate = match ensure_tracker(&mut package, request) {
        Ok(handle) => handle,
        Err(err) => panic!("tracker creation should be idempotent: {err}"),
    };

    assert_eq!(handle.track_id, duplicate.track_id);
    assert_eq!(package.tracks.len(), 1);
    let track = &package.tracks[0];
    assert_eq!(track.id, handle.track_id);
    assert_eq!(track.asset_id, "clip-a");
    assert_eq!(track.kind, TrackKind::Surface);
    assert_eq!(track.samples.len(), 3);
    assert_eq!(track.samples[0].frame, 10);
    assert_eq!(
        track.samples[0].points,
        vec![[0.20, 0.30], [0.60, 0.30], [0.60, 0.50], [0.20, 0.50]]
    );
    assert!(package.validate().is_empty());
}

#[test]
fn ensure_tracker_from_observations_creates_moving_track_rows() {
    let mut package = TrackingPackage::default();
    let request = TrackingRequest {
        clip_id: "clip-a".into(),
        kind: TrackKind::Surface,
        region: TrackerRegion::from_xywh(0.20, 0.30, 0.40, 0.20),
        start_frame: 10,
        end_frame: 12,
    };
    let observations = vec![
        TrackerObservation {
            frame: 10,
            region: TrackerRegion::from_xywh(0.20, 0.30, 0.40, 0.20),
            confidence: 0.98,
        },
        TrackerObservation {
            frame: 11,
            region: TrackerRegion::from_xywh(0.23, 0.31, 0.40, 0.20),
            confidence: 0.90,
        },
        TrackerObservation {
            frame: 12,
            region: TrackerRegion::from_xywh(0.27, 0.33, 0.41, 0.22),
            confidence: 0.82,
        },
    ];

    let handle = match ensure_tracker_from_observations(&mut package, request, observations) {
        Ok(handle) => handle,
        Err(err) => panic!("observed tracker creation should succeed: {err}"),
    };

    assert_eq!(package.tracks.len(), 1);
    let track = &package.tracks[0];
    assert_eq!(track.id, handle.track_id);
    assert_eq!(track.samples.len(), 3);
    assert_eq!(track.samples[0].points[0], [0.20, 0.30]);
    assert_eq!(track.samples[1].points[0], [0.23, 0.31]);
    assert_eq!(track.samples[2].points[2], [0.68, 0.55]);
    let Some(confidence) = track.confidence else {
        panic!("observed tracker confidence");
    };
    assert!((confidence - 0.9).abs() < 1e-9);
    assert!(package.validate().is_empty());
}

#[test]
fn author_tracked_insert_creates_bindings_and_review_surface() {
    let mut package = TrackingPackage::default();
    let plan = match author_tracked_insert(
        &mut package,
        TrackedInsertRequest {
            overlay_clip_id: "logo-overlay".into(),
            tracker: TrackingRequest {
                clip_id: "speaker-clip".into(),
                kind: TrackKind::Point,
                region: TrackerRegion::from_xywh(0.20, 0.30, 0.20, 0.20),
                start_frame: 0,
                end_frame: 30,
            },
            observations: vec![
                TrackerObservation {
                    frame: 0,
                    region: TrackerRegion::from_xywh(0.20, 0.30, 0.20, 0.20),
                    confidence: 0.94,
                },
                TrackerObservation {
                    frame: 30,
                    region: TrackerRegion::from_xywh(0.40, 0.50, 0.20, 0.20),
                    confidence: 0.52,
                },
            ],
        },
        30.0,
    ) {
        Ok(plan) => plan,
        Err(err) => panic!("tracked insert should author: {err}"),
    };

    assert_eq!(plan.generated_animations.len(), 2);
    assert!(
        plan.generated_animations
            .iter()
            .any(|animation| animation.target
                == AnimationTarget::ClipParameter {
                    clip_id: "logo-overlay".into(),
                    parameter: "overlay.x".into()
                })
    );
    assert!(
        plan.generated_animations
            .iter()
            .any(|animation| animation.target
                == AnimationTarget::ClipParameter {
                    clip_id: "logo-overlay".into(),
                    parameter: "overlay.y".into()
                })
    );
    assert_eq!(plan.review.tracks.len(), 1);
    assert_eq!(plan.review.tracks[0].track_id, plan.track_id);
    assert_eq!(plan.review.tracks[0].correction_frames, vec![30]);
    assert!(plan.review.tracks[0].requires_correction);

    if let Err(err) = (TrackCorrection {
        track_id: plan.track_id,
        samples: vec![
            (0, vec![[0.30, 0.40]], 0.96),
            (30, vec![[0.50, 0.60]], 0.95),
        ],
    })
    .apply(&mut package)
    {
        panic!("correction applies: {err}");
    }
    let review = summarize_tracking_package(&package);

    assert!(review.tracks[0].correction_frames.is_empty());
    assert!(!review.tracks[0].requires_correction);
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
fn tracker_bind_node_lowers_to_clip_parameter_animation() {
    let package = TrackingPackage {
        tracks: vec![TrackSidecar {
            id: "speaker-face".into(),
            asset_id: "clip-a".into(),
            kind: TrackKind::Point,
            samples: vec![
                TrackSample {
                    frame: 0,
                    points: vec![[0.25, 0.75]],
                    confidence: Some(0.95),
                },
                TrackSample {
                    frame: 30,
                    points: vec![[0.40, 0.60]],
                    confidence: Some(0.90),
                },
            ],
            confidence: Some(0.925),
            ..TrackSidecar::default()
        }],
        ..TrackingPackage::default()
    };
    let graph = CompositionGraph {
        id: "tracked-lower-third".into(),
        nodes: vec![node(
            "bind-x",
            "tracker_bind",
            json!({
                "track_id": "speaker-face",
                "target_clip_id": "lower-third",
                "target_parameter": "overlay.x",
                "channel": "x"
            }),
        )],
        ..CompositionGraph::default()
    };

    let animations = match lower_tracker_parameter_bindings(&package, &graph, 30.0) {
        Ok(animations) => animations,
        Err(err) => panic!("tracker bind should lower: {err}"),
    };

    assert_eq!(animations.len(), 1);
    assert_eq!(
        animations[0].target,
        AnimationTarget::ClipParameter {
            clip_id: "lower-third".into(),
            parameter: "overlay.x".into()
        }
    );
    assert_eq!(animations[0].keyframes[0], Keyframe::linear(0.0, 0.25));
    assert_eq!(animations[0].keyframes[1], Keyframe::linear(1.0, 0.40));
    assert!(!animations[0].metadata_only);
}

#[test]
fn tracker_bind_node_rejects_unsupported_target_parameter() {
    let package = TrackingPackage {
        tracks: vec![TrackSidecar {
            id: "speaker-face".into(),
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
        ..TrackingPackage::default()
    };
    let graph = CompositionGraph {
        id: "bad-bind".into(),
        nodes: vec![node(
            "bind-bad",
            "tracker_bind",
            json!({
                "track_id": "speaker-face",
                "target_clip_id": "lower-third",
                "target_parameter": "overlay.lut_path",
                "channel": "x"
            }),
        )],
        ..CompositionGraph::default()
    };

    let err = match lower_tracker_parameter_bindings(&package, &graph, 30.0) {
        Ok(_) => panic!("unsupported tracker_bind target should fail"),
        Err(err) => err,
    };

    assert!(
        err.to_string().contains("unsupported target_parameter")
            && err.to_string().contains("overlay.rotation_deg"),
        "error should enumerate supported runtime clip parameters: {err}"
    );
}

#[test]
fn surface_track_lowers_to_perspective_filter() {
    let package = TrackingPackage {
        tracks: vec![TrackSidecar {
            id: "screen-surface".into(),
            asset_id: "clip-a".into(),
            kind: TrackKind::Surface,
            samples: vec![
                TrackSample {
                    frame: 0,
                    points: vec![[0.10, 0.20], [0.90, 0.18], [0.86, 0.82], [0.14, 0.80]],
                    confidence: Some(0.91),
                },
                TrackSample {
                    frame: 30,
                    points: vec![[0.12, 0.22], [0.88, 0.20], [0.84, 0.84], [0.16, 0.82]],
                    confidence: Some(0.88),
                },
            ],
            confidence: Some(0.895),
            ..TrackSidecar::default()
        }],
        ..TrackingPackage::default()
    };

    let lowering = match lower_surface_track_corner_pin(
        &package,
        "screen-surface",
        "replacement-screen",
        1920,
        1080,
    ) {
        Ok(lowering) => lowering,
        Err(err) => panic!("surface track should lower: {err}"),
    };

    assert_eq!(lowering.track_id, "screen-surface");
    assert_eq!(lowering.target_clip_id, "replacement-screen");
    assert!(
        lowering.filter.contains("perspective=") && lowering.filter.contains(":eval=frame"),
        "corner pin should lower to a per-frame perspective filter: {}",
        lowering.filter
    );
    assert!(
        lowering.filter.contains("x0='if(lte(n\\,0)\\,192")
            && lowering.filter.contains("y0='if(lte(n\\,0)\\,216"),
        "top-left normalized surface point should become pixel expressions: {}",
        lowering.filter
    );
    assert!(
        lowering.filter.contains("x2='if(lte(n\\,0)\\,268.8")
            && lowering.filter.contains("x3='if(lte(n\\,0)\\,1651.2"),
        "surface point order should map bottom-left to x2 and bottom-right to x3: {}",
        lowering.filter
    );
}

#[test]
fn corner_pin_graph_node_lowers_from_surface_track() {
    let package = TrackingPackage {
        tracks: vec![TrackSidecar {
            id: "screen-surface".into(),
            asset_id: "clip-a".into(),
            kind: TrackKind::Surface,
            samples: vec![
                TrackSample {
                    frame: 0,
                    points: vec![[0.10, 0.20], [0.90, 0.18], [0.86, 0.82], [0.14, 0.80]],
                    confidence: Some(0.91),
                },
                TrackSample {
                    frame: 30,
                    points: vec![[0.12, 0.22], [0.88, 0.20], [0.84, 0.84], [0.16, 0.82]],
                    confidence: Some(0.88),
                },
            ],
            confidence: Some(0.895),
            ..TrackSidecar::default()
        }],
        ..TrackingPackage::default()
    };
    let graph = CompositionGraph {
        id: "screen-replace".into(),
        nodes: vec![node(
            "pin-screen",
            "corner_pin",
            json!({
                "track_id": "screen-surface",
                "target_clip_id": "replacement-screen"
            }),
        )],
        ..CompositionGraph::default()
    };

    let lowerings = match lower_surface_track_corner_pin_bindings(&package, &graph, 1920, 1080) {
        Ok(lowerings) => lowerings,
        Err(err) => panic!("corner-pin graph node should lower: {err}"),
    };

    assert_eq!(lowerings.len(), 1);
    assert_eq!(lowerings[0].track_id, "screen-surface");
    assert_eq!(lowerings[0].target_clip_id, "replacement-screen");
    assert!(
        lowerings[0].filter.contains("perspective=") && lowerings[0].filter.contains(":eval=frame"),
        "corner-pin graph node should lower through the surface track perspective path: {}",
        lowerings[0].filter
    );
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

    let review = summarize_tracking_package(&package);
    assert_eq!(review.reframe_paths.len(), 1);
    assert_eq!(review.reframe_paths[0].reframe_id, "vertical-speaker");
    assert_eq!(review.reframe_paths[0].keyframe_count, 2);
    assert!(!review.reframe_paths[0].requires_correction);
}

#[test]
fn author_subject_reframe_path_creates_reviewable_path_from_observations() {
    let mut package = TrackingPackage::default();
    let plan = match author_subject_reframe_path(
        &mut package,
        SubjectReframeRequest {
            clip_id: "clip-speaker".into(),
            aspect_ratio: "9:16".into(),
            source_width: 1920,
            source_height: 1080,
            target_width: 1080,
            target_height: 1920,
            frame_rate: 30.0,
            smoothing: ReframeSmoothing::Gentle,
            safe_area: Some("mobile".into()),
        },
        vec![
            TrackerObservation {
                frame: 0,
                region: TrackerRegion::from_xywh(0.25, 0.2, 0.2, 0.3),
                confidence: 0.92,
            },
            TrackerObservation {
                frame: 30,
                region: TrackerRegion::from_xywh(0.45, 0.25, 0.2, 0.3),
                confidence: 0.88,
            },
        ],
    ) {
        Ok(plan) => plan,
        Err(err) => panic!("subject reframe path should author: {err}"),
    };

    assert_eq!(plan.reframe_id, "reframe-clip-speaker-9-16");
    assert_eq!(package.reframe_paths.len(), 1);
    let path = &package.reframe_paths[0];
    assert_eq!(path.clip_id, "clip-speaker");
    assert_eq!(path.aspect_ratio, "9:16");
    assert_eq!(
        path.evidence_track_id.as_deref(),
        Some("subject-observations")
    );
    assert_eq!(path.keyframes.len(), 2);
    assert_eq!(path.keyframes[0].time_s, 0.0);
    assert_eq!(path.keyframes[1].time_s, 1.0);
    assert_eq!(path.keyframes[0].center, [0.35, 0.35]);
    assert_eq!(path.keyframes[1].center, [0.55, 0.4]);
    assert!((path.keyframes[0].scale - 3.1604938271604937).abs() < 1e-9);

    assert_eq!(plan.review.reframe_paths.len(), 1);
    assert_eq!(
        plan.review.reframe_paths[0].reframe_id,
        "reframe-clip-speaker-9-16"
    );
    assert!(!plan.review.reframe_paths[0].requires_correction);

    let lowering = match lower_reframe_path(&package, &plan.reframe_id) {
        Ok(lowering) => lowering,
        Err(err) => panic!("authored reframe should lower: {err}"),
    };
    assert!(lowering.expression.contains("clip=clip-speaker"));
    assert!(lowering.expression.contains("safe_area=mobile"));
}

#[test]
fn author_subject_reframe_path_from_track_uses_existing_tracker_samples() {
    let mut package = TrackingPackage::default();
    let handle = match ensure_tracker_from_observations(
        &mut package,
        TrackingRequest {
            clip_id: "clip-speaker".into(),
            kind: TrackKind::Surface,
            region: TrackerRegion::from_xywh(0.20, 0.20, 0.20, 0.30),
            start_frame: 0,
            end_frame: 30,
        },
        vec![
            TrackerObservation {
                frame: 0,
                region: TrackerRegion::from_xywh(0.20, 0.20, 0.20, 0.30),
                confidence: 0.91,
            },
            TrackerObservation {
                frame: 30,
                region: TrackerRegion::from_xywh(0.50, 0.30, 0.20, 0.30),
                confidence: 0.89,
            },
        ],
    ) {
        Ok(handle) => handle,
        Err(err) => panic!("tracker should author: {err}"),
    };

    let plan = match author_subject_reframe_path_from_track(
        &mut package,
        &handle.track_id,
        SubjectReframeRequest {
            clip_id: "clip-speaker".into(),
            aspect_ratio: "9:16".into(),
            source_width: 1920,
            source_height: 1080,
            target_width: 1080,
            target_height: 1920,
            frame_rate: 30.0,
            smoothing: ReframeSmoothing::Moderate,
            safe_area: Some("mobile".into()),
        },
    ) {
        Ok(plan) => plan,
        Err(err) => panic!("tracked subject reframe should author: {err}"),
    };

    assert_eq!(plan.reframe_id, "reframe-clip-speaker-9-16");
    assert_eq!(package.reframe_paths.len(), 1);
    let path = &package.reframe_paths[0];
    assert_eq!(
        path.evidence_track_id.as_deref(),
        Some(handle.track_id.as_str())
    );
    assert_eq!(path.keyframes.len(), 2);
    assert_eq!(path.keyframes[0].center, [0.3, 0.35]);
    assert_eq!(path.keyframes[1].center, [0.6, 0.45]);
    assert_eq!(path.keyframes[1].time_s, 1.0);
    assert!(!plan.review.reframe_paths[0].requires_correction);
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
fn motion_template_preserves_explicit_duplicate_subtitle() {
    let template = match built_in_motion_templates()
        .into_iter()
        .find(|template| template.id == "lower-third")
    {
        Some(template) => template,
        None => panic!("lower-third template"),
    };
    let filled = match fill_motion_template(
        &template,
        btree_map([
            ("target_clip", json!("clip-a")),
            ("text", json!("Ada")),
            ("subtitle", json!("Ada")),
            ("safe_area", json!("16:9")),
        ]),
    ) {
        Ok(filled) => filled,
        Err(err) => panic!("filled template: {err}"),
    };

    let render = lower_motion_template(
        &filled,
        MotionTemplateTiming {
            start_s: 1.0,
            end_s: 4.0,
            animation: TemplateAnimation::Opacity,
        },
    );

    assert_eq!(render.titles.len(), 2);
    assert_eq!(render.titles[0].text, "Ada");
    assert_eq!(render.titles[1].text, "Ada");
}

#[test]
fn motion_template_lowers_filled_image_slots_to_media_overlays() {
    let template = match built_in_motion_templates()
        .into_iter()
        .find(|template| template.id == "product-insert-emphasis")
    {
        Some(template) => template,
        None => panic!("product-insert-emphasis template"),
    };
    let filled = match fill_motion_template(
        &template,
        btree_map([
            ("target_clip", json!("clip-a")),
            ("image_asset", json!("logo.svg")),
            ("scale", json!(1.15)),
            ("safe_area", json!("16:9")),
        ]),
    ) {
        Ok(filled) => filled,
        Err(err) => panic!("filled template: {err}"),
    };

    let render = lower_motion_template(
        &filled,
        MotionTemplateTiming {
            start_s: 1.0,
            end_s: 4.0,
            animation: TemplateAnimation::Transform,
        },
    );

    assert_eq!(render.media_overlays.len(), 1);
    assert_eq!(
        render.media_overlays[0].segment.asset_path,
        PathBuf::from("logo.svg")
    );
    assert_eq!(render.media_overlays[0].track_start_s, 1.0);
    assert_eq!(render.media_overlays[0].segment.duration_s, 3.0);
    assert!(
        render
            .media_overlays
            .iter()
            .flat_map(|overlay| overlay.animations.iter())
            .any(|animation| animation.parameter == "overlay.scale")
    );
    assert!(
        render
            .limitations
            .iter()
            .all(|limitation| !limitation.node_id.ends_with(":image_asset")),
        "image slot should lower into a media overlay rather than report unsupported: {:?}",
        render.limitations
    );
}

#[test]
fn logo_reveal_template_lowers_logo_asset_to_fade_and_scale_overlay() {
    let template = built_in_template("logo-reveal");
    let filled = match fill_motion_template(
        &template,
        btree_map([
            ("target_clip", json!("logo-clip")),
            ("logo_asset", json!("brand/logo.png")),
            ("safe_area", json!("16:9")),
        ]),
    ) {
        Ok(filled) => filled,
        Err(err) => panic!("logo reveal fill: {err}"),
    };

    let render = lower_motion_template(
        &filled,
        MotionTemplateTiming {
            start_s: 2.0,
            end_s: 5.0,
            animation: TemplateAnimation::Transform,
        },
    );

    assert_eq!(render.media_overlays.len(), 1);
    assert_eq!(
        render.media_overlays[0].segment.asset_path,
        PathBuf::from("brand/logo.png")
    );
    assert!(
        render
            .media_overlays
            .iter()
            .flat_map(|overlay| overlay.animations.iter())
            .any(|animation| animation.parameter == "overlay.opacity")
    );
    assert!(
        render
            .media_overlays
            .iter()
            .flat_map(|overlay| overlay.animations.iter())
            .any(|animation| animation.parameter == "overlay.scale")
    );
}

#[test]
fn image_only_template_lowers_without_target_clip() {
    let template = MotionGraphicsTemplate {
        id: "brand-bug".into(),
        name: "Brand Bug".into(),
        slots: vec![TemplateSlot {
            id: "logo_asset".into(),
            kind: TemplateSlotKind::Image,
            required: true,
            ..TemplateSlot::default()
        }],
        safe_areas: Vec::new(),
        platform_variants: Vec::new(),
    };
    let filled =
        match fill_motion_template(&template, btree_map([("logo_asset", json!("bug.png"))])) {
            Ok(filled) => filled,
            Err(err) => panic!("image-only fill: {err}"),
        };

    let render = lower_motion_template(
        &filled,
        MotionTemplateTiming {
            start_s: 6.0,
            end_s: 9.0,
            animation: TemplateAnimation::None,
        },
    );

    assert_eq!(render.media_overlays.len(), 1);
    assert_eq!(
        render.media_overlays[0].segment.asset_path,
        PathBuf::from("bug.png")
    );
    assert!(render.media_overlays[0].animations.is_empty());
    assert!(
        render
            .limitations
            .iter()
            .all(|limitation| !limitation.node_id.ends_with(":logo_asset")),
        "image-only template should not silently drop or warn for a renderable logo slot: {:?}",
        render.limitations
    );
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
    let blur = match matrix
        .iter()
        .find(|entry| entry.effect == "awidat.video_overlay" && entry.parameter == "blur")
    {
        Some(entry) => entry,
        None => panic!("overlay blur capability"),
    };

    assert_eq!(blur.unit, "px");
    assert!(blur.previewable);
    assert!(blur.renderable);

    let shake = match matrix
        .iter()
        .find(|entry| entry.effect == "awidat.shake" && entry.parameter == "intensity_px")
    {
        Some(entry) => entry,
        None => panic!("shake intensity capability"),
    };

    assert_eq!(shake.unit, "px");
    assert!(shake.previewable);
    assert!(shake.renderable);

    let clip_blur = match matrix
        .iter()
        .find(|entry| entry.effect == "awidat.blur" && entry.parameter == "radius_px")
    {
        Some(entry) => entry,
        None => panic!("clip blur capability"),
    };

    assert_eq!(clip_blur.unit, "px");
    assert!(clip_blur.previewable);
    assert!(clip_blur.renderable);

    let warp = match matrix
        .iter()
        .find(|entry| entry.effect == "awidat.warp" && entry.parameter == "k1")
    {
        Some(entry) => entry,
        None => panic!("warp k1 capability"),
    };

    assert_eq!(warp.unit, "coefficient");
    assert!(warp.previewable);
    assert!(warp.renderable);

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
        "shake-emphasis",
        "logo-reveal",
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
            ("color", json!("#FFCC00")),
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
    assert_eq!(render.titles[0].color, "#FFCC00");
    assert_eq!(render.titles[1].color, "#FFCC00");
    assert!(
        render
            .limitations
            .iter()
            .all(|limitation| !limitation.node_id.ends_with(":color")),
        "color slot should lower into title color rather than report unsupported: {:?}",
        render.limitations
    );
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
fn shake_emphasis_template_lowers_to_overlay_x_and_rotation_keyframes() {
    let template = built_in_template("shake-emphasis");
    let filled = match fill_motion_template(
        &template,
        btree_map([
            ("target_clip", json!("overlay-clip")),
            ("intensity", json!(0.04)),
            ("safe_area", json!("16:9")),
        ]),
    ) {
        Ok(filled) => filled,
        Err(err) => panic!("shake emphasis fill: {err}"),
    };

    let render = lower_motion_template(
        &filled,
        MotionTemplateTiming {
            start_s: 4.0,
            end_s: 5.0,
            animation: TemplateAnimation::Transform,
        },
    );

    let x_animation = clip_parameter_animation(&render.parameter_animations, "overlay.x");
    let y_animation = clip_parameter_animation(&render.parameter_animations, "overlay.y");
    let rotation_animation =
        clip_parameter_animation(&render.parameter_animations, "overlay.rotation_deg");

    assert_eq!(x_animation.keyframes.len(), 6);
    assert_eq!(x_animation.keyframes[0].value, 0.0);
    assert_eq!(x_animation.keyframes[1].value, -0.04);
    assert_eq!(x_animation.keyframes[2].value, 0.04);
    assert_eq!(x_animation.keyframes[5].value, 0.0);
    assert_eq!(y_animation.keyframes.len(), 6);
    assert_eq!(y_animation.keyframes[0].value, 0.0);
    assert!(y_animation.keyframes[1].value > 0.0);
    assert!(y_animation.keyframes[2].value < 0.0);
    assert_eq!(y_animation.keyframes[5].value, 0.0);
    assert_eq!(rotation_animation.keyframes.len(), 5);
    assert_eq!(rotation_animation.keyframes[0].value, 0.0);
    assert!(rotation_animation.keyframes[1].value < 0.0);
    assert!(rotation_animation.keyframes[2].value > 0.0);
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
            "corner_pin" => awidat_proto::professional::CompositionNodeType::CornerPin,
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

fn clip_parameter_animation<'a>(
    animations: &'a [ParameterAnimation],
    parameter: &str,
) -> &'a ParameterAnimation {
    for animation in animations {
        let AnimationTarget::ClipParameter {
            parameter: candidate,
            ..
        } = &animation.target
        else {
            continue;
        };
        if candidate == parameter {
            return animation;
        }
    }
    panic!("parameter animation {parameter}");
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
