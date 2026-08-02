//! Local project inspection used by publishing UI.

use tauri::State;

use crate::publishing::{AiDisclosure, disclosure_for_project_root};
use crate::state::MontageState;

/// Inspect the loaded timeline for generated media so the desktop can warn
/// before publishing. No project loaded means no detected synthetic content.
#[tauri::command]
pub async fn compute_ai_disclosure(state: State<'_, MontageState>) -> Result<AiDisclosure, String> {
    let project_root = state.project_root.lock().await.clone();
    Ok(match project_root {
        Some(root) => disclosure_for_project_root(&root),
        None => AiDisclosure::empty(),
    })
}
