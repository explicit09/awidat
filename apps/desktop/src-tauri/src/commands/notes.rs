//! Tauri commands for the per-project editorial notes file.
//! Phase 1.7 wires the NotesPanel UI to the persistent storage in
//! `<project>/.awidat/notes.json` (shape defined in
//! `awidat_core::notes::NotesFile`).
//!
//! Dismiss is special — the command also stamps the matching coarse
//! pattern into `dismissed_patterns.json` so the agent's editorial-
//! finding tools (find_dead_air etc.) respect the dismissal on next
//! scan. That coupling lives here at the desktop layer rather than
//! in core; core just owns the load/save shape.

use awidat_core::dismissal::{
    DismissalBucket, DismissalFile, dismissal_file_path, load_dismissals, save_dismissals,
};
use awidat_core::notes::{NotesFile, PersistedNote, load_notes, save_notes};
use serde::Deserialize;
use tauri::State;

use crate::state::AwidatState;

/// Read the current notes file. Returns empty when no project is
/// loaded — same shape so the frontend doesn't have to special-case
/// the unloaded state.
#[tauri::command]
pub async fn list_notes(state: State<'_, AwidatState>) -> Result<NotesFile, String> {
    let project_root = match state.project_root.lock().await.clone() {
        Some(p) => p,
        None => return Ok(NotesFile::empty()),
    };
    Ok(load_notes(&project_root))
}

/// Append-or-replace a single note. Idempotent: same id → in-place
/// update so multiple agent emissions of the same note don't grow
/// duplicates. Returns the updated file.
#[tauri::command]
pub async fn upsert_note(
    state: State<'_, AwidatState>,
    note: PersistedNote,
) -> Result<NotesFile, String> {
    let project_root = state
        .project_root
        .lock()
        .await
        .clone()
        .ok_or_else(|| "no project loaded".to_string())?;
    let mut file = load_notes(&project_root);
    file.upsert(note);
    save_notes(&project_root, &file)?;
    Ok(file)
}

/// Mutate a note's status (resolved / dismissed). When the new
/// status is `dismissed` AND a dismissal bucket is supplied, also
/// stamp the bucket into `dismissed_patterns.json` so the agent
/// won't re-surface the same kind on next scan. The bucket comes
/// from the frontend because only the UI knows which threshold
/// (e.g. "silence under 2s") to dismiss; the note record alone
/// doesn't carry the bucket.
///
/// Returns `(updated notes, updated dismissals)` so the UI can
/// reflect both writes without round-tripping a separate
/// `list_dismissals` call.
#[tauri::command]
pub async fn set_note_status(
    state: State<'_, AwidatState>,
    args: SetNoteStatusArgs,
) -> Result<(NotesFile, DismissalFile), String> {
    let project_root = state
        .project_root
        .lock()
        .await
        .clone()
        .ok_or_else(|| "no project loaded".to_string())?;

    let mut notes = load_notes(&project_root);
    if !notes.set_status(&args.id, &args.status) {
        return Err(format!("note id {:?} not found", args.id));
    }
    save_notes(&project_root, &notes)?;

    let mut dismissals = load_dismissals(&project_root);
    if args.status == "dismissed"
        && let Some(bucket) = args.dismiss_bucket
    {
        dismissals.add(bucket);
        save_dismissals(&project_root, &dismissals)?;
    }
    Ok((notes, dismissals))
}

#[derive(Debug, Deserialize)]
pub struct SetNoteStatusArgs {
    /// Note id to mutate.
    pub id: String,
    /// New status: `"open"`, `"resolved"`, or `"dismissed"`.
    pub status: String,
    /// Optional bucket to dismiss alongside (only honored when
    /// `status == "dismissed"`). When `None`, only the individual
    /// note flips status; the agent will still re-surface the same
    /// kind on next scan.
    #[serde(default)]
    pub dismiss_bucket: Option<DismissalBucket>,
}

/// Drop a note record entirely. Used by the panel's "clear all
/// resolved" affordance and by edge cases where a stale note
/// should disappear (e.g. the agent emitted a note pointing at a
/// clip the user deleted; the desktop garbage-collects it). v1
/// has no UI for this beyond the Tauri command — wired for future
/// use.
#[tauri::command]
pub async fn delete_note(
    state: State<'_, AwidatState>,
    id: String,
) -> Result<NotesFile, String> {
    let project_root = state
        .project_root
        .lock()
        .await
        .clone()
        .ok_or_else(|| "no project loaded".to_string())?;
    let mut file = load_notes(&project_root);
    file.notes.retain(|n| n.id != id);
    save_notes(&project_root, &file)?;
    Ok(file)
}

/// Defensive util: surface where the dismissals file lives. Used
/// by the desktop's "show in finder" affordance (not wired to UI
/// in Phase 1, but cheap to expose now).
#[tauri::command]
pub async fn dismissals_path(
    state: State<'_, AwidatState>,
) -> Result<Option<String>, String> {
    Ok(state
        .project_root
        .lock()
        .await
        .as_ref()
        .map(|p| dismissal_file_path(p).to_string_lossy().into_owned()))
}
