//! Professional EDL operation contract tests.

use awidat_core::edl::anchor::AnchorContext;
use awidat_core::edl::apply::apply;
use awidat_core::edl::op::{
    Anchor, AnnotationKind, EdlOp, MotionTemplateAnimation, ProfessionalTimelineEdit, TitleWeight,
};
use awidat_core::edl::parser::parse;
use awidat_proto::awidat_meta::{Anchor as AwAnchor, AwidatClipMetadata};
use awidat_proto::otio::{
    Clip, ClipMetadata, ExternalReference, MediaReference, RationalTime, StackChild, TimeRange,
    Timeline, Track, TrackChild, TrackKind,
};
use awidat_proto::professional::{
    AnimationTarget, CapabilityArea, CompositionGraph, DeliveryProfile, Keyframe,
    ParameterAnimation, SourceRange, WorkflowLens,
};

#[test]
fn professional_timeline_ops_roundtrip_through_edl_json() {
    let ops = vec![
        EdlOp::ProfessionalTimelineEdit {
            edit: ProfessionalTimelineEdit::RippleTrim {
                anchor: Anchor::ClipUuid {
                    uuid: "clip-a".into(),
                },
                new_end_s: 42.0,
                ripple_tracks: vec!["V1".into(), "A1".into()],
            },
        },
        EdlOp::SetParameterAnimation {
            animation: ParameterAnimation {
                id: "anim-opacity".into(),
                target: AnimationTarget::ClipParameter {
                    clip_id: "clip-a".into(),
                    parameter: "opacity".into(),
                },
                keyframes: vec![Keyframe::linear(0.0, 0.0), Keyframe::linear(1.0, 1.0)],
                ..ParameterAnimation::default()
            },
        },
        EdlOp::AttachComposition {
            graph: CompositionGraph::single_output("tracked-title"),
            attach_to: Some(SourceRange {
                start_s: 4.0,
                end_s: 8.0,
            }),
        },
        EdlOp::SelectDeliveryProfile {
            profile: DeliveryProfile::youtube_1080p(),
        },
        EdlOp::SetWorkflowLens {
            lens: WorkflowLens::Preflight,
        },
    ];

    for op in ops {
        let json = match serde_json::to_string(&op) {
            Ok(json) => json,
            Err(error) => panic!("serialize op: {error}"),
        };
        let roundtrip: EdlOp = match serde_json::from_str(&json) {
            Ok(op) => op,
            Err(error) => panic!("deserialize op: {error}"),
        };
        assert_eq!(roundtrip, op);
    }
}

#[test]
fn professional_capability_area_serialization_is_stable() {
    let json = match serde_json::to_string(&CapabilityArea::PreAutonomyOrchestrationContract) {
        Ok(json) => json,
        Err(error) => panic!("serialize capability: {error}"),
    };

    assert_eq!(json, "\"pre_autonomy_orchestration_contract\"");
}

#[test]
fn parser_accepts_professional_substrate_json_ops() {
    let edl = r#"
*** Begin EDL
*** Set Asset Catalog
+ catalog_json: {"assets":[{"id":"asset-a","path":"raw/a.mov","role":"video","tags":["hero"],"readiness":{"proxy":"ready","index":"ready","online":"ready"}}]}
*** Set Parameter Animation
+ animation_json: {"id":"anim-a","target":{"kind":"clip_parameter","clip_id":"clip-a","parameter":"opacity"},"keyframes":[{"time_s":0.0,"value":0.0},{"time_s":1.0,"value":1.0}]}
*** Select Delivery Profile
+ profile_json: {"id":"web","name":"Web","platform":"youtube","aspect_ratio":"16:9","width":1920,"height":1080,"preflight_checks":["aspect_ratio","metadata"]}
*** End EDL
"#;

    let envelope = match parse(edl) {
        Ok(envelope) => envelope,
        Err(error) => panic!("parse professional edl: {error}"),
    };

    assert_eq!(envelope.ops.len(), 3);
    assert!(matches!(envelope.ops[0], EdlOp::SetAssetCatalog { .. }));
    assert!(matches!(
        envelope.ops[1],
        EdlOp::SetParameterAnimation { .. }
    ));
    assert!(matches!(
        envelope.ops[2],
        EdlOp::SelectDeliveryProfile { .. }
    ));
}

#[test]
fn parser_accepts_motion_template_instantiation() {
    let edl = r#"
*** Begin EDL
*** Instantiate Motion Template
+ template_id: lower-third
+ start_s: 1.0
+ end_s: 4.0
+ animation: transform
+ slot_values_json: {"text":"Ada Lovelace","subtitle":"Host","safe_area":"16:9"}
*** End EDL
"#;

    let envelope = match parse(edl) {
        Ok(envelope) => envelope,
        Err(error) => panic!("parse motion template instantiation: {error}"),
    };

    assert_eq!(envelope.ops.len(), 1);
    let EdlOp::InstantiateMotionTemplate {
        template_id,
        start_s,
        end_s,
        animation,
        slot_values,
    } = &envelope.ops[0]
    else {
        panic!(
            "expected InstantiateMotionTemplate, got {:?}",
            envelope.ops[0]
        );
    };

    assert_eq!(template_id, "lower-third");
    assert_eq!(*start_s, 1.0);
    assert_eq!(*end_s, 4.0);
    assert_eq!(*animation, MotionTemplateAnimation::Transform);
    assert_eq!(
        slot_values.get("text").and_then(serde_json::Value::as_str),
        Some("Ada Lovelace")
    );
}

#[test]
fn motion_template_instantiation_lowers_to_titles_and_runtime_animations() {
    let edl = r#"
*** Begin EDL
*** Instantiate Motion Template
+ template_id: lower-third
+ start_s: 1.0
+ end_s: 4.0
+ animation: transform
+ slot_values_json: {"text":"Ada Lovelace","subtitle":"Host","safe_area":"16:9"}
*** End EDL
"#;
    let envelope = match parse(edl) {
        Ok(envelope) => envelope,
        Err(error) => panic!("parse motion template instantiation: {error}"),
    };
    let timeline = Timeline::empty("motion-template-instantiation");

    let (timeline, outcome) = match apply(&timeline, &envelope, &AnchorContext::empty()) {
        Ok(result) => result,
        Err(error) => panic!("apply motion template instantiation: {error}"),
    };

    assert_eq!(outcome.applied.len(), 1);
    let Some(titles) = timeline
        .tracks
        .children
        .iter()
        .find_map(|child| match child {
            StackChild::Track(track) if track.name == "Titles" => Some(track),
            _ => None,
        })
    else {
        panic!("titles track should be created");
    };
    assert_eq!(titles.children.len(), 2);

    let TrackChild::Clip(first_title) = &titles.children[0] else {
        panic!("first titles child should be a clip");
    };
    let Some(effect) = first_title
        .effects
        .iter()
        .find(|effect| effect.effect_name == "awidat.title")
    else {
        panic!("motion template title should carry awidat.title effect");
    };
    assert_eq!(
        effect
            .metadata
            .get("text")
            .and_then(serde_json::Value::as_str),
        Some("Ada Lovelace")
    );
    assert_eq!(
        effect
            .metadata
            .get("role")
            .and_then(serde_json::Value::as_str),
        Some("motion_template")
    );
    let Some(clip_uuid) = first_title
        .metadata
        .awidat
        .as_ref()
        .and_then(|metadata| metadata.extra.get("clip_uuid"))
        .and_then(serde_json::Value::as_str)
    else {
        panic!("motion template title should have a targetable clip uuid");
    };

    let Some(metadata) = timeline.metadata.awidat else {
        panic!("timeline metadata should include template animations");
    };
    assert_eq!(metadata.parameter_animations.len(), 2);
    let animated_parameters = metadata
        .parameter_animations
        .iter()
        .map(|animation| match &animation.target {
            AnimationTarget::ClipParameter { clip_id, parameter } => {
                assert_eq!(clip_id, clip_uuid);
                parameter.as_str()
            }
            other => panic!("expected clip animation target, got {other:?}"),
        })
        .collect::<Vec<_>>();
    assert_eq!(animated_parameters, vec!["title.opacity", "title.y"]);
}

#[test]
fn parser_accepts_annotation_primitives() {
    let edl = r#"
*** Begin EDL
*** Insert Annotation
+ start_s: 2.0
+ end_s: 5.0
+ kind: rectangle
+ x: 0.10
+ y: 0.20
+ width: 0.30
+ height: 0.25
+ color: #FFCC00
+ stroke_width: 6
+ label: Circle this
*** End EDL
"#;

    let envelope = match parse(edl) {
        Ok(envelope) => envelope,
        Err(error) => panic!("parse annotation primitive: {error}"),
    };

    let EdlOp::InsertAnnotation {
        kind,
        start_s,
        end_s,
        x,
        y,
        width,
        height,
        color,
        stroke_width,
        label,
    } = &envelope.ops[0]
    else {
        panic!("expected InsertAnnotation, got {:?}", envelope.ops[0]);
    };

    assert_eq!(*kind, AnnotationKind::Rectangle);
    assert_eq!(*start_s, 2.0);
    assert_eq!(*end_s, 5.0);
    assert_eq!((*x, *y, *width, *height), (0.10, 0.20, 0.30, 0.25));
    assert_eq!(color, "#FFCC00");
    assert_eq!(*stroke_width, 6);
    assert_eq!(label.as_deref(), Some("Circle this"));
}

#[test]
fn parser_rejects_annotation_color_filter_options() {
    let edl = r#"
*** Begin EDL
*** Insert Annotation
+ start_s: 2.0
+ end_s: 5.0
+ kind: rectangle
+ x: 0.10
+ y: 0.20
+ width: 0.30
+ height: 0.25
+ color: red:t=fill
*** End EDL
"#;

    let err = match parse(edl) {
        Ok(envelope) => panic!("expected annotation color rejection, got {envelope:?}"),
        Err(err) => err,
    };

    assert!(
        err.to_string().contains("annotation color"),
        "unexpected parse error: {err}"
    );
}

#[test]
fn annotation_primitive_applies_as_virtual_overlay_clip() {
    let edl = r#"
*** Begin EDL
*** Insert Annotation
+ start_s: 2.0
+ end_s: 5.0
+ kind: blur
+ x: 0.10
+ y: 0.20
+ width: 0.30
+ height: 0.25
*** End EDL
"#;
    let envelope = match parse(edl) {
        Ok(envelope) => envelope,
        Err(error) => panic!("parse annotation primitive: {error}"),
    };
    let timeline = Timeline::empty("annotation-primitive");

    let (timeline, outcome) = match apply(&timeline, &envelope, &AnchorContext::empty()) {
        Ok(result) => result,
        Err(error) => panic!("apply annotation primitive: {error}"),
    };

    assert_eq!(outcome.applied.len(), 1);
    let Some(annotations) = timeline
        .tracks
        .children
        .iter()
        .find_map(|child| match child {
            StackChild::Track(track) if track.name == "Annotations" => Some(track),
            _ => None,
        })
    else {
        panic!("annotations track should be created");
    };
    assert_eq!(
        annotations
            .metadata
            .get("awidat_track_role")
            .and_then(serde_json::Value::as_str),
        Some("annotations")
    );
    let TrackChild::Clip(annotation_clip) = &annotations.children[0] else {
        panic!("expected annotation clip");
    };
    let Some(effect) = annotation_clip
        .effects
        .iter()
        .find(|effect| effect.effect_name == "awidat.annotation")
    else {
        panic!("annotation clip should carry awidat.annotation effect");
    };
    assert_eq!(
        effect
            .metadata
            .get("kind")
            .and_then(serde_json::Value::as_str),
        Some("blur")
    );
    assert_eq!(
        effect.metadata.get("x").and_then(serde_json::Value::as_f64),
        Some(0.10)
    );
}

#[test]
fn parser_accepts_rich_title_segments() {
    let edl = r##"
*** Begin EDL
*** Insert Rich Title
+ start_s: 1.0
+ end_s: 3.0
+ position: bottom
+ font_size: 64
+ animation: fade_in
+ segments_json: [{"text":"Launch","color":"#FFFFFF","font_weight":"bold"},{"text":" now","color":"#FFCC00","font_weight":"normal"}]
*** End EDL
"##;

    let envelope = match parse(edl) {
        Ok(envelope) => envelope,
        Err(error) => panic!("parse rich title: {error}"),
    };

    let EdlOp::InsertRichTitle {
        segments,
        font_size,
        ..
    } = &envelope.ops[0]
    else {
        panic!("expected InsertRichTitle, got {:?}", envelope.ops[0]);
    };

    assert_eq!(*font_size, 64);
    assert_eq!(segments.len(), 2);
    assert_eq!(segments[0].text, "Launch");
    assert_eq!(segments[0].font_weight, Some(TitleWeight::Bold));
    assert_eq!(segments[1].color.as_deref(), Some("#FFCC00"));
}

#[test]
fn rich_title_applies_as_title_with_run_metadata() {
    let edl = r##"
*** Begin EDL
*** Insert Rich Title
+ start_s: 1.0
+ end_s: 3.0
+ position: bottom
+ font_size: 64
+ segments_json: [{"text":"Launch","color":"#FFFFFF","font_weight":"bold"},{"text":" now","color":"#FFCC00","font_weight":"normal"}]
*** End EDL
"##;
    let envelope = match parse(edl) {
        Ok(envelope) => envelope,
        Err(error) => panic!("parse rich title: {error}"),
    };
    let timeline = Timeline::empty("rich-title");

    let (timeline, _) = match apply(&timeline, &envelope, &AnchorContext::empty()) {
        Ok(result) => result,
        Err(error) => panic!("apply rich title: {error}"),
    };

    let Some(titles) = timeline
        .tracks
        .children
        .iter()
        .find_map(|child| match child {
            StackChild::Track(track) if track.name == "Titles" => Some(track),
            _ => None,
        })
    else {
        panic!("titles track should be created");
    };
    let TrackChild::Clip(title_clip) = &titles.children[0] else {
        panic!("expected title clip");
    };
    let Some(effect) = title_clip
        .effects
        .iter()
        .find(|effect| effect.effect_name == "awidat.title")
    else {
        panic!("rich title should carry awidat.title");
    };

    assert_eq!(
        effect
            .metadata
            .get("text")
            .and_then(serde_json::Value::as_str),
        Some("Launch now")
    );
    assert!(
        effect.metadata.get("rich_segments").is_some(),
        "rich title should preserve segment styling metadata"
    );
}

#[test]
fn parsed_professional_substrate_ops_apply_to_timeline_metadata() {
    let edl = r#"
*** Begin EDL
*** Set Asset Catalog
+ catalog_json: {"assets":[{"id":"asset-a","path":"raw/a.mov","role":"video","tags":["hero"],"readiness":{"proxy":"ready","index":"ready","online":"ready"}}]}
*** Set Source Review
+ selects_json: [{"id":"select-a","asset_id":"asset-a","range":{"start_s":1.0,"end_s":3.0},"decision":"select"}]
+ stringouts_json: [{"id":"stringout-a","select_ids":["select-a"]}]
*** Select Delivery Profile
+ profile_json: {"id":"web","name":"Web","platform":"youtube","aspect_ratio":"16:9","width":1920,"height":1080,"preflight_checks":["aspect_ratio","metadata"]}
*** Set Workflow Lens
+ lens: selects
*** End EDL
"#;
    let envelope = match parse(edl) {
        Ok(envelope) => envelope,
        Err(error) => panic!("parse professional edl: {error}"),
    };
    let timeline = Timeline::empty("professional-pipeline");

    let (timeline, outcome) = match apply(&timeline, &envelope, &AnchorContext::empty()) {
        Ok(result) => result,
        Err(error) => panic!("apply professional edl: {error}"),
    };
    let metadata = match timeline.metadata.awidat {
        Some(metadata) => metadata,
        None => panic!("timeline metadata missing"),
    };

    assert_eq!(outcome.applied.len(), 4);
    assert_eq!(metadata.selects.len(), 1);
    assert_eq!(metadata.stringouts.len(), 1);
    assert_eq!(metadata.delivery_profiles.len(), 1);
    assert_eq!(
        metadata.build_professional_readiness_report().stages.len(),
        13
    );
}

#[test]
fn set_parameter_animation_rejects_runtime_unsupported_target_by_default() {
    let timeline = Timeline::empty("animation-contract");
    let envelope = awidat_core::edl::op::EdlEnvelope {
        ops: vec![EdlOp::SetParameterAnimation {
            animation: ParameterAnimation {
                id: "anim-blur".into(),
                target: AnimationTarget::ClipParameter {
                    clip_id: "clip-a".into(),
                    parameter: "effect.blur.radius".into(),
                },
                keyframes: vec![Keyframe::linear(0.0, 0.0), Keyframe::linear(1.0, 12.0)],
                ..ParameterAnimation::default()
            },
        }],
    };

    let err = match apply(&timeline, &envelope, &AnchorContext::empty()) {
        Ok(_) => panic!("unsupported target should be rejected"),
        Err(err) => err,
    };

    assert!(
        err.to_string()
            .contains("unsupported parameter animation target"),
        "expected unsupported target rejection, got: {err}"
    );
    assert!(
        err.to_string().contains("metadata_only"),
        "error should name the explicit metadata-only escape hatch, got: {err}"
    );
    assert!(
        err.to_string().contains("title.opacity") && err.to_string().contains("volume_db"),
        "error should list runtime-supported parameters for agent correction, got: {err}"
    );
}

#[test]
fn set_parameter_animation_preserves_unsupported_target_when_metadata_only() {
    let timeline = Timeline::empty("animation-contract");
    let envelope = awidat_core::edl::op::EdlEnvelope {
        ops: vec![EdlOp::SetParameterAnimation {
            animation: ParameterAnimation {
                id: "anim-blur".into(),
                target: AnimationTarget::ClipParameter {
                    clip_id: "clip-a".into(),
                    parameter: "effect.blur.radius".into(),
                },
                keyframes: vec![Keyframe::linear(0.0, 0.0), Keyframe::linear(1.0, 12.0)],
                metadata_only: true,
                ..ParameterAnimation::default()
            },
        }],
    };

    let (timeline, outcome) = match apply(&timeline, &envelope, &AnchorContext::empty()) {
        Ok(result) => result,
        Err(err) => panic!("metadata-only animation should apply: {err}"),
    };
    let Some(metadata) = timeline.metadata.awidat else {
        panic!("timeline metadata missing");
    };

    assert_eq!(outcome.applied.len(), 1);
    assert_eq!(metadata.parameter_animations.len(), 1);
    assert!(metadata.parameter_animations[0].metadata_only);
    assert_eq!(metadata.parameter_animations[0].id, "anim-blur");
}

#[test]
fn set_parameter_animation_accepts_track_volume_db_automation() {
    let timeline = Timeline::empty("animation-contract");
    let envelope = awidat_core::edl::op::EdlEnvelope {
        ops: vec![EdlOp::SetParameterAnimation {
            animation: ParameterAnimation {
                id: "music-duck".into(),
                target: AnimationTarget::TrackParameter {
                    track: "Music".into(),
                    parameter: "volume_db".into(),
                },
                keyframes: vec![Keyframe::linear(0.0, -18.0), Keyframe::linear(1.0, -6.0)],
                ..ParameterAnimation::default()
            },
        }],
    };

    let (timeline, outcome) = match apply(&timeline, &envelope, &AnchorContext::empty()) {
        Ok(result) => result,
        Err(err) => panic!("track volume automation should apply: {err}"),
    };
    let Some(metadata) = timeline.metadata.awidat else {
        panic!("timeline metadata missing");
    };

    assert_eq!(outcome.applied.len(), 1);
    assert_eq!(metadata.parameter_animations.len(), 1);
    assert_eq!(metadata.parameter_animations[0].id, "music-duck");
}

#[test]
fn lowered_professional_timeline_edit_records_metadata() {
    let edl = r#"
*** Begin EDL
*** Professional Timeline Edit
+ edit_json: {"edit":"overwrite","track":"V1","asset":"raw/replacement.mov","range":{"start_s":1.0,"end_s":3.0}}
*** End EDL
"#;
    let envelope = match parse(edl) {
        Ok(envelope) => envelope,
        Err(error) => panic!("parse professional timeline edit: {error}"),
    };
    let timeline = Timeline::empty("professional-record-only");

    let (timeline, outcome) = match apply(&timeline, &envelope, &AnchorContext::empty()) {
        Ok(result) => result,
        Err(error) => panic!("apply professional timeline edit: {error}"),
    };
    let metadata = match timeline.metadata.awidat {
        Some(metadata) => metadata,
        None => panic!("timeline metadata missing"),
    };

    assert_eq!(timeline.tracks.children.len(), 1);
    assert!(outcome.applied[0].description.contains("overwrote range"));
    assert!(
        metadata
            .extra
            .contains_key("last_professional_timeline_edit")
    );
}

#[test]
fn delete_clip_preserves_timeline_timing_with_gap() {
    let timeline = timeline_with_three_clip_track();
    let envelope = match parse(
        r#"
*** Begin EDL
*** Delete Clip
@@ anchor: transcript_snippet="bravo snippet"
*** End EDL
"#,
    ) {
        Ok(envelope) => envelope,
        Err(error) => panic!("delete clip edl should parse: {error}"),
    };

    let (timeline, outcome) = match apply(&timeline, &envelope, &AnchorContext::empty()) {
        Ok(result) => result,
        Err(error) => panic!("delete clip should apply: {error}"),
    };
    let StackChild::Track(track) = &timeline.tracks.children[0] else {
        panic!("expected video track");
    };

    assert_eq!(outcome.applied.len(), 1);
    assert_eq!(track.children.len(), 3);
    assert!(matches!(&track.children[0], TrackChild::Clip(clip) if clip.name == "clip-0"));
    assert!(
        matches!(&track.children[1], TrackChild::Gap(gap) if (gap.source_range.duration.to_seconds() - 5.0).abs() < 1e-9)
    );
    assert!(matches!(&track.children[2], TrackChild::Clip(clip) if clip.name == "clip-2"));
}

fn timeline_with_three_clip_track() -> Timeline {
    let mut timeline = Timeline::empty("primitive-fixture");
    let mut track = Track::empty("V1", TrackKind::Video);
    for (index, transcript_snippet) in ["alpha snippet", "bravo snippet", "charlie snippet"]
        .into_iter()
        .enumerate()
    {
        track
            .children
            .push(TrackChild::Clip(test_clip(index, transcript_snippet)));
    }
    timeline.tracks.children.push(StackChild::Track(track));
    timeline
}

fn test_clip(index: usize, transcript_snippet: &str) -> Clip {
    let mut clip = Clip::empty(format!("clip-{index}"));
    clip.media_reference =
        MediaReference::External(ExternalReference::new(format!("raw/{index}.mp4")));
    clip.source_range = Some(TimeRange::new(
        RationalTime::new(0.0, 24.0),
        RationalTime::new(5.0 * 24.0, 24.0),
    ));
    clip.metadata = ClipMetadata {
        awidat: Some(AwidatClipMetadata {
            anchor: Some(AwAnchor {
                transcript_snippet: Some(transcript_snippet.to_string()),
                ..AwAnchor::default()
            }),
            ..AwidatClipMetadata::default()
        }),
        ..ClipMetadata::default()
    };
    clip
}
