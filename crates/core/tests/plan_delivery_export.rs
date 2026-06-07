use montage_core::montage_mcp::context::McpToolCtx;
use montage_core::montage_mcp::tools::plan_delivery_export::{PlanDeliveryExportArgs, run};
use std::error::Error;

fn ctx() -> McpToolCtx {
    McpToolCtx {
        project_root: std::env::temp_dir(),
    }
}

#[test]
fn youtube_delivery_plan_sequences_preflight_package_and_verification() -> Result<(), Box<dyn Error>>
{
    let out = run(
        PlanDeliveryExportArgs {
            intent: "upload final video to YouTube with captions and thumbnail".into(),
            destination: Some("youtube".into()),
            needs_package: Some(true),
            prefer_remux: None,
        },
        ctx(),
    )?;
    let body: serde_json::Value = serde_json::from_str(&out)?;

    assert_eq!(
        body.pointer("/status").and_then(|v| v.as_str()),
        Some("ready")
    );
    assert_eq!(
        body.pointer("/recommended/delivery_kind")
            .and_then(|v| v.as_str()),
        Some("youtube")
    );
    assert_eq!(
        body.pointer("/recommended/export_preset_id")
            .and_then(|v| v.as_str()),
        Some("package_youtube_1080p")
    );
    assert_eq!(
        body.pointer("/recommended/package_format")
            .and_then(|v| v.as_str()),
        Some("youtube")
    );

    let tools = body
        .pointer("/follow_up_tools")
        .and_then(|v| v.as_array())
        .ok_or("missing follow-up tools")?;
    let names = tools
        .iter()
        .filter_map(|tool| tool.get("name").and_then(|v| v.as_str()))
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        vec![
            "render_preflight",
            "export_package",
            "poll_render",
            "verify_render"
        ]
    );
    assert!(tools.iter().any(|tool| {
        tool.get("name").and_then(|v| v.as_str()) == Some("export_package")
            && tool.pointer("/args/format").and_then(|v| v.as_str()) == Some("youtube")
    }));
    Ok(())
}

#[test]
fn archive_delivery_plan_uses_prores_render_and_local_review_package() -> Result<(), Box<dyn Error>>
{
    let out = run(
        PlanDeliveryExportArgs {
            intent: "create an archive master for future recut".into(),
            destination: Some("archive".into()),
            needs_package: Some(true),
            prefer_remux: None,
        },
        ctx(),
    )?;
    let body: serde_json::Value = serde_json::from_str(&out)?;

    assert_eq!(
        body.pointer("/recommended/delivery_kind")
            .and_then(|v| v.as_str()),
        Some("archive")
    );
    assert_eq!(
        body.pointer("/recommended/export_preset_id")
            .and_then(|v| v.as_str()),
        Some("archival_prores_hq_1080p")
    );
    assert_eq!(
        body.pointer("/recommended/container")
            .and_then(|v| v.as_str()),
        Some("mov")
    );

    let tools = body
        .pointer("/follow_up_tools")
        .and_then(|v| v.as_array())
        .ok_or("missing follow-up tools")?;
    assert!(tools.iter().any(|tool| {
        tool.get("name").and_then(|v| v.as_str()) == Some("start_render")
            && tool.pointer("/args/preset").and_then(|v| v.as_str()) == Some("prores")
    }));
    assert!(
        tools.iter().any(|tool| {
            tool.get("name").and_then(|v| v.as_str()) == Some("local_review_package")
        })
    );
    Ok(())
}

#[test]
fn remux_delivery_plan_routes_to_stream_remux_before_verification() -> Result<(), Box<dyn Error>> {
    let out = run(
        PlanDeliveryExportArgs {
            intent: "rewrap the approved H264 master as mp4 without re-encoding".into(),
            destination: Some("review".into()),
            needs_package: None,
            prefer_remux: Some(true),
        },
        ctx(),
    )?;
    let body: serde_json::Value = serde_json::from_str(&out)?;

    assert_eq!(
        body.pointer("/recommended/render_strategy")
            .and_then(|v| v.as_str()),
        Some("stream_remux")
    );
    let tools = body
        .pointer("/follow_up_tools")
        .and_then(|v| v.as_array())
        .ok_or("missing follow-up tools")?;
    assert_eq!(
        tools
            .first()
            .and_then(|tool| tool.get("name"))
            .and_then(|v| v.as_str()),
        Some("stream_remux")
    );
    assert!(
        tools
            .iter()
            .any(|tool| { tool.get("name").and_then(|v| v.as_str()) == Some("verify_render") })
    );
    Ok(())
}

#[test]
fn vague_delivery_intent_returns_review_questions() -> Result<(), Box<dyn Error>> {
    let out = run(
        PlanDeliveryExportArgs {
            intent: "export this".into(),
            destination: None,
            needs_package: None,
            prefer_remux: None,
        },
        ctx(),
    )?;
    let body: serde_json::Value = serde_json::from_str(&out)?;

    assert_eq!(
        body.pointer("/status").and_then(|v| v.as_str()),
        Some("needs_review")
    );
    assert!(body.pointer("/follow_up_tools").is_none());
    assert!(
        body.pointer("/review_questions")
            .and_then(|v| v.as_array())
            .is_some_and(|questions| !questions.is_empty())
    );
    Ok(())
}
