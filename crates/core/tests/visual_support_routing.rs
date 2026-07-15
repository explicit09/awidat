//! Visual-support routing policy tests.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use montage_core::montage_mcp::tools::plan_visual_support::{
    VisualSupportIntent, VisualSupportLane, VisualSupportNeedKind, route_visual_support_request,
};

#[test]
fn routes_abstract_explainer_to_motion_scene() {
    let route = route_visual_support_request(
        "visualize the three-step framework with animated labels and arrows",
    );

    assert_eq!(route.primary_lane, VisualSupportLane::MotionScene);
    assert!(route.next_tools.contains(&"plan_motion_scene".to_string()));
    assert!(route.next_tools.contains(&"apply_edl".to_string()));
}

#[test]
fn routes_concrete_real_world_support_to_broll() {
    let route = route_visual_support_request(
        "when they mention the dashboard, show supporting footage of it",
    );

    assert_eq!(route.primary_lane, VisualSupportLane::Broll);
    assert!(
        route
            .next_tools
            .contains(&"find_broll_opportunities".to_string())
    );
}

#[test]
fn routes_still_asset_overlay_to_motion_scene() {
    let route = route_visual_support_request(
        "add a product screenshot card with the logo as an animated overlay",
    );

    assert_eq!(route.primary_lane, VisualSupportLane::MotionScene);
    assert!(route.next_tools.contains(&"plan_motion_scene".to_string()));
}

#[test]
fn routes_actual_generated_video_to_broll() {
    let route = route_visual_support_request("show AI-generated video b-roll of the warehouse");

    assert_eq!(route.primary_lane, VisualSupportLane::Broll);
    assert!(route.supporting_lanes.is_empty());
}

#[test]
fn routes_simple_identity_label_to_title_annotation() {
    let route = route_visual_support_request("add the guest name and title on screen");

    assert_eq!(route.primary_lane, VisualSupportLane::TitleAnnotation);
    assert!(route.next_tools.contains(&"apply_edl".to_string()));
}

#[test]
fn routes_direct_footage_polish_to_effects() {
    let route = route_visual_support_request("blur the face and make this shot warmer");

    assert_eq!(route.primary_lane, VisualSupportLane::EffectsFinishing);
    assert!(route.next_tools.contains(&"apply_edl".to_string()));
}

#[test]
fn temporal_highlight_request_does_not_route_to_motion_scene() {
    // "highlight the funniest moment" is editorial moment-selection,
    // not a visual highlight-box overlay — it must not hit MotionScene.
    let route = route_visual_support_request("highlight the funniest moment in this clip");
    assert_ne!(route.primary_lane, VisualSupportLane::MotionScene);
}

#[test]
fn highlight_box_phrase_routes_to_motion_scene() {
    let route = route_visual_support_request("highlight box on the chart");
    assert_eq!(route.primary_lane, VisualSupportLane::MotionScene);
    assert!(route.next_tools.contains(&"plan_motion_scene".to_string()));
}

#[test]
fn highlight_the_spatial_noun_routes_to_motion_scene() {
    let route = route_visual_support_request("highlight the area around the price");
    assert_eq!(route.primary_lane, VisualSupportLane::MotionScene);
    assert!(route.next_tools.contains(&"plan_motion_scene".to_string()));
}

#[test]
fn exposes_supporting_lanes_for_hybrid_visual_requests() {
    let route = route_visual_support_request(
        "make this podcast segment more energetic with b-roll, captions, and animated callouts",
    );

    assert_eq!(route.primary_lane, VisualSupportLane::MotionScene);
    assert!(route.supporting_lanes.contains(&VisualSupportLane::Broll));
    assert!(
        route
            .supporting_lanes
            .contains(&VisualSupportLane::TitleAnnotation)
    );
}

#[test]
fn exposes_visual_reasoning_for_explainer_hybrid_requests() {
    let route = route_visual_support_request(
        "explain the 3 step onboarding process with a product screenshot and supporting b-roll",
    );

    assert_eq!(route.primary_lane, VisualSupportLane::MotionScene);
    assert!(route.supporting_lanes.contains(&VisualSupportLane::Broll));
    assert!(
        route
            .needs
            .iter()
            .any(|need| need.kind == VisualSupportNeedKind::ListOrProcess)
    );
    assert!(
        route
            .needs
            .iter()
            .any(|need| need.kind == VisualSupportNeedKind::ProductOrAssetMention)
    );
    assert!(route.intents.contains(&VisualSupportIntent::Explain));
    assert!(route.intents.contains(&VisualSupportIntent::ShowEvidence));
    assert!(route.plan_steps.iter().any(|step| {
        step.lane == VisualSupportLane::MotionScene && step.tool == "plan_motion_scene"
    }));
    assert!(route.plan_steps.iter().any(|step| {
        step.lane == VisualSupportLane::Broll && step.tool == "find_broll_opportunities"
    }));
}

#[test]
fn routes_plain_lower_third_to_title_annotation() {
    let route = route_visual_support_request("add a lower third with the guest name and title");

    assert_eq!(route.primary_lane, VisualSupportLane::TitleAnnotation);
    assert!(route.next_tools.contains(&"apply_edl".to_string()));
}

#[test]
fn routes_animated_lower_third_to_motion_scene_with_template_hint() {
    let route = route_visual_support_request("add an animated lower third for the guest");

    assert_eq!(route.primary_lane, VisualSupportLane::MotionScene);
    assert!(route.next_tools.contains(&"plan_motion_scene".to_string()));
    let step = route
        .plan_steps
        .iter()
        .find(|step| {
            step.lane == VisualSupportLane::MotionScene && step.tool == "plan_motion_scene"
        })
        .expect("motion scene planning step");
    assert!(
        step.action.contains("plan_motion_scene") && step.action.contains("template=lower_third"),
        "action should point at the template call: {}",
        step.action
    );
}

#[test]
fn routes_lower_third_template_request_to_motion_scene() {
    let route = route_visual_support_request("use the lower third template for the host name");

    assert_eq!(route.primary_lane, VisualSupportLane::MotionScene);
    let step = route
        .plan_steps
        .iter()
        .find(|step| {
            step.lane == VisualSupportLane::MotionScene && step.tool == "plan_motion_scene"
        })
        .expect("motion scene planning step");
    assert!(
        step.action.contains("template=lower_third"),
        "action should name the lower_third template: {}",
        step.action
    );
}

#[test]
fn routes_kinetic_text_request_to_motion_scene_with_template_hint() {
    let route = route_visual_support_request("do a kinetic text callout for the tagline");

    assert_eq!(route.primary_lane, VisualSupportLane::MotionScene);
    let step = route
        .plan_steps
        .iter()
        .find(|step| {
            step.lane == VisualSupportLane::MotionScene && step.tool == "plan_motion_scene"
        })
        .expect("motion scene planning step");
    assert!(
        step.action.contains("template=kinetic_text"),
        "action should name the kinetic_text template: {}",
        step.action
    );
}

#[test]
fn routes_progress_bar_request_to_motion_scene_with_template_hint() {
    let route = route_visual_support_request("show a progress bar as they go through the steps");

    assert_eq!(route.primary_lane, VisualSupportLane::MotionScene);
    let step = route
        .plan_steps
        .iter()
        .find(|step| {
            step.lane == VisualSupportLane::MotionScene && step.tool == "plan_motion_scene"
        })
        .expect("motion scene planning step");
    assert!(
        step.action.contains("template=progress_bar"),
        "action should name the progress_bar template: {}",
        step.action
    );
}

#[test]
fn routes_highlight_box_request_to_motion_scene_with_template_hint() {
    // "highlight the <spatial-noun>" phrasing: "region" is one of the
    // spatial nouns the tightened rule accepts (a bare "highlight the
    // signup button" no longer routes here — see
    // temporal_highlight_request_does_not_route_to_motion_scene).
    let route = route_visual_support_request("highlight the region around the signup button");

    assert_eq!(route.primary_lane, VisualSupportLane::MotionScene);
    let step = route
        .plan_steps
        .iter()
        .find(|step| {
            step.lane == VisualSupportLane::MotionScene && step.tool == "plan_motion_scene"
        })
        .expect("motion scene planning step");
    assert!(
        step.action.contains("template=highlight_box"),
        "action should name the highlight_box template: {}",
        step.action
    );
}

#[test]
fn routes_highlight_box_phrase_variant_to_motion_scene() {
    let route = route_visual_support_request("add a highlight box around the price");

    assert_eq!(route.primary_lane, VisualSupportLane::MotionScene);
    let step = route
        .plan_steps
        .iter()
        .find(|step| {
            step.lane == VisualSupportLane::MotionScene && step.tool == "plan_motion_scene"
        })
        .expect("motion scene planning step");
    assert!(
        step.action.contains("template=highlight_box"),
        "action should name the highlight_box template: {}",
        step.action
    );
}

#[test]
fn motion_scene_plan_step_names_required_content_args() {
    let route = route_visual_support_request(
        "turn this into a three-step animated card with a product screenshot",
    );

    let step = route
        .plan_steps
        .iter()
        .find(|step| {
            step.lane == VisualSupportLane::MotionScene && step.tool == "plan_motion_scene"
        })
        .expect("motion scene planning step");

    assert!(
        step.action.contains("headline") && step.action.contains("evidence_text"),
        "plan_motion_scene now rejects request-only calls; action should name content args: {}",
        step.action
    );
    assert!(
        step.action.contains("step_labels"),
        "step/process MotionScenes must pass exact labels: {}",
        step.action
    );
}
