//! `vedit_blame` tool — project vedit history onto one clip.

use async_trait::async_trait;
use serde::Deserialize;

use crate::FunctionCallError;
use crate::tool::{ToolContext, ToolHandler, ToolInvocation, ToolOutput};
use crate::tool_schema::Tool as ToolSchema;
use crate::vc;

const DEFAULT_LIMIT: usize = 200;
const HARD_LIMIT: usize = 500;

/// The `vedit_blame` tool.
pub struct VeditBlameTool;

#[derive(Debug, Deserialize)]
struct Args {
    /// Clip name or media reference to blame.
    clip_id: String,
    /// Ref where the first-parent walk starts. Defaults to HEAD.
    #[serde(default)]
    start_ref: Option<String>,
    /// Max commits to inspect.
    #[serde(default)]
    limit: Option<usize>,
}

#[async_trait]
impl ToolHandler for VeditBlameTool {
    fn name(&self) -> &'static str {
        "vedit_blame"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "vedit_blame".into(),
            description: DESCRIPTION.into(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["clip_id"],
                "properties": {
                    "clip_id": {
                        "type": "string",
                        "description": "Clip name or media reference to project history onto."
                    },
                    "start_ref": {
                        "type": "string",
                        "description": "Commit hash, short hash, tag, branch, or HEAD. Defaults to HEAD."
                    },
                    "limit": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 500,
                        "description": "Max first-parent commits to inspect. Default 200."
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
                "vedit_blame: invalid args ({e}). Required: {{ \"clip_id\": string }}."
            ))
        })?;
        let clip_id = args.clip_id.trim();
        if clip_id.is_empty() {
            return Err(FunctionCallError::RespondToModel(
                "vedit_blame: clip_id cannot be empty".into(),
            ));
        }
        let limit = args.limit.unwrap_or(DEFAULT_LIMIT).min(HARD_LIMIT);
        let repo = vc::open_or_init(&ctx.project_root).map_err(|e| {
            FunctionCallError::RespondToModel(format!("vedit_blame: opening repo failed: {e}"))
        })?;
        let entries = vc::blame_clip(&repo, clip_id, args.start_ref.as_deref(), limit)
            .map_err(|e| FunctionCallError::RespondToModel(format!("vedit_blame: {e}")))?;
        let matches = entries
            .into_iter()
            .map(|entry| {
                serde_json::json!({
                    "commit_hash": entry.commit_hash,
                    "timeline_hash": entry.timeline_hash,
                    "timestamp": entry.timestamp,
                    "header": entry.header,
                    "full_message": entry.full_message,
                    "parents": entry.parents,
                    "structural_changes": entry.changes,
                    "animation_changes": entry.animation_changes,
                })
            })
            .collect::<Vec<_>>();
        let body = serde_json::json!({
            "clip_id": clip_id,
            "start_ref": args.start_ref.unwrap_or_else(|| "HEAD".to_string()),
            "limit_applied": limit,
            "match_count": matches.len(),
            "matches": matches,
            "note": if matches.is_empty() {
                "No first-parent commits within the limit touched this clip by name, media reference, or animation target string."
            } else {
                ""
            },
        });
        Ok(ToolOutput::text(body.to_string()))
    }
}

const DESCRIPTION: &str = "\
Project vedit history onto one clip. Walks first-parent history from \
HEAD (or start_ref), computes each commit's semantic diff, and returns \
commits whose changes touch the supplied clip name or media reference. \
This is attribution, not a branch checkout or merge operation.\
";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::ToolContext;

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
    async fn returns_matching_clip_history() {
        let dir = tempfile::tempdir().unwrap();
        write_otio(dir.path(), 240.0);
        let repo = vc::open_or_init(dir.path()).unwrap();
        vc::commit_current_timeline(&repo, "Initial", None).unwrap();
        write_otio(dir.path(), 120.0);
        vc::commit_current_timeline(&repo, "Trim shot-a", Some("Tighter pacing.")).unwrap();

        let out = VeditBlameTool
            .handle(
                ToolInvocation {
                    call_id: "c1".into(),
                    name: "vedit_blame".into(),
                    args: serde_json::json!({"clip_id": "shot-a"}),
                },
                ctx_for(dir.path().to_path_buf()),
            )
            .await
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&out.content).unwrap();
        assert_eq!(parsed["match_count"].as_u64(), Some(1));
        assert_eq!(parsed["matches"][0]["header"].as_str(), Some("Trim shot-a"));
    }
}
