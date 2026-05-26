//! `proxy_media` — generate the .awidat proxy for one raw asset.
//! Ported from `crates/core/src/tools/proxy_media.rs` (the mutating
//! `generate_proxy` tool) to the in-process MCP server. The read-only
//! `proxy_status` companion in the source file is not ported here;
//! `read_media_readiness` already surfaces proxy state.

use std::path::{Component, Path};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::awidat_mcp::context::McpToolCtx;
use crate::proxy::{ProxyStatus, proxy_path_for, proxy_pending_path, proxy_status_for};

/// Arguments to `generate_proxy`.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct GenerateProxyArgs {
    /// Project-relative raw asset path.
    pub asset_id: String,
    /// Regenerate even when the proxy is fresh. Default false.
    #[serde(default)]
    pub force: bool,
}

/// Run `generate_proxy` against the project resolved from
/// [`McpToolCtx`]. Returns a JSON status body as `Ok(String)`.
pub async fn run(args: GenerateProxyArgs, ctx: McpToolCtx) -> Result<String, String> {
    validate_asset_id(&args.asset_id)?;
    let asset_path = ctx.project_root.join(&args.asset_id);
    if !asset_path.is_file() {
        return Err(format!(
            "generate_proxy: asset does not exist: {}",
            args.asset_id
        ));
    }
    let before = proxy_status_for(&ctx.project_root, &args.asset_id, &asset_path);
    if before.status == ProxyStatus::Fresh && !args.force {
        return Ok(serde_json::json!({
            "status": "already_fresh",
            "entry": before
        })
        .to_string());
    }

    let proxy_path = proxy_path_for(&ctx.project_root, &asset_path);
    let pending_path = proxy_pending_path(&proxy_path);
    awidat_render::transcode_proxy(&asset_path, &pending_path, None, CancellationToken::new())
        .await
        .map_err(|e| format!("generate_proxy: {e}"))?;
    if proxy_path.is_file() {
        tokio::fs::remove_file(&proxy_path)
            .await
            .map_err(|e| format!("generate_proxy: remove stale proxy: {e}"))?;
    }
    tokio::fs::rename(&pending_path, &proxy_path)
        .await
        .map_err(|e| format!("generate_proxy: finalize proxy: {e}"))?;

    Ok(serde_json::json!({
        "status": "generated",
        "entry": proxy_status_for(&ctx.project_root, &args.asset_id, &asset_path)
    })
    .to_string())
}

fn validate_asset_id(asset_id: &str) -> Result<(), String> {
    if asset_id.trim().is_empty()
        || Path::new(asset_id).is_absolute()
        || asset_id.contains("://")
        || Path::new(asset_id)
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        return Err("generate_proxy: asset_id must be a safe project-relative path".into());
    }
    Ok(())
}

pub const DESCRIPTION: &str = "\
Generate or refresh the .awidat proxy for one raw asset. \
The proxy is the cached low-bitrate playback file under \
`<project>/.awidat/proxies/`. Pass `asset_id` as the project-relative \
path of a raw asset (for example `raw/take-1.mov`); pass `force: true` \
to regenerate even when an up-to-date proxy already exists. Skips \
ffmpeg when the proxy is already fresh and `force` is false.";
