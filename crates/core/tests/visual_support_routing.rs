//! Visual-support routing policy tests.

use awidat_core::tools::plan_visual_support::{VisualSupportLane, route_visual_support_request};

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
        "when they mention the dashboard, show supporting footage or a screenshot of it",
    );

    assert_eq!(route.primary_lane, VisualSupportLane::Broll);
    assert!(
        route
            .next_tools
            .contains(&"find_broll_opportunities".to_string())
    );
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
