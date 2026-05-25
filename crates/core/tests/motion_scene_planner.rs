//! MotionScene planner tool contract tests.

use awidat_core::edl::op::EdlOp;
use awidat_core::edl::parser::parse;
use awidat_core::tools::plan_motion_scene::plan_motion_scene_request;
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
