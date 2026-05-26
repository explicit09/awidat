//! `vedit_merge_preflight` tool — check branch merge overlap without merging.

use async_trait::async_trait;
use serde::Deserialize;

use crate::FunctionCallError;
use crate::tool_schema::Tool as ToolSchema;
use crate::tool::{ToolContext, ToolHandler, ToolInvocation, ToolOutput};
use crate::vc;

/// The `vedit_merge_preflight` tool.
pub struct VeditMergePreflightTool;

#[derive(Debug, Deserialize)]
struct Args {
    /// Source ref that would be merged.
    source: String,
    /// Target ref that would receive source. Default `"HEAD"`.
    #[serde(default)]
    target: Option<String>,
}

#[async_trait]
impl ToolHandler for VeditMergePreflightTool {
    fn name(&self) -> &'static str {
        "vedit_merge_preflight"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "vedit_merge_preflight".into(),
            description: DESCRIPTION.into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "source": {
                        "type": "string",
                        "description": "Source branch, tag, commit hash, or ref that would be merged."
                    },
                    "target": {
                        "type": "string",
                        "description": "Target branch, tag, commit hash, or ref. Defaults to HEAD."
                    }
                },
                "required": ["source"]
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
                "vedit_merge_preflight: invalid args ({e}). Required: {{ \"source\": string, \"target\"?: string }}."
            ))
        })?;
        let repo = vc::open_or_init(&ctx.project_root).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "vedit_merge_preflight: opening repo failed: {e}"
            ))
        })?;
        let preflight =
            vc::merge_preflight(&repo, &args.source, args.target.as_deref()).map_err(|e| {
                FunctionCallError::RespondToModel(format!("vedit_merge_preflight: {e}"))
            })?;
        let body = serde_json::json!({
            "source_ref": preflight.source_ref,
            "target_ref": preflight.target_ref,
            "source_commit": preflight.source_commit,
            "target_commit": preflight.target_commit,
            "merge_base": preflight.merge_base,
            "is_mergeable": preflight.is_mergeable,
            "source_changed_clip_ids": preflight.source_changed_clip_ids,
            "target_changed_clip_ids": preflight.target_changed_clip_ids,
            "overlapping_clip_ids": preflight.overlapping_clip_ids,
            "source_change_count": preflight.source_change_count,
            "target_change_count": preflight.target_change_count,
            "next_step": if preflight.is_mergeable {
                "A future bounded merge can proceed under the non-overlapping clip-id rule; this preflight is read-only and does not merge refs."
            } else {
                "Resolve the overlapping clip ids manually; this preflight is read-only and does not merge refs."
            },
        });
        Ok(ToolOutput::text(body.to_string()))
    }
}

const DESCRIPTION: &str = "\
Check whether a source ref can be safely merged into a target ref under \
Awidat's proposed bounded merge rule: both sides must have changed \
non-overlapping clip/media identifiers since their common ancestor. \
This tool is read-only; it does not checkout, merge, resolve conflicts, \
or modify refs.\
";

#[cfg(test)]
mod tests {
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
    async fn reports_overlapping_clip_conflict_without_mutating() {
        let dir = tempfile::tempdir().unwrap();
        write_otio(dir.path(), 240.0);
        let repo = vc::open_or_init(dir.path()).unwrap();
        let base = vc::commit_current_timeline(&repo, "Initial", None).unwrap();
        vc::create_branch(&repo, "alt-tight", Some(&base.commit_hash)).unwrap();

        write_otio(dir.path(), 120.0);
        vc::commit_current_timeline(&repo, "Trim shot-a on main", None).unwrap();

        vc::checkout_branch(&repo, "alt-tight").unwrap();
        write_otio(dir.path(), 180.0);
        vc::commit_current_timeline(&repo, "Trim shot-a on alternate", None).unwrap();

        let out = super::VeditMergePreflightTool
            .handle(
                ToolInvocation {
                    call_id: "c1".into(),
                    name: "vedit_merge_preflight".into(),
                    args: serde_json::json!({
                        "source": "alt-tight",
                        "target": vc::DEFAULT_BRANCH
                    }),
                },
                ctx_for(dir.path().to_path_buf()),
            )
            .await
            .unwrap();

        let parsed: serde_json::Value = serde_json::from_str(&out.content).unwrap();
        assert_eq!(parsed["is_mergeable"].as_bool(), Some(false));
        assert_eq!(
            parsed["overlapping_clip_ids"][0].as_str(),
            Some("raw/foo.mp4")
        );
        assert_eq!(parsed["overlapping_clip_ids"][1].as_str(), Some("shot-a"));
        assert_eq!(
            parsed["next_step"].as_str(),
            Some(
                "Resolve the overlapping clip ids manually; this preflight is read-only and does not merge refs."
            )
        );
    }
}
