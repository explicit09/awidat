#![allow(clippy::expect_used, clippy::unwrap_used)]
//! `plan_motion_scene` template-mode integration tests.
//!
//! For each of the four motion templates: end-to-end through
//! `plan_motion_scene_request` -> scene validates -> the returned EDL
//! snippet parses via the EDL parser -> applies to a scratch timeline,
//! mirroring the established pattern in `motion_scene_planner.rs`.
//! Plus error cases for missing required fields and invalid enum
//! values.

use montage_core::edl::anchor::AnchorContext;
use montage_core::edl::apply::apply;
use montage_core::edl::op::EdlOp;
use montage_core::edl::parser::parse;
use montage_core::tools::plan_motion_scene::{MotionScenePlanRequest, plan_motion_scene_request};
use montage_proto::otio::Timeline;
use montage_proto::professional::MotionSceneLayerKind;

/// Apply a plan's EDL to a fresh scratch timeline and return the
/// stored `MotionScene` for assertions, mirroring
/// `motion_scene_planner.rs`'s `visual_route_planner_and_apply_...`
/// pattern.
fn apply_plan_to_scratch_timeline(
    edl: &str,
    scene_id: &str,
    timeline_name: &str,
) -> montage_proto::professional::MotionScene {
    let envelope = match parse(edl) {
        Ok(envelope) => envelope,
        Err(error) => panic!("parse generated motion scene edl: {error}"),
    };
    assert!(matches!(envelope.ops[0], EdlOp::SetMotionScene { .. }));

    let timeline = Timeline::empty(timeline_name);
    let (timeline, outcome) = match apply(&timeline, &envelope, &AnchorContext::empty()) {
        Ok(result) => result,
        Err(error) => panic!("apply generated EDL: {error}"),
    };
    assert_eq!(outcome.applied.len(), 1);

    let metadata = timeline.metadata.montage.expect("montage metadata");
    metadata
        .motion_scenes
        .into_iter()
        .find(|scene| scene.id == scene_id)
        .expect("stored motion scene")
}

#[test]
fn lower_third_template_end_to_end() {
    let plan = plan_motion_scene_request(&MotionScenePlanRequest {
        request: "lower third for the guest".into(),
        scene_id: Some("scene-lower-third".into()),
        duration_s: Some(4.0),
        template: Some("lower_third".into()),
        name: Some("Ada Lovelace".into()),
        role: Some("Mathematician".into()),
        ..MotionScenePlanRequest::default()
    })
    .expect("plan lower third template");

    assert_eq!(plan.scene.id, "scene-lower-third");
    let ids: Vec<&str> = plan.scene.layers.iter().map(|l| l.id.as_str()).collect();
    assert!(ids.contains(&"lower-third-bar"));
    assert!(ids.contains(&"lower-third-name"));
    assert!(ids.contains(&"lower-third-role"));
    assert!(plan.scene.validate().is_empty());
    assert!(
        plan.rationale.to_lowercase().contains("lower third")
            || plan
                .scene
                .rationale
                .as_deref()
                .unwrap_or("")
                .contains("Ada"),
        "rationale should reflect the template expansion: plan.rationale={}, scene.rationale={:?}",
        plan.rationale,
        plan.scene.rationale
    );

    let stored = apply_plan_to_scratch_timeline(&plan.edl, "scene-lower-third", "lower-third-e2e");
    assert_eq!(stored.layers.len(), plan.scene.layers.len());
    assert!(
        stored
            .layers
            .iter()
            .any(|layer| layer.id == "lower-third-name" && layer.kind == MotionSceneLayerKind::Text)
    );
}

#[test]
fn kinetic_text_template_end_to_end() {
    let plan = plan_motion_scene_request(&MotionScenePlanRequest {
        request: "kinetic text cascade".into(),
        scene_id: Some("scene-kinetic".into()),
        duration_s: Some(3.0),
        template: Some("kinetic_text".into()),
        words: vec![
            ("Ship".into(), 0.0, 0.4),
            ("it".into(), 0.25, 0.4),
            ("today".into(), 0.5, 0.4),
        ],
        ..MotionScenePlanRequest::default()
    })
    .expect("plan kinetic text template");

    assert_eq!(plan.scene.id, "scene-kinetic");
    assert!(plan.scene.validate().is_empty());
    let word_layer_count = plan
        .scene
        .layers
        .iter()
        .filter(|l| l.id.starts_with("kinetic-word-"))
        .count();
    assert_eq!(word_layer_count, 3);

    let stored = apply_plan_to_scratch_timeline(&plan.edl, "scene-kinetic", "kinetic-text-e2e");
    assert_eq!(stored.layers.len(), plan.scene.layers.len());
    let texts: Vec<&str> = stored
        .layers
        .iter()
        .filter_map(|layer| layer.params.get("text").and_then(|v| v.as_str()))
        .collect();
    assert_eq!(texts, vec!["Ship", "it", "today"]);
}

#[test]
fn kinetic_text_anchor_lower_center_places_words_near_bottom() {
    // `anchor` is now a real production input; "lower_center" pins the
    // words near the bottom (y ~ 0.82) instead of the default center
    // (y ~ 0.46).
    let plan = plan_motion_scene_request(&MotionScenePlanRequest {
        request: "kinetic text cascade".into(),
        scene_id: Some("scene-kinetic-anchor".into()),
        duration_s: Some(3.0),
        template: Some("kinetic_text".into()),
        words: vec![("Ship".into(), 0.0, 0.4)],
        anchor: Some("lower_center".into()),
        ..MotionScenePlanRequest::default()
    })
    .expect("plan kinetic text with lower_center anchor");

    let y = plan
        .scene
        .layers
        .iter()
        .find(|l| l.id == "kinetic-word-0")
        .and_then(|l| l.params.get("y"))
        .and_then(serde_json::Value::as_f64);
    assert_eq!(y, Some(0.82));
}

#[test]
fn kinetic_text_default_anchor_is_center() {
    let plan = plan_motion_scene_request(&MotionScenePlanRequest {
        request: "kinetic text cascade".into(),
        scene_id: Some("scene-kinetic-default-anchor".into()),
        duration_s: Some(3.0),
        template: Some("kinetic_text".into()),
        words: vec![("Ship".into(), 0.0, 0.4)],
        ..MotionScenePlanRequest::default()
    })
    .expect("plan kinetic text with default anchor");

    let y = plan
        .scene
        .layers
        .iter()
        .find(|l| l.id == "kinetic-word-0")
        .and_then(|l| l.params.get("y"))
        .and_then(serde_json::Value::as_f64);
    assert_eq!(y, Some(0.46));
}

#[test]
fn kinetic_text_invalid_anchor_errors_clearly() {
    let error = plan_motion_scene_request(&MotionScenePlanRequest {
        request: "kinetic text cascade".into(),
        template: Some("kinetic_text".into()),
        words: vec![("Ship".into(), 0.0, 0.4)],
        anchor: Some("top_right".into()),
        ..MotionScenePlanRequest::default()
    })
    .expect_err("invalid anchor must hard-fail");
    assert!(
        error.contains("anchor") && error.contains("top_right"),
        "error should name the anchor field and offending value: {error}"
    );
}

#[test]
fn highlight_box_template_end_to_end() {
    let plan = plan_motion_scene_request(&MotionScenePlanRequest {
        request: "highlight the pricing card".into(),
        scene_id: Some("scene-highlight".into()),
        duration_s: Some(3.0),
        template: Some("highlight_box".into()),
        box_region: Some((0.2, 0.25, 0.35, 0.3)),
        pulse: Some(true),
        ..MotionScenePlanRequest::default()
    })
    .expect("plan highlight box template");

    assert_eq!(plan.scene.id, "scene-highlight");
    assert!(plan.scene.validate().is_empty());
    let layer = plan
        .scene
        .layers
        .iter()
        .find(|l| l.id == "highlight-box")
        .expect("highlight-box layer");
    assert_eq!(layer.kind, MotionSceneLayerKind::Shape);

    let stored = apply_plan_to_scratch_timeline(&plan.edl, "scene-highlight", "highlight-box-e2e");
    assert_eq!(stored.layers.len(), plan.scene.layers.len());
}

#[test]
fn progress_bar_template_end_to_end() {
    let plan = plan_motion_scene_request(&MotionScenePlanRequest {
        request: "progress bar for the upload".into(),
        scene_id: Some("scene-progress".into()),
        duration_s: Some(3.0),
        template: Some("progress_bar".into()),
        progress: Some((0.1, 0.75, 0.9)),
        color: Some("#00FF00".into()),
        ..MotionScenePlanRequest::default()
    })
    .expect("plan progress bar template");

    assert_eq!(plan.scene.id, "scene-progress");
    assert!(plan.scene.validate().is_empty());
    let layer = plan
        .scene
        .layers
        .iter()
        .find(|l| l.id == "progress-bar")
        .expect("progress-bar layer");
    assert_eq!(layer.kind, MotionSceneLayerKind::Shape);
    assert_eq!(
        layer.params.get("color").and_then(|v| v.as_str()),
        Some("#00FF00")
    );

    let stored = apply_plan_to_scratch_timeline(&plan.edl, "scene-progress", "progress-bar-e2e");
    assert_eq!(stored.layers.len(), plan.scene.layers.len());
}

fn render_support_mentions_preview_only(render_support: &str) -> bool {
    // Normalize the wrapped-string whitespace before matching.
    let normalized: String = render_support
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    normalized.contains("live preview only")
        && normalized
            .contains("export/render lowering for scale-driven motion lands in a later phase")
}

#[test]
fn progress_bar_render_support_discloses_preview_only_scale_motion() {
    let plan = plan_motion_scene_request(&MotionScenePlanRequest {
        request: "progress bar for the upload".into(),
        duration_s: Some(3.0),
        template: Some("progress_bar".into()),
        progress: Some((0.1, 0.75, 0.9)),
        ..MotionScenePlanRequest::default()
    })
    .expect("plan progress bar template");
    assert!(
        render_support_mentions_preview_only(&plan.render_support),
        "progress_bar render_support should disclose preview-only scale motion: {}",
        plan.render_support
    );
}

#[test]
fn highlight_box_pulse_render_support_discloses_preview_only_scale_motion() {
    let plan = plan_motion_scene_request(&MotionScenePlanRequest {
        request: "highlight the pricing card".into(),
        duration_s: Some(3.0),
        template: Some("highlight_box".into()),
        box_region: Some((0.2, 0.25, 0.35, 0.3)),
        pulse: Some(true),
        ..MotionScenePlanRequest::default()
    })
    .expect("plan pulsing highlight box template");
    assert!(
        render_support_mentions_preview_only(&plan.render_support),
        "pulsing highlight_box render_support should disclose preview-only scale motion: {}",
        plan.render_support
    );
}

#[test]
fn highlight_box_without_pulse_render_support_omits_preview_only_disclosure() {
    let plan = plan_motion_scene_request(&MotionScenePlanRequest {
        request: "highlight the pricing card".into(),
        duration_s: Some(3.0),
        template: Some("highlight_box".into()),
        box_region: Some((0.2, 0.25, 0.35, 0.3)),
        pulse: Some(false),
        ..MotionScenePlanRequest::default()
    })
    .expect("plan non-pulsing highlight box template");
    assert!(
        !render_support_mentions_preview_only(&plan.render_support),
        "non-pulsing highlight_box must not disclose preview-only scale motion: {}",
        plan.render_support
    );
}

#[test]
fn lower_third_render_support_omits_preview_only_disclosure() {
    let plan = plan_motion_scene_request(&MotionScenePlanRequest {
        request: "lower third for the guest".into(),
        duration_s: Some(4.0),
        template: Some("lower_third".into()),
        name: Some("Ada Lovelace".into()),
        ..MotionScenePlanRequest::default()
    })
    .expect("plan lower third template");
    assert!(
        !render_support_mentions_preview_only(&plan.render_support),
        "lower_third must not disclose preview-only scale motion: {}",
        plan.render_support
    );
}

#[test]
fn kinetic_text_render_support_omits_preview_only_disclosure() {
    let plan = plan_motion_scene_request(&MotionScenePlanRequest {
        request: "kinetic text cascade".into(),
        duration_s: Some(3.0),
        template: Some("kinetic_text".into()),
        words: vec![("Ship".into(), 0.0, 0.4)],
        ..MotionScenePlanRequest::default()
    })
    .expect("plan kinetic text template");
    assert!(
        !render_support_mentions_preview_only(&plan.render_support),
        "kinetic_text must not disclose preview-only scale motion: {}",
        plan.render_support
    );
}

#[test]
fn template_layers_replace_heuristic_layers() {
    // A request phrase that would normally trigger heuristic panel +
    // callout layers must not leak into template mode: expander output
    // replaces the heuristic layers wholesale.
    let plan = plan_motion_scene_request(&MotionScenePlanRequest {
        request: "callout panel with a diagram".into(),
        scene_id: Some("scene-template-replace".into()),
        duration_s: Some(4.0),
        template: Some("lower_third".into()),
        name: Some("Grace Hopper".into()),
        ..MotionScenePlanRequest::default()
    })
    .expect("plan lower third template over a heuristic-triggering request");

    let ids: Vec<&str> = plan.scene.layers.iter().map(|l| l.id.as_str()).collect();
    assert!(
        !ids.contains(&"background-panel") && !ids.contains(&"callout-accent"),
        "template mode must replace heuristic layers, got ids: {ids:?}"
    );
    assert!(ids.contains(&"lower-third-bar"));
}

#[test]
fn lower_third_template_missing_name_errors_clearly() {
    let error = plan_motion_scene_request(&MotionScenePlanRequest {
        request: "lower third".into(),
        template: Some("lower_third".into()),
        ..MotionScenePlanRequest::default()
    })
    .expect_err("missing name must hard-fail");
    assert!(
        error.contains("name"),
        "error should name the field: {error}"
    );
}

#[test]
fn kinetic_text_template_missing_words_errors_clearly() {
    let error = plan_motion_scene_request(&MotionScenePlanRequest {
        request: "kinetic text".into(),
        template: Some("kinetic_text".into()),
        ..MotionScenePlanRequest::default()
    })
    .expect_err("missing words must hard-fail");
    assert!(
        error.contains("words"),
        "error should name the field: {error}"
    );
}

#[test]
fn highlight_box_template_missing_box_errors_clearly() {
    let error = plan_motion_scene_request(&MotionScenePlanRequest {
        request: "highlight box".into(),
        template: Some("highlight_box".into()),
        ..MotionScenePlanRequest::default()
    })
    .expect_err("missing box must hard-fail");
    assert!(
        error.contains("box"),
        "error should name the field: {error}"
    );
}

#[test]
fn progress_bar_template_missing_progress_errors_clearly() {
    let error = plan_motion_scene_request(&MotionScenePlanRequest {
        request: "progress bar".into(),
        template: Some("progress_bar".into()),
        ..MotionScenePlanRequest::default()
    })
    .expect_err("missing progress must hard-fail");
    assert!(
        error.contains("progress"),
        "error should name the field: {error}"
    );
}

#[test]
fn unknown_template_value_errors_clearly() {
    let error = plan_motion_scene_request(&MotionScenePlanRequest {
        request: "some scene".into(),
        template: Some("wipe_transition".into()),
        ..MotionScenePlanRequest::default()
    })
    .expect_err("unknown template must hard-fail");
    assert!(
        error.contains("wipe_transition"),
        "error should name the offending value: {error}"
    );
}

#[test]
fn template_mode_surfaces_ignored_inputs_in_rationale() {
    // template mode with image_asset and backdrop provided: they should be
    // silently ignored (not error), but their names should appear in the
    // plan's rationale so the agent knows they were discarded.
    let plan = plan_motion_scene_request(&MotionScenePlanRequest {
        request: "lower third for the guest".into(),
        scene_id: Some("scene-ignored-inputs".into()),
        duration_s: Some(4.0),
        template: Some("lower_third".into()),
        name: Some("Ada Lovelace".into()),
        // These five inputs should be ignored in template mode:
        image_asset: Some("assets/diagram.png".into()),
        backdrop: Some("full".into()),
        headline: Some("This should be ignored".into()),
        step_labels: vec!["Step 1".into(), "Step 2".into()],
        evidence_text: Some("Transcript text that should be ignored".into()),
        ..MotionScenePlanRequest::default()
    })
    .expect("plan lower third template with ignored inputs");

    // Scene must validate and have correct template layers.
    assert_eq!(plan.scene.id, "scene-ignored-inputs");
    assert!(plan.scene.validate().is_empty());
    let ids: Vec<&str> = plan.scene.layers.iter().map(|l| l.id.as_str()).collect();
    assert!(ids.contains(&"lower-third-bar"));
    assert!(ids.contains(&"lower-third-name"));

    // Rationale must mention the ignored inputs by name.
    let rationale_lower = plan
        .scene
        .rationale
        .as_ref()
        .unwrap_or(&"".to_string())
        .to_lowercase();
    assert!(
        rationale_lower.contains("ignored")
            && rationale_lower.contains("image_asset")
            && rationale_lower.contains("backdrop")
            && rationale_lower.contains("headline")
            && rationale_lower.contains("step_labels")
            && rationale_lower.contains("evidence_text"),
        "rationale should list all five ignored inputs: {:?}",
        plan.scene.rationale
    );

    // Verify that no image or backdrop layers made it into the scene.
    assert!(
        !ids.iter()
            .any(|id| id.contains("image") || id.contains("panel") || id.contains("backdrop")),
        "scene should contain no image or backdrop layers when template mode is active"
    );

    // Apply the EDL and verify the stored scene has the same structure.
    let stored =
        apply_plan_to_scratch_timeline(&plan.edl, "scene-ignored-inputs", "ignored-inputs-e2e");
    assert_eq!(stored.layers.len(), plan.scene.layers.len());
}
