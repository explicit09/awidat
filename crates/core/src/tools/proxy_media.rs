//! Agent-callable proxy status and generation tools.

use std::path::{Component, Path};

use async_trait::async_trait;
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

use crate::FunctionCallError;
use crate::proxy::{
    ProxyStatus, proxy_path_for, proxy_pending_path, proxy_status_for, proxy_status_for_project,
};
use crate::tool::{ApprovalKey, ToolContext, ToolHandler, ToolInvocation, ToolOutput};
use crate::tool_schema::Tool as ToolSchema;

/// Read proxy status for one or all raw assets.
pub struct ProxyStatusTool;

/// Generate a proxy for one raw asset.
pub struct GenerateProxyTool;

#[derive(Debug, Deserialize)]
struct ProxyStatusArgs {
    #[serde(default)]
    asset_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GenerateProxyArgs {
    asset_id: String,
    #[serde(default)]
    force: bool,
}

#[async_trait]
impl ToolHandler for ProxyStatusTool {
    fn name(&self) -> &'static str {
        "proxy_status"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "proxy_status".into(),
            description: "Report proxy cache status for one raw asset or all raw project media."
                .into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "asset_id": {
                        "type": "string",
                        "description": "Optional project-relative raw asset path. Omit to report all raw assets."
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
        let args: ProxyStatusArgs = serde_json::from_value(invocation.args).map_err(|e| {
            FunctionCallError::RespondToModel(format!("proxy_status: invalid args ({e})"))
        })?;
        let body = if let Some(asset_id) = args.asset_id {
            validate_asset_id(&asset_id)?;
            let asset_path = ctx.project_root.join(&asset_id);
            if !asset_path.is_file() {
                return Err(FunctionCallError::RespondToModel(format!(
                    "proxy_status: asset does not exist: {asset_id}"
                )));
            }
            serde_json::json!({
                "entries": [proxy_status_for(&ctx.project_root, &asset_id, &asset_path)]
            })
        } else {
            let entries = proxy_status_for_project(&ctx.project_root).map_err(|e| {
                FunctionCallError::RespondToModel(format!("proxy_status: unable to scan raw/: {e}"))
            })?;
            serde_json::json!({ "entries": entries })
        };
        Ok(ToolOutput::text(body.to_string()))
    }
}

#[async_trait]
impl ToolHandler for GenerateProxyTool {
    fn name(&self) -> &'static str {
        "generate_proxy"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "generate_proxy".into(),
            description: "Generate or refresh the .awidat proxy for one raw asset.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["asset_id"],
                "properties": {
                    "asset_id": {
                        "type": "string",
                        "description": "Project-relative raw asset path."
                    },
                    "force": {
                        "type": "boolean",
                        "description": "Regenerate even when the proxy is fresh. Default false."
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
        vec![ApprovalKey::new(
            self.name(),
            format!(
                "asset:{}",
                invocation.args["asset_id"].as_str().unwrap_or("")
            ),
        )]
    }

    async fn handle(
        &self,
        invocation: ToolInvocation,
        ctx: ToolContext,
    ) -> Result<ToolOutput, FunctionCallError> {
        let args: GenerateProxyArgs = serde_json::from_value(invocation.args).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "generate_proxy: invalid args ({e}). Expected asset_id and optional force."
            ))
        })?;
        validate_asset_id(&args.asset_id)?;
        let asset_path = ctx.project_root.join(&args.asset_id);
        if !asset_path.is_file() {
            return Err(FunctionCallError::RespondToModel(format!(
                "generate_proxy: asset does not exist: {}",
                args.asset_id
            )));
        }
        let before = proxy_status_for(&ctx.project_root, &args.asset_id, &asset_path);
        if before.status == ProxyStatus::Fresh && !args.force {
            return Ok(ToolOutput::text(
                serde_json::json!({
                    "status": "already_fresh",
                    "entry": before
                })
                .to_string(),
            ));
        }

        let proxy_path = proxy_path_for(&ctx.project_root, &asset_path);
        let pending_path = proxy_pending_path(&proxy_path);
        awidat_render::transcode_proxy(&asset_path, &pending_path, None, CancellationToken::new())
            .await
            .map_err(|e| FunctionCallError::RespondToModel(format!("generate_proxy: {e}")))?;
        if proxy_path.is_file() {
            tokio::fs::remove_file(&proxy_path).await.map_err(|e| {
                FunctionCallError::Fatal(format!("generate_proxy: remove stale proxy: {e}"))
            })?;
        }
        tokio::fs::rename(&pending_path, &proxy_path)
            .await
            .map_err(|e| {
                FunctionCallError::Fatal(format!("generate_proxy: finalize proxy: {e}"))
            })?;

        Ok(ToolOutput::text(
            serde_json::json!({
                "status": "generated",
                "entry": proxy_status_for(&ctx.project_root, &args.asset_id, &asset_path)
            })
            .to_string(),
        ))
    }
}

fn validate_asset_id(asset_id: &str) -> Result<(), FunctionCallError> {
    if asset_id.trim().is_empty()
        || Path::new(asset_id).is_absolute()
        || asset_id.contains("://")
        || Path::new(asset_id)
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(FunctionCallError::RespondToModel(
            "proxy tool: asset_id must be a safe project-relative path".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use tokio::sync::broadcast;

    use super::*;

    fn ctx_at(root: &std::path::Path) -> ToolContext {
        let (tx, _) = broadcast::channel(8);
        ToolContext {
            project_root: root.to_path_buf(),
            events_tx: tx,
            user_input_tx: None,
            job_manager: awidat_render::JobManager::new(),
            approval_tx: None,
            sandbox_mode: crate::tool::SandboxMode::Default,
            mcp_host: crate::mcp_host::McpHost::new(awidat_mcp::ClientInfo {
                name: "test".into(),
                version: "0.0.0".into(),
            }),
            skills: std::sync::Arc::new(crate::skills::SkillRegistry::default()),
            subagent_return: None,
        }
    }

    fn invocation(name: &str, args: serde_json::Value) -> ToolInvocation {
        ToolInvocation {
            call_id: "c1".into(),
            name: name.into(),
            args,
        }
    }

    fn write(path: &std::path::Path, body: &[u8]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    #[tokio::test]
    async fn proxy_status_reports_one_asset() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("raw/a.mov"), b"media");

        let output = ProxyStatusTool
            .handle(
                invocation("proxy_status", serde_json::json!({"asset_id": "raw/a.mov"})),
                ctx_at(dir.path()),
            )
            .await
            .unwrap();

        let body: serde_json::Value = serde_json::from_str(&output.content).unwrap();
        assert_eq!(body["entries"][0]["asset_id"], "raw/a.mov");
        assert_eq!(body["entries"][0]["status"], "missing");
    }

    #[tokio::test]
    async fn generate_proxy_skips_fresh_proxy_without_invoking_ffmpeg() {
        let dir = tempfile::tempdir().unwrap();
        let asset = dir.path().join("raw/a.mov");
        write(&asset, b"media");
        std::thread::sleep(std::time::Duration::from_millis(20));
        let proxy_path = proxy_path_for(dir.path(), &asset);
        write(&proxy_path, b"proxy");

        let output = GenerateProxyTool
            .handle(
                invocation(
                    "generate_proxy",
                    serde_json::json!({"asset_id": "raw/a.mov"}),
                ),
                ctx_at(dir.path()),
            )
            .await
            .unwrap();

        let body: serde_json::Value = serde_json::from_str(&output.content).unwrap();
        assert_eq!(body["status"], "already_fresh");
        assert_eq!(body["entry"]["status"], "fresh");
    }

    #[tokio::test]
    async fn proxy_tools_reject_unsafe_asset_id() {
        let dir = tempfile::tempdir().unwrap();

        let err = ProxyStatusTool
            .handle(
                invocation("proxy_status", serde_json::json!({"asset_id": "../a.mov"})),
                ctx_at(dir.path()),
            )
            .await
            .unwrap_err();

        match err {
            FunctionCallError::RespondToModel(message) => {
                assert!(
                    message.contains("safe project-relative path"),
                    "got: {message}"
                );
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }
}
