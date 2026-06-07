use montage_core::montage_mcp::context::McpToolCtx;
use montage_core::montage_mcp::tools::plan_transition::{PlanTransitionArgs, run};
use std::error::Error;

fn context(
    motion_match: &str,
    direction: Option<&str>,
    occlusion_score: Option<f64>,
) -> serde_json::Value {
    let outgoing = side(direction, occlusion_score);
    let incoming = side(direction, occlusion_score);
    serde_json::json!({
        "between": {
            "from": {"clip_uuid": "outgoing-clip"},
            "to": {"clip_uuid": "incoming-clip"}
        },
        "handles": {
            "max_centered_duration_s": 0.7
        },
        "continuity": {
            "verdict": "risky"
        },
        "visual_signals": {
            "outgoing": outgoing,
            "incoming": incoming,
            "motion_match": motion_match,
            "motion_match_confidence": 0.88,
            "either_side_static": false
        },
        "missing_signals": []
    })
}

fn side(direction: Option<&str>, occlusion_score: Option<f64>) -> serde_json::Value {
    serde_json::json!({
        "motion_direction": direction,
        "motion_magnitude": 0.8,
        "whip_pan_score": 0.7,
        "occlusion_score": occlusion_score
    })
}

fn plan(
    objective: &str,
    direction: Option<&str>,
    context: serde_json::Value,
) -> Result<serde_json::Value, Box<dyn Error>> {
    let out = run(
        PlanTransitionArgs {
            context,
            objective: Some(objective.into()),
            direction: direction.map(str::to_string),
        },
        McpToolCtx {
            project_root: std::env::temp_dir(),
        },
    )?;
    Ok(serde_json::from_str(&out)?)
}

fn assert_edl_parses(body: &serde_json::Value) -> Result<(), Box<dyn Error>> {
    let edl = body
        .pointer("/edl_fragment")
        .and_then(serde_json::Value::as_str)
        .ok_or("missing EDL fragment")?;
    montage_core::edl::parse(edl)?;
    Ok(())
}

#[test]
fn occlusion_job_selects_directional_pass_by() -> Result<(), Box<dyn Error>> {
    let body = plan(
        "invisible_scene_move",
        Some("left"),
        context("aligned", Some("left"), Some(0.82)),
    )?;

    assert_eq!(
        body.pointer("/recommended/decision")
            .and_then(|value| value.as_str()),
        Some("transition")
    );
    assert_eq!(
        body.pointer("/recommended/id")
            .and_then(|value| value.as_str()),
        Some("montage.pass_by_left")
    );
    assert_edl_parses(&body)
}

#[test]
fn invisible_cut_without_occlusion_refuses_to_hard_cut() -> Result<(), Box<dyn Error>> {
    let body = plan("mask_cut", None, context("unknown", None, Some(0.2)))?;

    assert_eq!(
        body.pointer("/recommended/decision")
            .and_then(|value| value.as_str()),
        Some("hard_cut")
    );
    assert_eq!(
        body.pointer("/recommended/intent")
            .and_then(|value| value.as_str()),
        Some("transition_incompatible")
    );
    assert_edl_parses(&body)
}

#[test]
fn punch_in_job_selects_zoom_in() -> Result<(), Box<dyn Error>> {
    let body = plan("punch_in", Some("in"), context("unknown", None, None))?;

    assert_eq!(
        body.pointer("/recommended/id")
            .and_then(|value| value.as_str()),
        Some("montage.zoom_in")
    );
    assert_edl_parses(&body)
}

#[test]
fn stylized_reveal_and_closure_select_iris_variants() -> Result<(), Box<dyn Error>> {
    let reveal = plan(
        "stylized_reveal",
        Some("out"),
        context("unknown", None, None),
    )?;
    let closure = plan(
        "stylized_closure",
        Some("in"),
        context("unknown", None, None),
    )?;

    assert_eq!(
        reveal
            .pointer("/recommended/id")
            .and_then(|value| value.as_str()),
        Some("montage.iris_open")
    );
    assert_eq!(
        closure
            .pointer("/recommended/id")
            .and_then(|value| value.as_str()),
        Some("montage.iris_close")
    );
    assert_edl_parses(&reveal)?;
    assert_edl_parses(&closure)
}

#[test]
fn directional_graphic_movement_selects_wipe() -> Result<(), Box<dyn Error>> {
    let body = plan(
        "graphic_movement",
        Some("up"),
        context("aligned", Some("up"), None),
    )?;

    assert_eq!(
        body.pointer("/recommended/id")
            .and_then(|value| value.as_str()),
        Some("montage.wipe_up")
    );
    assert_edl_parses(&body)
}
