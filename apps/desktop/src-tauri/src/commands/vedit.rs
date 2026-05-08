//! Desktop-facing vedit history commands.

use serde::Serialize;
use tauri::State;

use crate::state::AwidatState;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VeditCommitEntry {
    /// Commit hash.
    pub commit_hash: String,
    /// Timeline content hash.
    pub timeline_hash: String,
    /// ISO timestamp.
    pub timestamp: String,
    /// First line of the commit message.
    pub header: String,
    /// Full commit message.
    pub full_message: String,
    /// Parent commit hashes.
    pub parents: Vec<String>,
}

#[tauri::command]
pub async fn list_vedit_commits(
    state: State<'_, AwidatState>,
    limit: Option<usize>,
) -> Result<Vec<VeditCommitEntry>, String> {
    let project_root = state
        .project_root
        .lock()
        .await
        .clone()
        .ok_or_else(|| "no project loaded".to_string())?;
    let repo = awidat_core::vc::open_or_init(&project_root)
        .map_err(|e| format!("open vedit repo: {e}"))?;
    let entries = awidat_core::vc::log(&repo, limit.unwrap_or(30).min(200))
        .map_err(|e| format!("read vedit log: {e}"))?;
    Ok(entries
        .into_iter()
        .map(|entry| VeditCommitEntry {
            commit_hash: entry.commit_hash,
            timeline_hash: entry.timeline_hash,
            timestamp: entry.timestamp,
            header: entry.header,
            full_message: entry.full_message,
            parents: entry.parents,
        })
        .collect())
}
