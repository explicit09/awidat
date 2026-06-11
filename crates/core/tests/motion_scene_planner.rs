#![allow(clippy::expect_used, clippy::unwrap_used)]
//! MotionScene planner tool contract tests.

use montage_core::edl::anchor::AnchorContext;
use montage_core::edl::apply::apply;
use montage_core::edl::op::EdlOp;
use montage_core::edl::parser::parse;
use montage_core::tools::plan_motion_scene::{MotionScenePlanRequest, plan_motion_scene_request};
use montage_core::tools::plan_visual_support::{VisualSupportLane, route_visual_support_request};
use montage_proto::otio::Timeline;
use montage_proto::professional::MotionSceneLayerKind;

fn onboarding_steps() -> Vec<String> {
    vec![
        "Sign up with your workspace".into(),
        "Import your first project".into(),
        "Invite the team".into(),
    ]
}

#[test]
fn planner_returns_valid_motion_scene_and_storable_edl() {
    let plan = match plan_motion_scene_request(&MotionScenePlanRequest {
        request: "animate a three-step explainer for the onboarding framework".into(),
        scene_id: Some("scene-onboarding".into()),
        duration_s: Some(5.0),
        width: Some(1280),
        height: Some(720),
        fps: Some(24.0),
        headline: Some("The onboarding framework".into()),
        step_labels: onboarding_steps(),
        ..MotionScenePlanRequest::default()
    }) {
        Ok(plan) => plan,
        Err(error) => panic!("plan motion scene: {error}"),
    };

    assert_eq!(plan.scene.id, "scene-onboarding");
    assert_eq!(plan.scene.duration_s, 5.0);
    assert_eq!(plan.scene.width, 1280);
    assert_eq!(plan.scene.height, 720);
    assert!(
        plan.scene
            .layers
            .iter()
            .any(|layer| layer.kind == MotionSceneLayerKind::Text)
    );
    assert!(plan.render_support.contains("text"));
    assert!(plan.render_support.contains("rectangle/solid"));
    assert!(plan.render_support.contains("image"));

    let envelope = match parse(&plan.edl) {
        Ok(envelope) => envelope,
        Err(error) => panic!("parse generated motion scene edl: {error}"),
    };
    assert!(matches!(envelope.ops[0], EdlOp::SetMotionScene { .. }));
}

#[test]
fn planner_rejects_invalid_scene_timing() {
    let error = plan_motion_scene_request(&MotionScenePlanRequest {
        request: "animated callout".into(),
        duration_s: Some(0.0),
        headline: Some("Callout".into()),
        ..MotionScenePlanRequest::default()
    })
    .expect_err("zero duration should be rejected");

    assert!(error.contains("duration_s"));
}

#[test]
fn planner_requires_headline_or_evidence_text() {
    // The planner used to truncate the request prompt into the
    // headline, putting the editor's instructions on screen.
    let error = plan_motion_scene_request(&MotionScenePlanRequest {
        request: "animated callout for the key claim".into(),
        ..MotionScenePlanRequest::default()
    })
    .expect_err("missing content must hard-fail");

    assert!(error.contains("headline"), "error: {error}");
    assert!(error.contains("evidence_text"), "error: {error}");
}

#[test]
fn planner_requires_step_labels_for_step_requests() {
    // "Step 1/2/3" placeholders are never invented.
    let error = plan_motion_scene_request(&MotionScenePlanRequest {
        request: "three-step process card".into(),
        headline: Some("Hardware-first drone workflow".into()),
        ..MotionScenePlanRequest::default()
    })
    .expect_err("step request without labels must hard-fail");

    assert!(error.contains("step_labels"), "error: {error}");
}

#[test]
fn planner_derives_headline_from_evidence_text() {
    let plan = plan_motion_scene_request(&MotionScenePlanRequest {
        request: "animated callout".into(),
        evidence_text: Some(
            "If AI makes a mistake with code, you might have a cybersecurity issue. The rest \
             of the window keeps going."
                .into(),
        ),
        ..MotionScenePlanRequest::default()
    })
    .expect("plan motion scene from evidence");

    let headline = plan
        .scene
        .layers
        .iter()
        .find(|layer| layer.id == "headline")
        .and_then(|layer| layer.params.get("text"))
        .and_then(|value| value.as_str())
        .expect("headline text");
    assert_eq!(
        headline,
        "If AI makes a mistake with code, you might have a cybersecurity issue"
    );
    assert!(
        plan.scene
            .rationale
            .as_deref()
            .is_some_and(|rationale| rationale.contains("transcript evidence")),
        "rationale records the evidence window: {:?}",
        plan.scene.rationale
    );
}

#[test]
fn planner_puts_step_labels_on_screen() {
    let plan = plan_motion_scene_request(&MotionScenePlanRequest {
        request: "three-step explainer".into(),
        headline: Some("The onboarding framework".into()),
        step_labels: onboarding_steps(),
        ..MotionScenePlanRequest::default()
    })
    .expect("plan motion scene");

    let step_texts: Vec<&str> = plan
        .scene
        .layers
        .iter()
        .filter(|layer| layer.id.starts_with("step-"))
        .filter_map(|layer| layer.params.get("text").and_then(|value| value.as_str()))
        .collect();
    assert_eq!(
        step_texts,
        vec![
            "Sign up with your workspace",
            "Import your first project",
            "Invite the team"
        ]
    );
}

#[test]
fn planner_adds_panel_and_image_layers_for_visual_still_requests() {
    let plan = plan_motion_scene_request(&MotionScenePlanRequest {
        request: "make a callout card with the product logo screenshot".into(),
        scene_id: Some("scene-logo".into()),
        duration_s: Some(3.0),
        image_asset: Some("raw/logo.png".into()),
        headline: Some("Product logo".into()),
        ..MotionScenePlanRequest::default()
    })
    .expect("plan motion scene");

    assert!(plan.scene.layers.iter().any(|layer| {
        matches!(
            layer.kind,
            MotionSceneLayerKind::Solid | MotionSceneLayerKind::Shape
        )
    }));
    let image = plan
        .scene
        .layers
        .iter()
        .find(|layer| layer.kind == MotionSceneLayerKind::Image)
        .expect("image layer");
    assert_eq!(image.params["asset"], "raw/logo.png");
    assert!(
        image.params["animations"]
            .as_array()
            .is_some_and(|animations| {
                animations
                    .iter()
                    .any(|animation| animation["parameter"] == "overlay.opacity")
            })
    );
}

#[test]
fn planner_builds_multi_layer_explainer_scene() {
    let plan = plan_motion_scene_request(&MotionScenePlanRequest {
        request: "create a three-step explainer card with a headline, step labels, callout \
                  arrow, and product screenshot"
            .into(),
        scene_id: Some("scene-steps".into()),
        duration_s: Some(4.0),
        width: Some(1920),
        height: Some(1080),
        fps: Some(30.0),
        image_asset: Some("raw/product.png".into()),
        headline: Some("The onboarding framework".into()),
        step_labels: onboarding_steps(),
        ..MotionScenePlanRequest::default()
    })
    .expect("plan motion scene");

    assert!(
        plan.scene.layers.len() >= 7,
        "expected panel, image, callout, headline, and step layers"
    );
    assert!(
        plan.scene
            .layers
            .iter()
            .any(|layer| layer.id == "background-panel")
    );
    assert!(
        plan.scene
            .layers
            .iter()
            .any(|layer| layer.id == "product-image")
    );
    assert!(
        plan.scene
            .layers
            .iter()
            .any(|layer| layer.id == "callout-accent")
    );
    let step_count = plan
        .scene
        .layers
        .iter()
        .filter(|layer| layer.id.starts_with("step-") && layer.kind == MotionSceneLayerKind::Text)
        .count();
    assert_eq!(step_count, 3);
    assert!(
        plan.scene.layers.iter().all(|layer| {
            layer.params.contains_key("x")
                || layer.kind == MotionSceneLayerKind::Text
                || layer.kind == MotionSceneLayerKind::Group
        }),
        "non-text renderable layers should carry shared transforms"
    );
}

#[test]
fn visual_route_planner_and_apply_persist_multilayer_motion_scene() {
    let request =
        "explain the three-step onboarding process with a product screenshot and supporting b-roll";
    let route = route_visual_support_request(request);

    // This is the agent workflow contract: route the editorial need, plan
    // the native MotionScene, then persist the returned Set Motion Scene EDL.
    assert_eq!(route.primary_lane, VisualSupportLane::MotionScene);
    assert!(route.supporting_lanes.contains(&VisualSupportLane::Broll));
    let motion_tools: Vec<&str> = route
        .plan_steps
        .iter()
        .filter(|step| step.lane == VisualSupportLane::MotionScene)
        .map(|step| step.tool.as_str())
        .collect();
    assert_eq!(motion_tools, vec!["plan_motion_scene", "apply_edl"]);

    let plan = plan_motion_scene_request(&MotionScenePlanRequest {
        request: request.into(),
        scene_id: Some("scene-onboarding-e2e".into()),
        duration_s: Some(4.0),
        width: Some(1920),
        height: Some(1080),
        fps: Some(30.0),
        image_asset: Some("raw/product.png".into()),
        headline: Some("The onboarding process".into()),
        step_labels: onboarding_steps(),
        ..MotionScenePlanRequest::default()
    })
    .expect("plan motion scene");
    let envelope = parse(&plan.edl).expect("parse generated EDL");
    let timeline = Timeline::empty("motion-scene-route-apply");
    let (timeline, outcome) =
        apply(&timeline, &envelope, &AnchorContext::empty()).expect("apply generated EDL");
    let metadata = timeline.metadata.montage.expect("montage metadata");
    let stored_scene = metadata
        .motion_scenes
        .iter()
        .find(|scene| scene.id == "scene-onboarding-e2e")
        .expect("stored motion scene");

    assert_eq!(outcome.applied.len(), 1);
    assert_eq!(stored_scene.layers.len(), plan.scene.layers.len());
    assert!(
        stored_scene
            .layers
            .iter()
            .any(|layer| layer.kind == MotionSceneLayerKind::Text)
    );
    assert!(
        stored_scene
            .layers
            .iter()
            .any(|layer| layer.kind == MotionSceneLayerKind::Solid)
    );
    assert!(
        stored_scene
            .layers
            .iter()
            .any(|layer| layer.kind == MotionSceneLayerKind::Shape)
    );
    assert!(
        stored_scene
            .layers
            .iter()
            .any(|layer| layer.kind == MotionSceneLayerKind::Image)
    );
}

#[test]
fn planner_full_backdrop_spans_the_entire_frame() {
    let plan = plan_motion_scene_request(&MotionScenePlanRequest {
        request: "full-frame quote card".into(),
        headline: Some("Hardware mistakes move into the real world".into()),
        backdrop: Some("full".into()),
        ..MotionScenePlanRequest::default()
    })
    .expect("plan full-backdrop scene");

    let backdrop = plan
        .scene
        .layers
        .iter()
        .find(|layer| layer.id == "backdrop-full")
        .expect("full-bleed backdrop layer");
    assert_eq!(backdrop.kind, MotionSceneLayerKind::Solid);
    let param = |key: &str| {
        backdrop
            .params
            .get(key)
            .and_then(serde_json::Value::as_f64)
            .unwrap_or_else(|| panic!("backdrop param {key}"))
    };
    // True full bleed: exactly 0,0 → 1,1, no inset margins.
    assert_eq!(param("x"), 0.0);
    assert_eq!(param("y"), 0.0);
    assert_eq!(param("width"), 1.0);
    assert_eq!(param("height"), 1.0);
}

#[test]
fn planner_rejects_unknown_backdrop_mode() {
    let error = plan_motion_scene_request(&MotionScenePlanRequest {
        request: "quote card".into(),
        headline: Some("A headline".into()),
        backdrop: Some("hero".into()),
        ..MotionScenePlanRequest::default()
    })
    .expect_err("unknown backdrop must be rejected");
    assert!(error.contains("backdrop 'hero'"), "{error}");
}

#[test]
fn planner_headline_carries_an_explicit_text_box() {
    let plan = plan_motion_scene_request(&MotionScenePlanRequest {
        request: "quote card".into(),
        headline: Some("Software is too easy to get into".into()),
        ..MotionScenePlanRequest::default()
    })
    .expect("plan quote scene");

    let headline = plan
        .scene
        .layers
        .iter()
        .find(|layer| layer.id == "headline")
        .expect("headline layer");
    for key in ["x", "y", "width", "font_size"] {
        assert!(
            headline.params.contains_key(key),
            "headline should carry an explicit box param {key}: {:?}",
            headline.params
        );
    }
}

#[test]
fn planner_shrinks_font_until_text_fits_its_box() {
    // A single word far wider than the 0.76 headline box at 64px —
    // the planner must step font_size down instead of overflowing.
    let plan = plan_motion_scene_request(&MotionScenePlanRequest {
        request: "quote card".into(),
        headline: Some(
            "Antidisestablishmentarianism-Antidisestablishmentarianism-Antidisestablishmentarianism"
                .into(),
        ),
        ..MotionScenePlanRequest::default()
    })
    .expect("plan oversized headline scene");

    let headline = plan
        .scene
        .layers
        .iter()
        .find(|layer| layer.id == "headline")
        .expect("headline layer");
    let font_size = headline
        .params
        .get("font_size")
        .and_then(serde_json::Value::as_u64)
        .expect("font_size param");
    assert!(
        font_size < 64,
        "oversized headline should auto-shrink, got {font_size}"
    );
}
