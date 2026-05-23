//! Integration coverage for the agent-facing stream remux tool.

use awidat_core::tool::{SandboxMode, ToolContext, ToolHandler, ToolInvocation};
use awidat_core::tools::stream_remux::StreamRemuxTool;
use tokio::sync::broadcast;

fn ctx_at(root: &std::path::Path) -> ToolContext {
    let (tx, _) = broadcast::channel(8);
    ToolContext {
        project_root: root.to_path_buf(),
        events_tx: tx,
        user_input_tx: None,
        job_manager: awidat_render::JobManager::new(),
        approval_tx: None,
        sandbox_mode: SandboxMode::Default,
        mcp_host: awidat_core::mcp_host::McpHost::new(awidat_mcp::ClientInfo {
            name: "test".into(),
            version: "0.0.0".into(),
        }),
        skills: std::sync::Arc::new(awidat_core::skills::SkillRegistry::default()),
        subagent_return: None,
    }
}

fn invocation(args: serde_json::Value) -> ToolInvocation {
    ToolInvocation {
        call_id: "c1".into(),
        name: "stream_remux".into(),
        args,
    }
}

#[tokio::test]
async fn stream_remux_writes_manifest_and_starts_copy_job() -> Result<(), Box<dyn std::error::Error>>
{
    let dir = tempfile::tempdir()?;
    let input = dir.path().join("raw/source.mp4");
    let input_parent = input.parent().ok_or("input path has no parent")?;
    std::fs::create_dir_all(input_parent)?;
    std::fs::write(&input, b"not-real-media")?;

    let output = "renders/remux/source.mkv";
    let out = StreamRemuxTool
        .handle(
            invocation(serde_json::json!({
                "input": "raw/source.mp4",
                "output": output,
                "container": "matroska",
                "streams": [
                    {"id": "v0", "kind": "video", "source_index": 0, "mode": "copy"},
                    {"id": "a0", "kind": "audio", "source_index": 1, "mode": "copy"}
                ],
                "metadata": {"title": "Lossless review export"}
            })),
            ctx_at(dir.path()),
        )
        .await?;

    let body: serde_json::Value = serde_json::from_str(&out.content)?;
    assert_eq!(body["render_kind"], "stream_export_remux");
    assert_eq!(
        body["output_path"],
        dir.path().join(output).display().to_string()
    );
    assert_eq!(
        body["remux_evidence"]["remux_backend"],
        "stream_export_remux"
    );
    assert_eq!(
        body["remux_evidence"]["remux_eligibility_reason"],
        "explicit_stream_copy_contract"
    );
    assert_eq!(body["remux_evidence"]["copy_stream_count"], "2");
    assert_eq!(body["remux_evidence"]["transcode_stream_count"], "0");
    assert_eq!(body["remux_evidence"]["all_streams_copy"], "true");
    assert_eq!(
        body["remux_evidence"]["render_feature_id"],
        "stream_copy_remux"
    );
    assert_eq!(
        body["remux_evidence"]["render_feature_export_supported"],
        "supported"
    );
    assert!(body["job_id"].as_str().is_some());

    let manifest_path = dir.path().join("renders/remux/source.render-manifest.json");
    let manifest = awidat_render::read_render_manifest(&manifest_path)?;
    assert_eq!(
        manifest.backend,
        awidat_render::RenderBackendKind::StreamExportRemux
    );
    assert_eq!(manifest.metadata["container"], "matroska");
    assert_eq!(
        manifest.metadata["contract_id"],
        "stream-remux-raw-source-mp4"
    );
    assert_eq!(manifest.metadata["remux_backend"], "stream_export_remux");
    assert_eq!(
        manifest.metadata["remux_eligibility_reason"],
        "explicit_stream_copy_contract"
    );
    assert_eq!(manifest.metadata["copy_stream_count"], "2");
    assert_eq!(manifest.metadata["transcode_stream_count"], "0");
    assert_eq!(manifest.metadata["all_streams_copy"], "true");
    assert_eq!(manifest.metadata["render_feature_id"], "stream_copy_remux");
    assert_eq!(
        manifest.metadata["render_feature_export_supported"],
        "supported"
    );
    assert!(matches!(
        manifest.replay,
        awidat_render::RenderReplayPlan::FfmpegArgv { .. }
    ));
    let awidat_render::RenderReplayPlan::FfmpegArgv { argv, cwd } = manifest.replay else {
        unreachable!();
    };
    assert_eq!(cwd.as_deref(), Some(dir.path().to_string_lossy().as_ref()));
    assert!(argv.windows(2).any(|pair| pair == ["-c:0", "copy"]));
    assert!(argv.windows(2).any(|pair| pair == ["-c:1", "copy"]));
    assert!(argv.windows(2).any(|pair| pair == ["-f", "matroska"]));
    Ok(())
}

#[tokio::test]
async fn stream_remux_rejects_unsafe_paths_before_starting_job()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let result = StreamRemuxTool
        .handle(
            invocation(serde_json::json!({
                "input": "../outside.mp4",
                "output": "renders/remux/out.mkv",
                "container": "matroska",
                "streams": [{"id": "v0", "source_index": 0, "mode": "copy"}]
            })),
            ctx_at(dir.path()),
        )
        .await;
    let err = match result {
        Ok(_) => return Err("stream_remux accepted an unsafe input path".into()),
        Err(err) => err,
    };

    assert!(err.to_string().contains("project-relative"));
    assert_eq!(ctx_at(dir.path()).job_manager.job_count().await, 0);
    Ok(())
}
