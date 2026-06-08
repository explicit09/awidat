//! Help and support commands used by the native Help menu.

use tauri::{AppHandle, Manager};

/// Reveal Montage's application log directory in the platform file manager.
#[tauri::command]
pub fn reveal_app_log_dir(app: AppHandle) -> Result<(), String> {
    let log_dir = app.path().app_log_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&log_dir).map_err(|e| {
        format!(
            "failed to create app log directory {}: {e}",
            log_dir.display()
        )
    })?;
    tauri_plugin_opener::reveal_item_in_dir(log_dir).map_err(|e| e.to_string())
}
