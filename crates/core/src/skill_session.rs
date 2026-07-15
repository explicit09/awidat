//! Active skill session state + runtime tool allowlist enforcement.
//!
//! Skill frontmatter may declare `tools_allowlist`. That used to be
//! documentation only. After `load_skill`, the allowlist is persisted
//! under `<project>/.montage/active_skill.json` and every MCP tool call
//! is checked against it. Empty allowlist (or no active skill) means
//! unrestricted access.
//!
//! Meta tools listed in [`ALWAYS_ALLOWED`] remain callable so the agent
//! can switch skills, surface plans, and complete turns without being
//! permanently locked out.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Project-relative path for the active skill session file.
pub const ACTIVE_SKILL_REL: &str = ".montage/active_skill.json";

/// Tools that remain callable even when a skill allowlist is active.
pub const ALWAYS_ALLOWED: &[&str] = &[
    "load_skill",
    "load_project_instructions",
    "attempt_completion",
    "update_plan",
    "request_user_input",
    // Department handoff must remain reachable mid-skill.
    "set_picture_lock",
];

/// Persisted active skill binding for a project.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActiveSkill {
    /// Skill name from the catalog.
    pub name: String,
    /// Tool names the agent may call while this skill is active.
    /// Empty = no restriction for this skill.
    #[serde(default)]
    pub tools_allowlist: Vec<String>,
    /// When the skill was loaded.
    pub loaded_at: DateTime<Utc>,
}

/// Absolute path to the active skill file for `project_root`.
pub fn active_skill_path(project_root: &Path) -> PathBuf {
    project_root.join(ACTIVE_SKILL_REL)
}

/// Read the active skill, if any.
pub fn read_active_skill(project_root: &Path) -> Option<ActiveSkill> {
    let path = active_skill_path(project_root);
    let bytes = std::fs::read(&path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Persist or clear the active skill.
///
/// When `tools_allowlist` is empty the active skill file is removed so
/// unrestricted tool access is restored (skills without allowlists do
/// not lock the surface).
pub fn set_active_skill(
    project_root: &Path,
    name: impl Into<String>,
    tools_allowlist: Vec<String>,
) -> Result<Option<ActiveSkill>, String> {
    let name = name.into();
    let path = active_skill_path(project_root);
    if tools_allowlist.is_empty() {
        clear_active_skill(project_root)?;
        return Ok(None);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("skill_session: create {}: {e}", parent.display()))?;
    }
    let active = ActiveSkill {
        name,
        tools_allowlist,
        loaded_at: Utc::now(),
    };
    let body = serde_json::to_vec_pretty(&active)
        .map_err(|e| format!("skill_session: serialize active skill: {e}"))?;
    std::fs::write(&path, body)
        .map_err(|e| format!("skill_session: write {}: {e}", path.display()))?;
    Ok(Some(active))
}

/// Clear any active skill restriction for the project.
pub fn clear_active_skill(project_root: &Path) -> Result<(), String> {
    let path = active_skill_path(project_root);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("skill_session: remove {}: {e}", path.display())),
    }
}

/// Return `Ok(())` when `tool` may run under the project's active skill.
pub fn check_tool_allowed(project_root: &Path, tool: &str) -> Result<(), String> {
    if std::env::var_os("MONTAGE_DISABLE_SKILL_ALLOWLIST").is_some() {
        return Ok(());
    }
    if ALWAYS_ALLOWED.contains(&tool) {
        return Ok(());
    }
    let Some(active) = read_active_skill(project_root) else {
        return Ok(());
    };
    if active.tools_allowlist.is_empty() {
        return Ok(());
    }
    if active.tools_allowlist.iter().any(|t| t == tool) {
        return Ok(());
    }
    Err(format!(
        "tool `{tool}` is blocked by active skill `{}` tools_allowlist. \
         Allowed tools: {}. Call `load_skill` with a different skill \
         (or a skill without an allowlist) to change the surface.",
        active.name,
        active.tools_allowlist.join(", ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unrestricted_when_no_active_skill() {
        let dir = tempfile::tempdir().unwrap();
        assert!(check_tool_allowed(dir.path(), "apply_edl").is_ok());
    }

    #[test]
    fn allowlist_blocks_other_tools() {
        let dir = tempfile::tempdir().unwrap();
        set_active_skill(
            dir.path(),
            "auto-cutter",
            vec!["view_timeline".into(), "apply_edl".into()],
        )
        .unwrap();
        assert!(check_tool_allowed(dir.path(), "apply_edl").is_ok());
        assert!(check_tool_allowed(dir.path(), "load_skill").is_ok());
        let err = check_tool_allowed(dir.path(), "start_render").unwrap_err();
        assert!(err.contains("blocked by active skill"), "{err}");
    }

    #[test]
    fn empty_allowlist_clears_restriction() {
        let dir = tempfile::tempdir().unwrap();
        set_active_skill(dir.path(), "x", vec!["apply_edl".into()]).unwrap();
        set_active_skill(dir.path(), "y", Vec::new()).unwrap();
        assert!(read_active_skill(dir.path()).is_none());
        assert!(check_tool_allowed(dir.path(), "start_render").is_ok());
    }
}
