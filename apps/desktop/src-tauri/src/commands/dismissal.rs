//! Tauri commands for the per-project dismissal-pattern memory
//! (Phase 1.6). The NotesPanel calls these when the user clicks
//! "Dismiss" on an EditorialNote, and on first paint to load the
//! current dismissal state for client-side filtering.
//!
//! Storage lives at `<project>/.awidat/dismissed_patterns.json`,
//! shape defined in `awidat_core::dismissal::DismissalFile`.

use awidat_core::dismissal::{
    DismissalBucket, DismissalFile, load_dismissals, save_dismissals,
};
use tauri::State;

use crate::state::AwidatState;

/// Read the current dismissal file for the loaded project. Returns
/// an empty file when no project is loaded — same shape so the
/// frontend doesn't have to special-case.
#[tauri::command]
pub async fn list_dismissals(state: State<'_, AwidatState>) -> Result<DismissalFile, String> {
    let project_root = match state.project_root.lock().await.clone() {
        Some(p) => p,
        None => return Ok(DismissalFile::empty()),
    };
    Ok(load_dismissals(&project_root))
}

/// Append a new dismissal record. Idempotent — duplicates are
/// silently dropped. Returns the updated file so the frontend can
/// reflect the change without a separate `list_dismissals` call.
#[tauri::command]
pub async fn dismiss_pattern(
    state: State<'_, AwidatState>,
    bucket: DismissalBucket,
) -> Result<DismissalFile, String> {
    let project_root = state
        .project_root
        .lock()
        .await
        .clone()
        .ok_or_else(|| "no project loaded".to_string())?;
    let mut file = load_dismissals(&project_root);
    file.add(bucket);
    save_dismissals(&project_root, &file)?;
    Ok(file)
}

/// Undo a previous dismissal. No-op when the bucket isn't on file.
/// Returns the updated file.
#[tauri::command]
pub async fn undismiss_pattern(
    state: State<'_, AwidatState>,
    bucket: DismissalBucket,
) -> Result<DismissalFile, String> {
    let project_root = state
        .project_root
        .lock()
        .await
        .clone()
        .ok_or_else(|| "no project loaded".to_string())?;
    let mut file = load_dismissals(&project_root);
    file.remove(bucket);
    save_dismissals(&project_root, &file)?;
    Ok(file)
}
