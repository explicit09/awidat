//! MotionScene planner tool contract tests.

use awidat_core::edl::anchor::AnchorContext;
use awidat_core::edl::apply::apply;
use awidat_core::edl::op::EdlOp;
use awidat_core::edl::parser::parse;
use awidat_core::tools::plan_motion_scene::plan_motion_scene_request;
use awidat_core::tools::plan_visual_support::{VisualSupportLane, route_visual_support_request};
use awidat_proto::otio::Timeline;
use awidat_proto::professional::MotionSceneLayerKind;

#[test]
fn planner_returns_valid_motion_scene_and_storable_edl() {
    let plan = match plan_motion_scene_request(
        "animate a three-step explainer for the onboarding framework",
        Some("scene-onboarding"),
        Some(5.0),
        Some(1280),
        Some(720),
        Some(24.0),
        None,
    ) {
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
    let error =
        plan_motion_scene_request("animated callout", None, Some(0.0), None, None, None, None)
            .expect_err("zero duration should be rejected");

    assert!(error.contains("duration_s"));
}

#[test]
fn planner_adds_panel_and_image_layers_for_visual_still_requests() {
    let plan = plan_motion_scene_request(
        "make a callout card with the product logo screenshot",
        Some("scene-logo"),
        Some(3.0),
        None,
        None,
        None,
        Some("raw/logo.png"),
    )
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
    let plan = plan_motion_scene_request(
        "create a three-step explainer card with a headline, step labels, callout arrow, and product screenshot",
        Some("scene-steps"),
        Some(4.0),
        Some(1920),
        Some(1080),
        Some(30.0),
        Some("raw/product.png"),
    )
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

    let plan = plan_motion_scene_request(
        request,
        Some("scene-onboarding-e2e"),
        Some(4.0),
        Some(1920),
        Some(1080),
        Some(30.0),
        Some("raw/product.png"),
    )
    .expect("plan motion scene");
    let envelope = parse(&plan.edl).expect("parse generated EDL");
    let timeline = Timeline::empty("motion-scene-route-apply");
    let (timeline, outcome) =
        apply(&timeline, &envelope, &AnchorContext::empty()).expect("apply generated EDL");
    let metadata = timeline.metadata.awidat.expect("awidat metadata");
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
