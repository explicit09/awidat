//! `vedit_changed_clip_ids` tool — list clip/media ids touched by a ref diff.

use async_trait::async_trait;
use serde::Deserialize;

use crate::FunctionCallError;
use crate::anthropic::Tool as ToolSchema;
use crate::tool::{ToolContext, ToolHandler, ToolInvocation, ToolOutput};
use crate::vc;

/// The `vedit_changed_clip_ids` tool.
pub struct VeditChangedClipIdsTool;

#[derive(Debug, Deserialize)]
struct Args {
    /// From-ref. Default `"session-start"`.
    #[serde(default)]
    from: Option<String>,
    /// To-ref. Default `"HEAD"`.
    #[serde(default)]
    to: Option<String>,
}

#[async_trait]
impl ToolHandler for VeditChangedClipIdsTool {
    fn name(&self) -> &'static str {
        "vedit_changed_clip_ids"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "vedit_changed_clip_ids".into(),
            description: DESCRIPTION.into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "from": {
                        "type": "string",
                        "description": "From-ref. Default 'session-start'."
                    },
                    "to": {
                        "type": "string",
                        "description": "To-ref. Default 'HEAD'."
                    }
                }
            }),
            ..Default::default()
        }
    }

    fn is_mutating(&self, _invocation: &ToolInvocation) -> bool {
        false
    }

    async fn handle(
        &self,
        invocation: ToolInvocation,
        ctx: ToolContext,
    ) -> Result<ToolOutput, FunctionCallError> {
        let args: Args = serde_json::from_value(invocation.args).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "vedit_changed_clip_ids: invalid args ({e}). Both fields optional; default is session-start..HEAD."
            ))
        })?;
        let repo = vc::open_or_init(&ctx.project_root).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "vedit_changed_clip_ids: opening repo failed: {e}"
            ))
        })?;
        let diff = vc::diff_refs(&repo, args.from.as_deref(), args.to.as_deref()).map_err(|e| {
            FunctionCallError::RespondToModel(format!("vedit_changed_clip_ids: {e}"))
        })?;
        let changed_clip_ids = vc::changed_clip_ids(&diff);
        let body = serde_json::json!({
            "from": diff.from_ref,
            "to": diff.to_ref,
            "changed_clip_count": changed_clip_ids.len(),
            "changed_clip_ids": changed_clip_ids,
            "structural_change_count": diff.changes.len(),
            "animation_change_count": diff.animation_changes.len(),
            "note": if diff.is_empty() {
                "No structural or animation changes touched clip/media identifiers in this ref range."
            } else {
                ""
            },
        });
        Ok(ToolOutput::text(body.to_string()))
    }
}

const DESCRIPTION: &str = "\
List the sorted clip names, media references, and clip animation targets \
touched by a vedit diff. Default: from='session-start', to='HEAD'. \
This is read-only preflight data for history review or future merge \
conflict checks; it does not checkout, merge, or modify refs.\
";

#[cfg(test)]
mod tests {
    use crate::FunctionCallError;
    use crate::tool::{ToolContext, ToolHandler, ToolInvocation};
    use crate::vc;

    fn ctx_for(project_root: std::path::PathBuf) -> ToolContext {
        use std::sync::Arc;
        use tokio::sync::broadcast;
        let (tx, _) = broadcast::channel(8);
        ToolContext {
            project_root,
            events_tx: tx,
            user_input_tx: None,
            job_manager: awidat_render::JobManager::new(),
            approval_tx: None,
            sandbox_mode: crate::tool::SandboxMode::Default,
            mcp_host: crate::mcp_host::McpHost::new(awidat_mcp::ClientInfo {
                name: "test".into(),
                version: "0.0.0".into(),
            }),
            skills: Arc::new(crate::skills::SkillRegistry::default()),
            subagent_return: None,
        }
    }

    fn write_otio(project_root: &std::path::Path, duration: f64) {
        let value = serde_json::json!({
            "OTIO_SCHEMA": "Timeline.1",
            "name": "test",
            "tracks": {
                "OTIO_SCHEMA": "Stack.1",
                "name": "tracks",
                "children": [{
                    "OTIO_SCHEMA": "Track.1",
                    "name": "V1",
                    "kind": "Video",
                    "children": [{
                        "OTIO_SCHEMA": "Clip.2",
                        "name": "shot-a",
                        "source_range": {
                            "OTIO_SCHEMA": "TimeRange.1",
                            "start_time": {"OTIO_SCHEMA": "RationalTime.1", "value": 0.0, "rate": 24.0},
                            "duration": {"OTIO_SCHEMA": "RationalTime.1", "value": duration, "rate": 24.0}
                        },
                        "media_reference": {
                            "OTIO_SCHEMA": "ExternalReference.1",
                            "target_url": "raw/foo.mp4"
                        }
                    }]
                }]
            }
        });
        std::fs::write(
            project_root.join("project.otio.json"),
            serde_json::to_vec_pretty(&value).unwrap(),
        )
        .unwrap();
    }

    #[tokio::test]
    async fn returns_changed_clip_ids_for_ref_diff() {
        let dir = tempfile::tempdir().unwrap();
        write_otio(dir.path(), 240.0);
        let repo = vc::open_or_init(dir.path()).unwrap();
        let first = vc::commit_current_timeline(&repo, "Initial", None).unwrap();
        write_otio(dir.path(), 120.0);
        let second = vc::commit_current_timeline(&repo, "Trim shot-a", None).unwrap();

        let out = super::VeditChangedClipIdsTool
            .handle(
                ToolInvocation {
                    call_id: "c1".into(),
                    name: "vedit_changed_clip_ids".into(),
                    args: serde_json::json!({
                        "from": first.commit_hash,
                        "to": second.commit_hash
                    }),
                },
                ctx_for(dir.path().to_path_buf()),
            )
            .await
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&out.content).unwrap();
        assert_eq!(parsed["changed_clip_count"].as_u64(), Some(2));
        assert_eq!(parsed["changed_clip_ids"][0].as_str(), Some("raw/foo.mp4"));
        assert_eq!(parsed["changed_clip_ids"][1].as_str(), Some("shot-a"));
        assert_eq!(parsed["structural_change_count"].as_u64(), Some(1));
        assert_eq!(parsed["animation_change_count"].as_u64(), Some(0));
    }

    #[tokio::test]
    async fn unknown_ref_surfaces_a_clear_error() {
        let dir = tempfile::tempdir().unwrap();
        write_otio(dir.path(), 240.0);
        let repo = vc::open_or_init(dir.path()).unwrap();
        vc::commit_current_timeline(&repo, "Initial", None).unwrap();

        let err = super::VeditChangedClipIdsTool
            .handle(
                ToolInvocation {
                    call_id: "c1".into(),
                    name: "vedit_changed_clip_ids".into(),
                    args: serde_json::json!({"from": "no-such-ref"}),
                },
                ctx_for(dir.path().to_path_buf()),
            )
            .await
            .unwrap_err();
        match err {
            FunctionCallError::RespondToModel(msg) => assert!(msg.contains("no-such-ref")),
            other => panic!("unexpected: {other:?}"),
        }
    }
}
