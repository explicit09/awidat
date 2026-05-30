//! Restore the project's OTIO timeline from an inline JSON snapshot
//! captured at an earlier accept decision (History tab → ↺ Restore).
//!
//! Why this lives in its own module:
//!   The Brief stack records a HistoryEntry on each accept; the
//!   inline `timelineSnapshot` field holds the raw OTIO at that
//!   moment. Restoring is a single atomic op: parse → validate →
//!   write → emit. None of the existing timeline commands had the
//!   "blind write a known-good Timeline" shape, so a dedicated entry
//!   point keeps the round-trip honest.
//!
//! Read side:
//!   `read_timeline_otio_raw` returns the raw `project.otio.json`
//!   bytes so the frontend can stash them inside a HistoryEntry at
//!   accept time. We could derive this from `read_timeline`'s
//!   flattened view but the flattening drops effect metadata, awidat
//!   metadata, and original schema fields that the restore path
//!   needs to round-trip. Reading the file directly is simpler and
//!   honest.
//!
//! Write side:
//!   `restore_timeline_otio` accepts the JSON string, deserializes it
//!   to a `Timeline` (the validator rejects malformed shapes), and
//!   writes it back through `Project::write` so every managed file
//!   stays consistent. A vedit audit-commit captures the restore
//!   itself, so the version-control history records "Restored
//!   timeline to <history-entry timestamp>" — same shape as the
//!   existing vedit-ref restore.

use awidat_proto::otio::Timeline;
use awidat_proto::project::Project;
use tauri::{AppHandle, State};

use crate::events::emit_timeline_changed;
use crate::state::AwidatState;

/// Read the active project's `project.otio.json` and return the raw
/// JSON bytes as a string.
///
/// Called by the Brief stack right before an accept dispatch lands so
/// the resulting HistoryEntry can stash an inline timeline snapshot.
/// Returns an empty-snapshot OTIO string when no project is loaded so
/// the caller can skip the capture without an error path.
#[tauri::command]
pub async fn read_timeline_otio_raw(state: State<'_, AwidatState>) -> Result<String, String> {
    let project_root = match state.project_root.lock().await.clone() {
        Some(p) => p,
        None => return Ok(String::new()),
    };
    let raw = tokio::task::spawn_blocking(move || -> Result<String, String> {
        let path = project_root.join("project.otio.json");
        if !path.is_file() {
            return Ok(String::new());
        }
        std::fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))
    })
    .await
    .map_err(|e| format!("join: {e}"))??;
    Ok(raw)
}

/// Restore the project's timeline from an inline JSON snapshot.
///
/// The snapshot must be a valid OTIO `Timeline.1` JSON string. We
/// validate by deserializing into [`Timeline`] (serde rejects
/// malformed shapes); the write goes through [`Project::write`] so
/// the edit-plan + manifest stay in sync with the OTIO file.
///
/// On success emits `awidat://timeline-changed` so the frontend
/// re-reads the timeline and the canvas reflects the restored state.
#[tauri::command]
pub async fn restore_timeline_otio(
    app: AppHandle,
    state: State<'_, AwidatState>,
    snapshot_json: String,
) -> Result<(), String> {
    let trimmed = snapshot_json.trim();
    if trimmed.is_empty() {
        return Err("snapshot_json is empty".into());
    }
    let project_root = state
        .project_root
        .lock()
        .await
        .clone()
        .ok_or_else(|| "no project loaded".to_string())?;

    let snapshot_owned = snapshot_json.clone();
    let project_root_buf = project_root.clone();
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        // Parse first so a malformed snapshot is rejected before we
        // touch disk. serde's error message carries line/column, which
        // is more useful than "write failed" when the caller hands us
        // a corrupted blob.
        let restored_timeline: Timeline =
            serde_json::from_str(&snapshot_owned).map_err(|e| format!("parse snapshot: {e}"))?;

        // Read the project so we preserve the edit_plan + manifest;
        // we only swap the timeline. If the project no longer exists
        // on disk (deleted while History tab was open) the read fails
        // here with a clear "project read" error.
        let mut project =
            Project::read(&project_root_buf).map_err(|e| format!("project read: {e}"))?;
        project.timeline = restored_timeline;
        project
            .write(&project_root_buf)
            .map_err(|e| format!("project write: {e}"))?;

        // Audit-commit the restore so vedit history records the
        // rollback. Best-effort: mirrors the policy in clip_params and
        // proposal — vedit failures log a warning but don't unwind the
        // user-visible disk write.
        let seat_author = crate::commands::vedit::desktop_commit_author();
        match awidat_core::vc::open_or_init(&project_root_buf) {
            Ok(repo) => {
                if let Err(e) = awidat_core::vc::commit_current_timeline_as(
                    &repo,
                    "Restore timeline from History snapshot",
                    Some(
                        "Restored project.otio.json from an inline History tab snapshot. \
                         See the desktop History surface for the originating decision.",
                    ),
                    seat_author,
                ) {
                    tracing::warn!(
                        error = %e,
                        "vedit audit-commit failed (timeline restore path)"
                    );
                }
            }
            Err(e) => {
                tracing::debug!(
                    error = %e,
                    "vedit repo unavailable; skipping audit-commit"
                );
            }
        }

        Ok(())
    })
    .await
    .map_err(|e| format!("spawn_blocking join: {e}"))??;

    emit_timeline_changed(&app, &project_root);
    Ok(())
}

#[cfg(test)]
mod tests {
    use awidat_proto::otio::Timeline;

    #[test]
    fn snapshot_round_trips_through_serde() {
        let timeline = Timeline::empty("restore-test");
        let json = serde_json::to_string(&timeline).expect("serialize timeline");
        let restored: Timeline = serde_json::from_str(&json).expect("deserialize timeline");
        assert_eq!(restored.name, "restore-test");
    }

    #[test]
    fn malformed_snapshot_fails_parse() {
        let bad = "{ not valid json";
        let err = serde_json::from_str::<Timeline>(bad).unwrap_err();
        // Any non-empty parse error message is fine — we just want to
        // prove the deserialize path rejects malformed input before
        // the restore command touches disk.
        assert!(!err.to_string().is_empty());
    }
}
