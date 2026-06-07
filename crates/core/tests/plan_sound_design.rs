use awidat_core::awidat_mcp::context::McpToolCtx;
use awidat_core::awidat_mcp::tools::plan_sound_design::{PlanSoundDesignArgs, run};
use std::error::Error;

fn ctx() -> McpToolCtx {
    McpToolCtx {
        project_root: std::env::temp_dir(),
    }
}

fn transition_context() -> serde_json::Value {
    serde_json::json!({
        "between": {
            "from": {"clip_uuid": "outgoing-clip"},
            "to": {"clip_uuid": "incoming-clip"}
        },
        "handles": {
            "max_centered_duration_s": 0.6
        },
        "visual_signals": {
            "motion_match": "aligned",
            "motion_match_confidence": 0.82
        },
        "transcript": {
            "before": [{"text": "watch", "start_s": 2.5, "end_s": 2.8}],
            "after": [{"text": "this", "start_s": 0.2, "end_s": 0.5}]
        },
        "missing_signals": []
    })
}

#[test]
fn motion_transition_plan_requests_whoosh_and_parseable_audio_edl() -> Result<(), Box<dyn Error>> {
    let out = run(
        PlanSoundDesignArgs {
            intent: "whoosh_transition".into(),
            context: transition_context(),
            start_s: Some(12.0),
            duration_s: None,
            target_lufs: Some(-14.0),
        },
        ctx(),
    )?;
    let body: serde_json::Value = serde_json::from_str(&out)?;

    assert_eq!(
        body.pointer("/recommended/kind").and_then(|v| v.as_str()),
        Some("sound_design")
    );
    assert_eq!(
        body.pointer("/recommended/asset_query/kind")
            .and_then(|v| v.as_str()),
        Some("sfx")
    );
    assert_eq!(
        body.pointer("/recommended/asset_query/mood")
            .and_then(|v| v.as_str()),
        Some("whoosh")
    );

    let edl = body
        .pointer("/edl_template")
        .and_then(|v| v.as_str())
        .ok_or("missing EDL template")?;
    assert!(edl.contains("*** Insert Clip"));
    assert!(edl.contains("+ track: SFX"));
    assert!(edl.contains("+ at_s: 11.900"));
    assert!(edl.contains("*** Set Loudness Target"));
    assert!(edl.contains("+ integrated_lufs: -14.000"));
    awidat_core::edl::parse(&edl.replace("<asset from find_audio_asset>", "raw/whoosh.wav"))?;

    let followups = body
        .pointer("/follow_up_tools")
        .and_then(|v| v.as_array())
        .ok_or("missing follow-up tools")?;
    assert!(followups.iter().any(|tool| {
        tool.pointer("/name").and_then(|v| v.as_str()) == Some("find_audio_asset")
    }));
    Ok(())
}

#[test]
fn dialogue_bed_plan_adds_ambience_bridge_and_l_cut() -> Result<(), Box<dyn Error>> {
    let out = run(
        PlanSoundDesignArgs {
            intent: "ambience_bridge".into(),
            context: transition_context(),
            start_s: Some(30.0),
            duration_s: Some(1.2),
            target_lufs: None,
        },
        ctx(),
    )?;
    let body: serde_json::Value = serde_json::from_str(&out)?;

    assert_eq!(
        body.pointer("/recommended/asset_query/kind")
            .and_then(|v| v.as_str()),
        Some("ambience")
    );
    assert_eq!(
        body.pointer("/recommended/split_edit/kind")
            .and_then(|v| v.as_str()),
        Some("l_cut")
    );
    let edl = body
        .pointer("/edl_template")
        .and_then(|v| v.as_str())
        .ok_or("missing EDL template")?;
    assert!(edl.contains("*** Set Audio Trail"));
    assert!(edl.contains("@@ anchor: clip_uuid=outgoing-clip"));
    assert!(edl.contains("*** Insert Clip"));
    awidat_core::edl::parse(&edl.replace("<asset from find_audio_asset>", "raw/room.wav"))?;
    Ok(())
}

#[test]
fn vague_plan_returns_review_status_without_edl() -> Result<(), Box<dyn Error>> {
    let out = run(
        PlanSoundDesignArgs {
            intent: "make it sound cool".into(),
            context: serde_json::json!({}),
            start_s: None,
            duration_s: None,
            target_lufs: None,
        },
        ctx(),
    )?;
    let body: serde_json::Value = serde_json::from_str(&out)?;

    assert_eq!(
        body.pointer("/status").and_then(|v| v.as_str()),
        Some("needs_review")
    );
    assert!(body.pointer("/edl_template").is_none());
    assert!(
        body.pointer("/review_questions")
            .and_then(|v| v.as_array())
            .is_some_and(|questions| !questions.is_empty())
    );
    Ok(())
}
