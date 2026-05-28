//! `vedit_merge` tool — execute bounded local vedit branch merges.

use async_trait::async_trait;
use serde::Deserialize;

use crate::FunctionCallError;
use crate::tool::{ApprovalKey, ToolContext, ToolHandler, ToolInvocation, ToolOutput};
use crate::tool_schema::Tool as ToolSchema;
use crate::vc;

/// The `vedit_merge` tool.
pub struct VeditMergeTool;

#[derive(Debug, Deserialize)]
struct Args {
    /// Source ref to merge.
    source: String,
    /// Target branch/ref to receive the merge. Defaults to HEAD.
    #[serde(default)]
    target: Option<String>,
}

#[async_trait]
impl ToolHandler for VeditMergeTool {
    fn name(&self) -> &'static str {
        "vedit_merge"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "vedit_merge".into(),
            description: DESCRIPTION.into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "source": {
                        "type": "string",
                        "description": "Source branch, tag, commit hash, or ref to merge."
                    },
                    "target": {
                        "type": "string",
                        "description": "Target branch/ref to receive the merge. Defaults to HEAD."
                    }
                },
                "required": ["source"]
            }),
            ..Default::default()
        }
    }

    fn is_mutating(&self, _invocation: &ToolInvocation) -> bool {
        true
    }

    fn approval_keys(&self, invocation: &ToolInvocation) -> Vec<ApprovalKey> {
        let source = invocation
            .args
            .get("source")
            .and_then(|v| v.as_str())
            .unwrap_or("<missing-source>");
        let target = invocation
            .args
            .get("target")
            .and_then(|v| v.as_str())
            .unwrap_or("HEAD");
        vec![ApprovalKey::new(
            "vedit_merge",
            format!("source={source}:target={target}"),
        )]
    }

    async fn handle(
        &self,
        invocation: ToolInvocation,
        ctx: ToolContext,
    ) -> Result<ToolOutput, FunctionCallError> {
        let args: Args = serde_json::from_value(invocation.args).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "vedit_merge: invalid args ({e}). Required: {{ \"source\": string, \"target\"?: string }}."
            ))
        })?;
        let repo = vc::open_or_init(&ctx.project_root).map_err(|e| {
            FunctionCallError::RespondToModel(format!("vedit_merge: opening repo failed: {e}"))
        })?;
        let outcome = vc::merge_refs(&repo, &args.source, args.target.as_deref())
            .map_err(|e| FunctionCallError::RespondToModel(format!("vedit_merge: {e}")))?;
        let body = serde_json::json!({
            "commit_hash": outcome.commit_hash,
            "timeline_hash": outcome.timeline_hash,
            "message": outcome.message,
            "source_ref": outcome.source_ref,
            "target_ref": outcome.target_ref,
            "source_commit": outcome.source_commit,
            "target_commit": outcome.target_commit,
            "merge_base": outcome.merge_base,
            "parents": outcome.parents,
            "parent_count": outcome.parents.len(),
            "source_changed_clip_ids": outcome.source_changed_clip_ids,
            "target_changed_clip_ids": outcome.target_changed_clip_ids,
            "note": "Merged after bounded preflight confirmed non-overlapping changed clip/media ids.",
        });
        Ok(ToolOutput::text(body.to_string()))
    }
}

const DESCRIPTION: &str = "\
Merge a source vedit ref into a target branch/ref using Awidat's bounded \
non-overlapping clip-id rule. The merge first runs the same preflight as \
vedit_merge_preflight. If any changed clip/media ids overlap, it refuses \
the merge and reports a conflict. On success it writes project.otio.json, \
advances the target branch, and creates a two-parent merge commit.\
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

    fn write_otio(project_root: &std::path::Path, a_duration: f64, b_duration: f64) {
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
                    "children": [
                        {
                            "OTIO_SCHEMA": "Clip.2",
                            "name": "shot-a",
                            "source_range": {
                                "OTIO_SCHEMA": "TimeRange.1",
                                "start_time": {"OTIO_SCHEMA": "RationalTime.1", "value": 0.0, "rate": 24.0},
                                "duration": {"OTIO_SCHEMA": "RationalTime.1", "value": a_duration, "rate": 24.0}
                            },
                            "media_reference": {
                                "OTIO_SCHEMA": "ExternalReference.1",
                                "target_url": "raw/a.mp4"
                            }
                        },
                        {
                            "OTIO_SCHEMA": "Clip.2",
                            "name": "shot-b",
                            "source_range": {
                                "OTIO_SCHEMA": "TimeRange.1",
                                "start_time": {"OTIO_SCHEMA": "RationalTime.1", "value": 0.0, "rate": 24.0},
                                "duration": {"OTIO_SCHEMA": "RationalTime.1", "value": b_duration, "rate": 24.0}
                            },
                            "media_reference": {
                                "OTIO_SCHEMA": "ExternalReference.1",
                                "target_url": "raw/b.mp4"
                            }
                        }
                    ]
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
    async fn merges_non_overlapping_branch_and_reports_commit() {
        let dir = tempfile::tempdir().unwrap();
        write_otio(dir.path(), 240.0, 240.0);
        let repo = vc::open_or_init(dir.path()).unwrap();
        let base = vc::commit_current_timeline(&repo, "Initial", None).unwrap();
        vc::create_branch(&repo, "alt-tight", Some(&base.commit_hash)).unwrap();

        write_otio(dir.path(), 240.0, 120.0);
        vc::commit_current_timeline(&repo, "Trim shot-b on main", None).unwrap();

        vc::checkout_branch(&repo, "alt-tight").unwrap();
        write_otio(dir.path(), 120.0, 240.0);
        vc::commit_current_timeline(&repo, "Trim shot-a on alternate", None).unwrap();

        let out = super::VeditMergeTool
            .handle(
                ToolInvocation {
                    call_id: "c1".into(),
                    name: "vedit_merge".into(),
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
        assert_eq!(parsed["source_ref"].as_str(), Some("alt-tight"));
        assert_eq!(parsed["target_ref"].as_str(), Some(vc::DEFAULT_BRANCH));
        assert_eq!(parsed["parent_count"].as_u64(), Some(2));
        assert_eq!(
            parsed["source_changed_clip_ids"][0].as_str(),
            Some("raw/a.mp4")
        );
        assert_eq!(
            parsed["target_changed_clip_ids"][0].as_str(),
            Some("raw/b.mp4")
        );
        assert!(
            parsed["commit_hash"]
                .as_str()
                .unwrap()
                .starts_with("sha256:")
        );
    }
}
