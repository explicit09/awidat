//! Per-project agent permission mode (Phase 1.8).
//!
//! Storage: `<project>/.awidat/permission_mode` — a single line of
//! text, one of `"manual"` / `"copilot"` / `"autopilot"`. The file
//! mirrors the protocol's `PermissionMode` enum (snake_case
//! serialization).
//!
//! The agent session-build path reads this file for prompt behavior,
//! and the approval bridge reads it at each approval request so runtime
//! gating matches the dropdown.

use std::path::Path;

use awidat_desktop_protocol::PermissionMode;
use tauri::State;

use crate::state::AwidatState;

/// Filename inside `.awidat/`.
const PERMISSION_FILE: &str = "permission_mode";

/// Read the project's permission mode. Defaults to `Manual` when
/// no project is loaded OR when the file is missing — matches the
/// pre-1.8 behavior (every proposal needs explicit Accept) so
/// users with old projects don't get upgraded into autopilot
/// without choosing it.
#[tauri::command]
pub async fn get_permission_mode(state: State<'_, AwidatState>) -> Result<PermissionMode, String> {
    let project_root = match state.project_root.lock().await.clone() {
        Some(p) => p,
        None => return Ok(PermissionMode::Manual),
    };
    Ok(read_mode(&project_root))
}

/// Persist a new permission mode for the loaded project.
#[tauri::command]
pub async fn set_permission_mode(
    state: State<'_, AwidatState>,
    mode: PermissionMode,
) -> Result<(), String> {
    let project_root = state
        .project_root
        .lock()
        .await
        .clone()
        .ok_or_else(|| "no project loaded".to_string())?;
    write_mode(&project_root, mode)?;

    // The system prompt includes the permission mode. Drop the cached
    // session so the next turn rebuilds with the newly selected mode.
    *state.session.lock().await = None;
    Ok(())
}

/// Pure helper: parse the string in the file → `PermissionMode`.
/// Falls back to Manual on missing or unparseable input.
fn read_mode(project_root: &Path) -> PermissionMode {
    let path = project_root.join(".awidat").join(PERMISSION_FILE);
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(_) => return PermissionMode::Manual,
    };
    match text.trim() {
        "manual" => PermissionMode::Manual,
        "copilot" => PermissionMode::Copilot,
        "autopilot" => PermissionMode::Autopilot,
        other => {
            tracing::warn!(
                value = %other,
                path = %path.display(),
                "unknown permission_mode value — falling back to manual",
            );
            PermissionMode::Manual
        }
    }
}

/// Pure helper: serialize `PermissionMode` → file. Creates the
/// `.awidat/` dir if missing.
fn write_mode(project_root: &Path, mode: PermissionMode) -> Result<(), String> {
    let path = project_root.join(".awidat").join(PERMISSION_FILE);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create .awidat: {e}"))?;
    }
    let text = match mode {
        PermissionMode::Manual => "manual",
        PermissionMode::Copilot => "copilot",
        PermissionMode::Autopilot => "autopilot",
    };
    std::fs::write(&path, text).map_err(|e| format!("write permission_mode: {e}"))?;
    Ok(())
}

/// Public read helper for the agent's system-prompt builder
/// (Phase 1.9). Wrapped pub for cross-module access.
pub fn read_mode_pub(project_root: &Path) -> PermissionMode {
    read_mode(project_root)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_manual_when_file_missing() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read_mode(dir.path()), PermissionMode::Manual);
    }

    #[test]
    fn write_then_read_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        write_mode(dir.path(), PermissionMode::Copilot).unwrap();
        assert_eq!(read_mode(dir.path()), PermissionMode::Copilot);
        write_mode(dir.path(), PermissionMode::Autopilot).unwrap();
        assert_eq!(read_mode(dir.path()), PermissionMode::Autopilot);
    }

    #[test]
    fn unknown_value_falls_back_to_manual() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".awidat").join("permission_mode");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"bogus").unwrap();
        assert_eq!(read_mode(dir.path()), PermissionMode::Manual);
    }
}
