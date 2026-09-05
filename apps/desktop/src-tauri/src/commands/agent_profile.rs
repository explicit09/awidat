//! Per-project GPT-5.6 Codex capability profile.

use std::path::Path;

use montage_desktop_protocol::AgentProfile;
use tauri::State;

use crate::state::MontageState;

const PROFILE_FILE: &str = "agent_profile";

#[tauri::command]
pub async fn get_agent_profile(state: State<'_, MontageState>) -> Result<AgentProfile, String> {
    let project_root = match state.project_root.lock().await.clone() {
        Some(path) => path,
        None => return Ok(AgentProfile::Balanced),
    };
    Ok(read_profile(&project_root))
}

#[tauri::command]
pub async fn set_agent_profile(
    state: State<'_, MontageState>,
    profile: AgentProfile,
) -> Result<(), String> {
    let project_root = state
        .project_root
        .lock()
        .await
        .clone()
        .ok_or_else(|| "no project loaded".to_string())?;
    write_profile(&project_root, profile)
}

pub(crate) fn read_profile(project_root: &Path) -> AgentProfile {
    let path = project_root.join(".montage").join(PROFILE_FILE);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(_) => return AgentProfile::Balanced,
    };
    match text.trim() {
        "balanced" => AgentProfile::Balanced,
        "deep_edit" => AgentProfile::DeepEdit,
        other => {
            tracing::warn!(
                value = %other,
                path = %path.display(),
                "unknown agent_profile value; falling back to balanced",
            );
            AgentProfile::Balanced
        }
    }
}

fn write_profile(project_root: &Path, profile: AgentProfile) -> Result<(), String> {
    let path = project_root.join(".montage").join(PROFILE_FILE);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| format!("create .montage: {error}"))?;
    }
    let value = match profile {
        AgentProfile::Balanced => "balanced",
        AgentProfile::DeepEdit => "deep_edit",
    };
    std::fs::write(path, value).map_err(|error| format!("write agent_profile: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_defaults_to_balanced_and_roundtrips_deep_edit() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read_profile(dir.path()), AgentProfile::Balanced);

        write_profile(dir.path(), AgentProfile::DeepEdit).unwrap();
        assert_eq!(read_profile(dir.path()), AgentProfile::DeepEdit);
    }

    #[test]
    fn unknown_profile_falls_back_to_balanced() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".montage").join("agent_profile");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "unknown").unwrap();

        assert_eq!(read_profile(dir.path()), AgentProfile::Balanced);
    }
}
