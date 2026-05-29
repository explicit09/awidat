//! Per-project skill enable/disable config (Wave 4 T1).
//!
//! The Skills surface (`apps/desktop/src/shell/SkillsSurface.tsx`) lets
//! the editor toggle individual skills on or off for the active project.
//! Wave 3 shipped the UI; this module ships the persistence:
//!
//! Disabled skills are stored at `<project>/.awidat/skills.json` so the
//! state survives reloads, syncs through file-based project sharing
//! (Dropbox/git/etc.), and is visible from CLI invocations that don't
//! load the React store. `codex_session::render_skills_catalog()` reads
//! the same file when assembling the L1 catalog so a "disabled" skill
//! never lands in the agent's loadout.
//!
//! Schema:
//!
//! ```json
//! {
//!   "version": 1,
//!   "disabled": ["auto-cutter", "b-roll-suggester"]
//! }
//! ```
//!
//! - `version` is reserved for future migrations; today's reader treats
//!   any value as compatible (the field is purely informational).
//! - `disabled` is the only meaningful field. Absence = nothing disabled.
//! - Missing file or malformed JSON => "no skills disabled". The agent
//!   loads everything. We deliberately do not surface parse errors to
//!   the user — a broken file should never silently disable skills
//!   the user thought were active, and going the other way (loading
//!   everything) is the safer fail mode.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tokio::fs;

/// Subdirectory that holds `skills.json` and other awidat-managed state
/// files (proxies, notes, thumbnails, …). Mirrors the convention used
/// across `commands/transcode.rs`, `commands/notes.rs`, etc.
const AWIDAT_DIR: &str = ".awidat";
/// Filename for the skill enable/disable config.
const SKILLS_CONFIG_FILENAME: &str = "skills.json";
/// Schema version we write today. Bump alongside any breaking
/// `DisabledSkillsConfig` shape change; readers stay tolerant.
const CURRENT_VERSION: u32 = 1;

/// On-disk shape. Both fields default-initialize so partial files
/// (e.g. only `disabled` written by a hand-edit) still load cleanly.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DisabledSkillsConfig {
    /// Schema version. Today's loader ignores the value; kept for
    /// forward-compatibility once we need real migrations.
    #[serde(default = "default_version")]
    pub version: u32,
    /// Skill names (matching `SkillMeta::name`) the user disabled.
    /// Order is not significant; we sort on write for stable diffs.
    #[serde(default)]
    pub disabled: Vec<String>,
}

fn default_version() -> u32 {
    CURRENT_VERSION
}

/// Read the per-project disabled-skills list. Returns an empty vec when
/// the file is absent or malformed — see module docs for the fail-mode
/// rationale.
///
/// Synchronous helper so [`crate::codex_session`] can call it during
/// session launch without spinning up a separate async task.
pub fn load_disabled_skills_sync(project_root: &Path) -> Vec<String> {
    let path = config_path(project_root);
    let Ok(bytes) = std::fs::read(&path) else {
        return Vec::new();
    };
    match serde_json::from_slice::<DisabledSkillsConfig>(&bytes) {
        Ok(cfg) => cfg.disabled,
        Err(err) => {
            tracing::warn!(
                path = %path.display(),
                error = %err,
                "skills.json malformed; treating as no skills disabled"
            );
            Vec::new()
        }
    }
}

/// Tauri command — read the project's `skills.json` and return the
/// disabled skill names. Empty vec = nothing disabled (including
/// missing file).
#[tauri::command]
pub async fn read_disabled_skills(project_path: String) -> Result<Vec<String>, String> {
    let root = validate_project_root(&project_path)?;
    let path = config_path(&root);
    match fs::read(&path).await {
        Ok(bytes) => match serde_json::from_slice::<DisabledSkillsConfig>(&bytes) {
            Ok(cfg) => Ok(cfg.disabled),
            Err(err) => {
                // Front-end never sees the error — the agent's catalog
                // reader follows the same fail-open rule, so we keep
                // the UI consistent with it.
                tracing::warn!(
                    path = %path.display(),
                    error = %err,
                    "skills.json malformed; returning empty disabled list"
                );
                Ok(Vec::new())
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(format!("read skills.json: {e}")),
    }
}

/// Tauri command — persist the project's disabled-skills list. The
/// frontend invokes this after every toggle; the file is small enough
/// that re-writing the whole thing is the simplest correct strategy.
///
/// Creates `<project>/.awidat/` if missing so the very first toggle on
/// a fresh project still succeeds.
#[tauri::command]
pub async fn write_disabled_skills(
    project_path: String,
    disabled: Vec<String>,
) -> Result<(), String> {
    let root = validate_project_root(&project_path)?;
    let awidat_dir = root.join(AWIDAT_DIR);
    fs::create_dir_all(&awidat_dir)
        .await
        .map_err(|e| format!("create {AWIDAT_DIR}/: {e}"))?;

    // Sort + dedupe so the on-disk shape is stable across writes —
    // friendly to diff tools and to source control if the user is
    // syncing the project that way.
    let mut sorted = disabled;
    sorted.sort();
    sorted.dedup();

    let cfg = DisabledSkillsConfig {
        version: CURRENT_VERSION,
        disabled: sorted,
    };
    let json = serde_json::to_vec_pretty(&cfg)
        .map_err(|e| format!("serialize skills.json: {e}"))?;
    let path = awidat_dir.join(SKILLS_CONFIG_FILENAME);
    fs::write(&path, json)
        .await
        .map_err(|e| format!("write skills.json: {e}"))?;
    Ok(())
}

/// `<project>/.awidat/skills.json`.
fn config_path(project_root: &Path) -> PathBuf {
    project_root.join(AWIDAT_DIR).join(SKILLS_CONFIG_FILENAME)
}

/// Same guard the AGENTS.md command uses — reject empty / relative
/// inputs before they hit the filesystem and resolve against cwd.
fn validate_project_root(project_path: &str) -> Result<PathBuf, String> {
    if project_path.is_empty() {
        return Err("project_path is empty".into());
    }
    let buf = PathBuf::from(project_path);
    if !buf.is_absolute() {
        return Err(format!("project_path must be absolute: {project_path}"));
    }
    if !Path::new(&buf).is_dir() {
        return Err(format!("not a directory: {project_path}"));
    }
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn read_returns_empty_when_file_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let out = read_disabled_skills(tmp.path().to_string_lossy().into_owned())
            .await
            .unwrap();
        assert!(out.is_empty());
    }

    #[tokio::test]
    async fn write_then_read_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let names = vec!["auto-cutter".to_string(), "b-roll-suggester".to_string()];
        write_disabled_skills(
            tmp.path().to_string_lossy().into_owned(),
            names.clone(),
        )
        .await
        .unwrap();
        let out = read_disabled_skills(tmp.path().to_string_lossy().into_owned())
            .await
            .unwrap();
        // Sorted on write — assert the deterministic order, not the
        // input order.
        assert_eq!(out, vec!["auto-cutter", "b-roll-suggester"]);
    }

    #[tokio::test]
    async fn write_creates_awidat_dir_when_missing() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(!tmp.path().join(".awidat").exists());
        write_disabled_skills(
            tmp.path().to_string_lossy().into_owned(),
            vec!["auto-cutter".to_string()],
        )
        .await
        .unwrap();
        assert!(tmp.path().join(".awidat/skills.json").exists());
    }

    #[tokio::test]
    async fn write_sorts_and_dedupes() {
        let tmp = tempfile::tempdir().unwrap();
        write_disabled_skills(
            tmp.path().to_string_lossy().into_owned(),
            vec![
                "z-skill".to_string(),
                "a-skill".to_string(),
                "a-skill".to_string(),
            ],
        )
        .await
        .unwrap();
        let out = read_disabled_skills(tmp.path().to_string_lossy().into_owned())
            .await
            .unwrap();
        assert_eq!(out, vec!["a-skill", "z-skill"]);
    }

    #[tokio::test]
    async fn read_returns_empty_on_malformed_json() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".awidat")).unwrap();
        std::fs::write(tmp.path().join(".awidat/skills.json"), b"{not json").unwrap();
        let out = read_disabled_skills(tmp.path().to_string_lossy().into_owned())
            .await
            .unwrap();
        // Fail-open: malformed file means "agent loads everything".
        assert!(out.is_empty());
    }

    #[tokio::test]
    async fn read_tolerates_missing_version_field() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".awidat")).unwrap();
        std::fs::write(
            tmp.path().join(".awidat/skills.json"),
            br#"{"disabled":["a","b"]}"#,
        )
        .unwrap();
        let out = read_disabled_skills(tmp.path().to_string_lossy().into_owned())
            .await
            .unwrap();
        assert_eq!(out, vec!["a", "b"]);
    }

    #[test]
    fn load_disabled_skills_sync_matches_async_path() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".awidat")).unwrap();
        std::fs::write(
            tmp.path().join(".awidat/skills.json"),
            br#"{"version":1,"disabled":["auto-cutter"]}"#,
        )
        .unwrap();
        assert_eq!(
            load_disabled_skills_sync(tmp.path()),
            vec!["auto-cutter".to_string()]
        );
    }

    #[test]
    fn load_disabled_skills_sync_returns_empty_when_missing() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(load_disabled_skills_sync(tmp.path()).is_empty());
    }

    #[tokio::test]
    async fn write_refuses_relative_paths() {
        let err = write_disabled_skills("relative/path".to_string(), vec![])
            .await
            .unwrap_err();
        assert!(err.contains("must be absolute"), "got: {err}");
    }

    #[tokio::test]
    async fn read_refuses_relative_paths() {
        let err = read_disabled_skills("relative/path".to_string())
            .await
            .unwrap_err();
        assert!(err.contains("must be absolute"), "got: {err}");
    }

    #[tokio::test]
    async fn read_refuses_empty_path() {
        let err = read_disabled_skills(String::new()).await.unwrap_err();
        assert!(err.contains("empty"), "got: {err}");
    }
}
