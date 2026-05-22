//! Desktop-facing configuration inspection commands.

use std::collections::HashSet;
use std::path::PathBuf;

use awidat_config::{Config, McpServerKind};
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::state::AwidatState;

/// Read the merged indexer configuration the desktop will use for
/// `index_project`, plus the global/project paths where users can override it.
#[tauri::command]
pub async fn read_indexer_config(
    state: State<'_, AwidatState>,
) -> Result<IndexerConfigSnapshot, String> {
    let project_root = state.project_root.lock().await.clone();
    indexer_config_snapshot(project_root).await
}

/// Enable or disable one indexer in the current project's config overlay.
///
/// This intentionally writes project-local config only. Global config remains a
/// user-level concern; desktop toggles should not change every project by accident.
#[tauri::command]
pub async fn set_project_indexer_enabled(
    state: State<'_, AwidatState>,
    args: SetProjectIndexerEnabledArgs,
) -> Result<IndexerConfigSnapshot, String> {
    let project_root = state
        .project_root
        .lock()
        .await
        .clone()
        .ok_or_else(|| "open a project before changing indexer config".to_string())?;

    let merged = Config::load(Some(&project_root)).map_err(|e| format!("load config: {e}"))?;
    let mut server = merged
        .mcp
        .servers
        .iter()
        .find(|server| server.kind == McpServerKind::Indexer && server.name == args.name)
        .cloned()
        .ok_or_else(|| format!("unknown indexer '{}'", args.name))?;
    server.enabled = args.enabled;

    let project_path = awidat_config::project_config_path(&project_root);
    let mut project_config = if project_path.exists() {
        Config::load_file(&project_path).map_err(|e| format!("load project config: {e}"))?
    } else {
        Config::default()
    };

    if let Some(existing) = project_config
        .mcp
        .servers
        .iter_mut()
        .find(|existing| existing.name == server.name)
    {
        *existing = server;
    } else {
        project_config.mcp.servers.push(server);
    }

    if let Some(parent) = project_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("create config dir: {e}"))?;
    }
    let text =
        toml::to_string_pretty(&project_config).map_err(|e| format!("serialize config: {e}"))?;
    tokio::fs::write(&project_path, text)
        .await
        .map_err(|e| format!("write project config: {e}"))?;

    indexer_config_snapshot(Some(project_root)).await
}

async fn indexer_config_snapshot(
    project_root: Option<PathBuf>,
) -> Result<IndexerConfigSnapshot, String> {
    let merged = Config::load(project_root.as_deref()).map_err(|e| format!("load config: {e}"))?;
    let user_supplied = Config::load_without_defaults(project_root.as_deref()).unwrap_or_default();
    let user_names: HashSet<&str> = user_supplied
        .mcp
        .servers
        .iter()
        .map(|server| server.name.as_str())
        .collect();

    let global_path = awidat_config::global_config_path()
        .map_err(|e| format!("global config path: {e}"))?
        .map(|path| path.display().to_string());
    let project_path = project_root
        .as_deref()
        .map(awidat_config::project_config_path)
        .map(|path| path.display().to_string());

    let indexers = merged
        .mcp
        .servers
        .iter()
        .filter(|server| server.kind == McpServerKind::Indexer)
        .map(|server| IndexerConfigEntry {
            name: server.name.clone(),
            enabled: server.enabled,
            command: server.command.clone(),
            args: server.args.clone(),
            cwd: server.cwd.as_ref().map(|path| path.display().to_string()),
            depends_on: server.depends_on.clone(),
            resource_class: format!("{:?}", server.resource_class),
            group: server.indexer_group.map(|group| format!("{group:?}")),
            user_configured: user_names.contains(server.name.as_str()),
        })
        .collect();

    Ok(IndexerConfigSnapshot {
        global_path,
        project_path,
        indexers,
    })
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetProjectIndexerEnabledArgs {
    pub name: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexerConfigSnapshot {
    pub global_path: Option<String>,
    pub project_path: Option<String>,
    pub indexers: Vec<IndexerConfigEntry>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexerConfigEntry {
    pub name: String,
    pub enabled: bool,
    pub command: String,
    pub args: Vec<String>,
    pub cwd: Option<String>,
    pub depends_on: Vec<String>,
    pub resource_class: String,
    pub group: Option<String>,
    pub user_configured: bool,
}
