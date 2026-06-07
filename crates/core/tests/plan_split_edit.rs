use montage_core::montage_mcp::context::McpToolCtx;
use montage_core::montage_mcp::tools::plan_split_edit::{PlanSplitEditArgs, run};
use std::error::Error;

fn context() -> serde_json::Value {
    serde_json::json!({
        "between": {
            "from": {"clip_uuid": "outgoing-clip"},
            "to": {"clip_uuid": "incoming-clip"}
        },
        "from": {
            "clip_id": "outgoing-clip",
            "duration_s": 4.0
        },
        "to": {
            "clip_id": "incoming-clip",
            "duration_s": 4.0
        },
        "transcript": {
            "before": [
                {"text": "that", "start_s": 3.20, "end_s": 3.45},
                {"text": "matters", "start_s": 3.50, "end_s": 3.86}
            ],
            "after": [
                {"text": "because", "start_s": 0.22, "end_s": 0.50},
                {"text": "listen", "start_s": 0.55, "end_s": 0.88}
            ]
        },
        "missing_signals": []
    })
}

#[test]
fn j_cut_plan_emits_concrete_audio_lead_edl() -> Result<(), Box<dyn Error>> {
    let out = run(
        PlanSplitEditArgs {
            context: context(),
            objective: Some("speaker_handoff".into()),
            direction: Some("j_cut".into()),
        },
        McpToolCtx {
            project_root: std::env::temp_dir(),
        },
    )?;
    let body: serde_json::Value = serde_json::from_str(&out)?;

    assert_eq!(
        body.pointer("/recommended/decision")
            .and_then(|v| v.as_str()),
        Some("split_edit")
    );
    assert_eq!(
        body.pointer("/recommended/kind").and_then(|v| v.as_str()),
        Some("j_cut")
    );
    assert_eq!(
        body.pointer("/recommended/lead_s")
            .and_then(serde_json::Value::as_f64),
        Some(0.45)
    );

    let edl = body
        .pointer("/edl_fragment")
        .and_then(serde_json::Value::as_str)
        .ok_or("missing EDL fragment")?;
    assert!(edl.contains("*** Set Audio Lead"));
    assert!(edl.contains("@@ anchor: clip_uuid=incoming-clip"));
    assert!(edl.contains("+ lead_s: 0.450"));
    montage_core::edl::parse(edl)?;
    Ok(())
}

#[test]
fn l_cut_plan_emits_concrete_audio_trail_edl() -> Result<(), Box<dyn Error>> {
    let out = run(
        PlanSplitEditArgs {
            context: context(),
            objective: Some("thought_continuity".into()),
            direction: Some("l_cut".into()),
        },
        McpToolCtx {
            project_root: std::env::temp_dir(),
        },
    )?;
    let body: serde_json::Value = serde_json::from_str(&out)?;

    assert_eq!(
        body.pointer("/recommended/kind").and_then(|v| v.as_str()),
        Some("l_cut")
    );
    assert_eq!(
        body.pointer("/recommended/trail_s")
            .and_then(serde_json::Value::as_f64),
        Some(0.5)
    );

    let edl = body
        .pointer("/edl_fragment")
        .and_then(serde_json::Value::as_str)
        .ok_or("missing EDL fragment")?;
    assert!(edl.contains("*** Set Audio Trail"));
    assert!(edl.contains("@@ anchor: clip_uuid=outgoing-clip"));
    assert!(edl.contains("+ trail_s: 0.500"));
    montage_core::edl::parse(edl)?;
    Ok(())
}
