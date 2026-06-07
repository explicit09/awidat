//! `list_bins` tool — enumerate available asset bins for the project.
//!
//! Slice C2 (wave3-bin-aware). Returns the union of:
//!   - User-defined bins from [`AssetCatalog::bins`] (kind=user).
//!   - Built-in role buckets keyed by [`AssetRole`] (kind=role), with
//!     synthetic ids of the form `role:<snake_case>`. These mirror
//!     Kdenlive's "Audio Clips" / "Video Clips" sidebar buckets so the
//!     agent can filter `list_assets` on role without the user manually
//!     creating bins.
//!
//! Read-only. Safe to call without project mutation.

use async_trait::async_trait;
use montage_proto::professional::{AssetRecord, AssetRole};
use montage_proto::project::Project;
use serde::Deserialize;

use crate::FunctionCallError;
use crate::tool::{ToolContext, ToolHandler, ToolInvocation, ToolOutput};
use crate::tool_schema::Tool as ToolSchema;

/// Synthetic-bin id prefix for built-in role buckets.
pub const ROLE_BIN_PREFIX: &str = "role:";

/// All built-in role buckets, in stable display order.
const ROLE_BUCKETS: &[(AssetRole, &str)] = &[
    (AssetRole::Video, "Video clips"),
    (AssetRole::Audio, "Audio clips"),
    (AssetRole::Still, "Stills"),
    (AssetRole::Graphic, "Graphics"),
    (AssetRole::Caption, "Captions"),
    (AssetRole::Support, "Support media"),
];

/// The `list_bins` tool.
pub struct ListBinsTool;

#[derive(Debug, Deserialize)]
struct ListBinsArgs {}

#[async_trait]
impl ToolHandler for ListBinsTool {
    fn name(&self) -> &'static str {
        "list_bins"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "list_bins".into(),
            description: DESCRIPTION.into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {}
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
        let _args: ListBinsArgs = serde_json::from_value(invocation.args).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "list_bins: invalid args ({e}). No arguments expected."
            ))
        })?;

        // Read the project to get the user-defined bins. If the project
        // doesn't exist yet (e.g. fresh init), gracefully degrade to just
        // the role buckets.
        let project = Project::read(&ctx.project_root).map_err(|e| {
            FunctionCallError::RespondToModel(format!("list_bins: unable to read project: {e}"))
        })?;

        let mut lines: Vec<String> = Vec::new();
        let mut count = 0usize;

        // Role buckets first. They're always present so the agent always
        // has a stable surface to filter against.
        for (role, label) in ROLE_BUCKETS {
            count += 1;
            lines.push(format!(
                "{idx:>3}. kind=role id={id} name=\"{name}\"",
                idx = count,
                id = role_bin_id(*role),
                name = label,
            ));
        }

        // User-defined bins, if any.
        if let Some(catalog) = project
            .timeline
            .metadata
            .montage
            .as_ref()
            .and_then(|meta| meta.asset_catalog.as_ref())
        {
            for bin in &catalog.bins {
                count += 1;
                let parent = bin
                    .parent_id
                    .as_deref()
                    .map(|p| format!(" parent={p}"))
                    .unwrap_or_default();
                lines.push(format!(
                    "{idx:>3}. kind=user id={id} name=\"{name}\"{parent}",
                    idx = count,
                    id = bin.id,
                    name = bin.name,
                ));
            }
        }

        let mut out = format!("total={count}\n");
        for line in lines {
            out.push_str(&line);
            out.push('\n');
        }
        Ok(ToolOutput::text(out))
    }
}

/// Stable synthetic bin id for an [`AssetRole`].
pub fn role_bin_id(role: AssetRole) -> String {
    let suffix = match role {
        AssetRole::Video => "video",
        AssetRole::Audio => "audio",
        AssetRole::Still => "still",
        AssetRole::Graphic => "graphic",
        AssetRole::Caption => "caption",
        AssetRole::Support => "support",
    };
    format!("{ROLE_BIN_PREFIX}{suffix}")
}

/// Resolve the role bin id for an [`AssetRecord`].
pub fn role_id_for_record(record: &AssetRecord) -> String {
    role_bin_id(record.role)
}

const DESCRIPTION: &str = "\
List the available asset bins in this project. Returns built-in role \
buckets (kind=role, ids like 'role:video', 'role:audio') plus any \
user/agent-defined bins (kind=user). Pass any returned id as the \
`bin` argument to `list_assets` to filter that surface.\
";

#[cfg(test)]
mod tests {
    use std::path::Path;

    use montage_proto::professional::{AssetReadiness, AssetRecord, AssetRole};
    use montage_proto::project::Project;
    use tokio::sync::broadcast;

    use super::*;
    use crate::media_catalog_mutation::{create_bin, ensure_montage_metadata, upsert_asset};
    use crate::tool::SandboxMode;

    fn ctx_at(root: &Path) -> ToolContext {
        let (tx, _) = broadcast::channel(8);
        ToolContext {
            project_root: root.to_path_buf(),
            events_tx: tx,
            user_input_tx: None,
            job_manager: montage_render::JobManager::new(),
            approval_tx: None,
            sandbox_mode: SandboxMode::Default,
            mcp_host: crate::mcp_host::McpHost::new(montage_mcp::ClientInfo {
                name: "test".into(),
                version: "0.0.0".into(),
            }),
            skills: std::sync::Arc::new(crate::skills::SkillRegistry::default()),
            subagent_return: None,
        }
    }

    fn invoke() -> ToolInvocation {
        ToolInvocation {
            call_id: "c1".into(),
            name: "list_bins".into(),
            args: serde_json::json!({}),
        }
    }

    #[tokio::test]
    async fn lists_only_role_buckets_on_empty_project() {
        let dir = tempfile::tempdir().unwrap();
        Project::init(dir.path()).unwrap();
        let out = ListBinsTool
            .handle(invoke(), ctx_at(dir.path()))
            .await
            .unwrap();
        assert!(out.content.contains("total=6"));
        assert!(out.content.contains("role:video"));
        assert!(out.content.contains("role:audio"));
        assert!(out.content.contains("role:still"));
        assert!(out.content.contains("role:graphic"));
        assert!(out.content.contains("role:caption"));
        assert!(out.content.contains("role:support"));
        assert!(!out.content.contains("kind=user"));
    }

    #[tokio::test]
    async fn lists_user_bins_after_role_buckets() {
        let dir = tempfile::tempdir().unwrap();
        let mut project = Project::init(dir.path()).unwrap();
        let meta = ensure_montage_metadata(&mut project.timeline);
        create_bin(meta, "scene-1".into(), "Scene 1".into(), None).unwrap();
        create_bin(meta, "broll".into(), "B-roll".into(), None).unwrap();
        project.write(dir.path()).unwrap();

        let out = ListBinsTool
            .handle(invoke(), ctx_at(dir.path()))
            .await
            .unwrap();
        assert!(out.content.contains("total=8")); // 6 role + 2 user
        assert!(out.content.contains("kind=user id=scene-1"));
        assert!(out.content.contains("kind=user id=broll"));
    }

    #[tokio::test]
    async fn role_id_for_record_matches_role() {
        let record = AssetRecord {
            id: "raw/a.wav".into(),
            path: "raw/a.wav".into(),
            role: AssetRole::Audio,
            readiness: AssetReadiness::default(),
            ..AssetRecord::default()
        };
        assert_eq!(role_id_for_record(&record), "role:audio");
        // Sanity-check upsert is just to keep this test independent of
        // any external setup.
        let mut timeline = montage_proto::otio::Timeline::empty("t");
        let meta = ensure_montage_metadata(&mut timeline);
        upsert_asset(meta, record);
    }
}
