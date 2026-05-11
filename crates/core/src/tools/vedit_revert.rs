//! `vedit_revert` tool — restore the working timeline to a prior vedit
//! commit/ref, optionally recording that restore as a new commit.

use async_trait::async_trait;
use serde::Deserialize;

use crate::FunctionCallError;
use crate::anthropic::Tool as ToolSchema;
use crate::tool::{ApprovalKey, ToolContext, ToolHandler, ToolInvocation, ToolOutput};
use crate::vc;

/// The `vedit_revert` tool.
pub struct VeditRevertTool;

#[derive(Debug, Deserialize)]
struct Args {
    /// Commit hash, short hash, branch name, or HEAD to restore from.
    #[serde(alias = "ref")]
    refstr: String,
    /// Whether to create an audit commit after restoring the working
    /// timeline. Defaults to true so the restore is visible in history.
    #[serde(default = "default_commit")]
    commit: bool,
    /// Optional commit header when `commit` is true.
    #[serde(default)]
    header: Option<String>,
    /// Optional reasoning body when `commit` is true.
    #[serde(default)]
    reasoning: Option<String>,
}

fn default_commit() -> bool {
    true
}

#[async_trait]
impl ToolHandler for VeditRevertTool {
    fn name(&self) -> &'static str {
        "vedit_revert"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "vedit_revert".into(),
            description: DESCRIPTION.into(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["refstr"],
                "properties": {
                    "refstr": {
                        "type": "string",
                        "description": "Commit hash, short hash, branch name, or HEAD to restore from."
                    },
                    "commit": {
                        "type": "boolean",
                        "description": "Create an audit commit after restoring. Defaults to true."
                    },
                    "header": {
                        "type": "string",
                        "description": "Optional one-line commit header when commit=true."
                    },
                    "reasoning": {
                        "type": "string",
                        "description": "Optional body explaining why the restore was requested."
                    }
                }
            }),
            ..Default::default()
        }
    }

    fn is_mutating(&self, _invocation: &ToolInvocation) -> bool {
        true
    }

    fn approval_keys(&self, invocation: &ToolInvocation) -> Vec<ApprovalKey> {
        let refstr = invocation
            .args
            .get("refstr")
            .or_else(|| invocation.args.get("ref"))
            .and_then(|v| v.as_str())
            .unwrap_or("<missing-ref>")
            .trim();
        let commit = invocation
            .args
            .get("commit")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        vec![ApprovalKey::new(
            "vedit_revert",
            format!("restore:{refstr}:audit_commit={commit}"),
        )]
    }

    async fn handle(
        &self,
        invocation: ToolInvocation,
        ctx: ToolContext,
    ) -> Result<ToolOutput, FunctionCallError> {
        let args: Args = serde_json::from_value(invocation.args).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "vedit_revert: invalid args ({e}). Required: {{ \"refstr\": string }}."
            ))
        })?;
        let refstr = args.refstr.trim();
        if refstr.is_empty() {
            return Err(FunctionCallError::RespondToModel(
                "vedit_revert: empty refstr. Pass a commit hash, short hash, branch name, or HEAD."
                    .into(),
            ));
        }

        let repo = vc::open_or_init(&ctx.project_root).map_err(|e| {
            FunctionCallError::RespondToModel(format!("vedit_revert: opening repo failed: {e}"))
        })?;
        let restored = vc::restore_working_timeline(&repo, refstr)
            .map_err(|e| FunctionCallError::RespondToModel(format!("vedit_revert: {e}")))?;

        let commit_outcome = if args.commit {
            let header = args
                .header
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| {
                    format!("Restore timeline to {}", short_hash(&restored.commit_hash))
                });
            let reasoning = args.reasoning.as_deref().or(Some(
                "Restored project.otio.json to a prior vedit snapshot at the user's request.",
            ));
            Some(
                vc::commit_current_timeline(&repo, &header, reasoning)
                    .map_err(|e| FunctionCallError::RespondToModel(format!("vedit_revert: {e}")))?,
            )
        } else {
            None
        };

        let body = serde_json::json!({
            "restored_ref": restored.requested_ref,
            "restored_commit_hash": restored.commit_hash,
            "restored_timeline_hash": restored.timeline_hash,
            "project_otio": repo.project_otio_path().display().to_string(),
            "audit_commit": commit_outcome.map(|out| serde_json::json!({
                "commit_hash": out.commit_hash,
                "timeline_hash": out.timeline_hash,
                "message": out.message,
            })),
            "next_step": "Use vedit_diff to inspect the restore, or reload the timeline in the UI if needed.",
        });
        Ok(ToolOutput::text(body.to_string()))
    }
}

fn short_hash(hash: &str) -> String {
    hash.strip_prefix("sha256:")
        .unwrap_or(hash)
        .chars()
        .take(7)
        .collect()
}

const DESCRIPTION: &str = "\
Restore the project's working `project.otio.json` to a prior vedit \
commit/ref. This is the safe product-level undo path for edit history: \
it reads the timeline snapshot stored at the requested ref and writes \
that snapshot back to the current project.\
\n\nBy default, `commit=true`, so the restore itself is recorded as a new \
commit and appears in `vedit_log`. Set `commit=false` only when the \
user explicitly asks to inspect or stage a restore without saving it.\
\n\nThis is not a branch checkout and not a merge. It does not switch HEAD \
or create branches; it restores the working timeline file to a known \
snapshot.\
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

    fn write_minimal_otio(project_root: &std::path::Path, name: &str) {
        let v = serde_json::json!({
            "OTIO_SCHEMA": "Timeline.1",
            "name": name,
            "tracks": {
                "OTIO_SCHEMA": "Stack.1",
                "name": "tracks",
                "children": []
            }
        });
        std::fs::write(
            project_root.join("project.otio.json"),
            serde_json::to_vec_pretty(&v).unwrap(),
        )
        .unwrap();
    }

    #[tokio::test]
    async fn restores_and_audit_commits_by_default() {
        let dir = tempfile::tempdir().unwrap();
        write_minimal_otio(dir.path(), "v1");
        let repo = vc::open_or_init(dir.path()).unwrap();
        let first = vc::commit_current_timeline(&repo, "v1", None).unwrap();
        write_minimal_otio(dir.path(), "v2");
        vc::commit_current_timeline(&repo, "v2", None).unwrap();

        let out = VeditRevertTool
            .handle(
                ToolInvocation {
                    call_id: "c1".into(),
                    name: "vedit_revert".into(),
                    args: serde_json::json!({ "refstr": first.commit_hash }),
                },
                ctx_for(dir.path().to_path_buf()),
            )
            .await
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&out.content).unwrap();
        assert!(parsed["audit_commit"]["commit_hash"].as_str().is_some());

        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.path().join("project.otio.json")).unwrap())
                .unwrap();
        assert_eq!(value["name"].as_str(), Some("v1"));
    }
}
