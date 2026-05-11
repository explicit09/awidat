//! Desktop-facing vedit history commands.

use serde::Serialize;
use tauri::{AppHandle, State};

use crate::events::emit_timeline_changed;
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

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VeditDiffResponse {
    pub from_ref: String,
    pub to_ref: String,
    pub change_count: usize,
    pub changes: serde_json::Value,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VeditRestoreResponse {
    pub restored_ref: String,
    pub restored_commit_hash: String,
    pub restored_timeline_hash: String,
    pub audit_commit_hash: Option<String>,
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

#[tauri::command]
pub async fn diff_vedit_refs(
    state: State<'_, AwidatState>,
    from_ref: Option<String>,
    to_ref: Option<String>,
) -> Result<VeditDiffResponse, String> {
    let project_root = state
        .project_root
        .lock()
        .await
        .clone()
        .ok_or_else(|| "no project loaded".to_string())?;
    let repo = awidat_core::vc::open_or_init(&project_root)
        .map_err(|e| format!("open vedit repo: {e}"))?;
    let diff = awidat_core::vc::diff_refs(&repo, from_ref.as_deref(), to_ref.as_deref())
        .map_err(|e| format!("read vedit diff: {e}"))?;
    let change_count = diff.len();
    let changes =
        serde_json::to_value(&diff.changes).map_err(|e| format!("serialize vedit diff: {e}"))?;
    Ok(VeditDiffResponse {
        from_ref: diff.from_ref,
        to_ref: diff.to_ref,
        change_count,
        changes,
    })
}

#[tauri::command]
pub async fn restore_vedit_ref(
    app: AppHandle,
    state: State<'_, AwidatState>,
    refstr: String,
) -> Result<VeditRestoreResponse, String> {
    let refstr = refstr.trim().to_string();
    if refstr.is_empty() {
        return Err("restore ref cannot be empty".into());
    }
    let project_root = state
        .project_root
        .lock()
        .await
        .clone()
        .ok_or_else(|| "no project loaded".to_string())?;
    let repo = awidat_core::vc::open_or_init(&project_root)
        .map_err(|e| format!("open vedit repo: {e}"))?;
    let restored = awidat_core::vc::restore_working_timeline(&repo, &refstr)
        .map_err(|e| format!("restore vedit ref: {e}"))?;
    let header = format!("Restore timeline to {}", short_hash(&restored.commit_hash));
    let audit = awidat_core::vc::commit_current_timeline(
        &repo,
        &header,
        Some("Restored project.otio.json from the desktop timeline history panel."),
    )
    .map_err(|e| format!("commit restore audit: {e}"))?;
    emit_timeline_changed(&app, &project_root);
    Ok(VeditRestoreResponse {
        restored_ref: restored.requested_ref,
        restored_commit_hash: restored.commit_hash,
        restored_timeline_hash: restored.timeline_hash,
        audit_commit_hash: Some(audit.commit_hash),
    })
}

fn short_hash(hash: &str) -> String {
    hash.strip_prefix("sha256:")
        .unwrap_or(hash)
        .chars()
        .take(7)
        .collect()
}
