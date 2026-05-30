//! Per-project indexer enable/disable overlay (Wave 4 T3).
//!
//! IndexRailPro's `IndexersStrip` lets the editor toggle individual
//! indexers on or off for the active project. Wave 2 trimmed the old
//! inline panel down to read-only chips; Wave 4 T3 restores the
//! affordance with a small popover. This module mirrors `skill_config`
//! and persists the disabled set to
//! `<project>/.awidat/indexers.json` so the state survives reloads,
//! syncs through file-based project sharing (Dropbox/git/etc.), and
//! is visible to `index_project_at_root` when it filters the indexer
//! list before dispatching.
//!
//! Schema:
//!
//! ```json
//! {
//!   "version": 1,
//!   "disabled": ["face", "motion"]
//! }
//! ```
//!
//! - `version` is reserved for future migrations; today's reader treats
//!   any value as compatible.
//! - `disabled` lists indexer `name`s (matching `McpServer::name`). The
//!   dispatcher drops any server whose name appears here.
//! - Missing file or malformed JSON => "nothing disabled". The same
//!   fail-open rule we use for `skills.json`: a broken overlay should
//!   never silently disable indexers the user thought were active.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tokio::fs;

/// Subdirectory that holds project-managed state files. Same convention
/// as `commands/skill_config.rs`, `commands/notes.rs`, etc.
const AWIDAT_DIR: &str = ".awidat";
/// Filename for the indexer enable/disable overlay.
const INDEXERS_CONFIG_FILENAME: &str = "indexers.json";
/// Schema version we write today. Bump alongside any breaking
/// `DisabledIndexersConfig` shape change; readers stay tolerant.
const CURRENT_VERSION: u32 = 1;

/// On-disk shape. Both fields default-initialize so partial files
/// (e.g. only `disabled` written by a hand-edit) still load cleanly.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DisabledIndexersConfig {
    /// Schema version. Today's loader ignores the value; kept for
    /// forward-compatibility once we need real migrations.
    #[serde(default = "default_version")]
    pub version: u32,
    /// Indexer names (matching `McpServer::name`) the user disabled.
    /// Order is not significant; we sort on write for stable diffs.
    #[serde(default)]
    pub disabled: Vec<String>,
}

fn default_version() -> u32 {
    CURRENT_VERSION
}

/// Read the per-project disabled-indexers list. Returns an empty vec
/// when the file is absent or malformed — see module docs for the
/// fail-mode rationale.
///
/// Synchronous helper so `index_project_at_root` can call it during
/// dispatch setup without spinning up a separate async task.
pub fn load_disabled_indexers_sync(project_root: &Path) -> Vec<String> {
    let path = config_path(project_root);
    let Ok(bytes) = std::fs::read(&path) else {
        return Vec::new();
    };
    match serde_json::from_slice::<DisabledIndexersConfig>(&bytes) {
        Ok(cfg) => cfg.disabled,
        Err(err) => {
            tracing::warn!(
                path = %path.display(),
                error = %err,
                "indexers.json malformed; treating as no indexers disabled"
            );
            Vec::new()
        }
    }
}

/// Tauri command — read the project's `indexers.json` and return the
/// disabled indexer names. Empty vec = nothing disabled (including
/// missing file).
#[tauri::command]
pub async fn read_disabled_indexers(project_path: String) -> Result<Vec<String>, String> {
    let root = validate_project_root(&project_path)?;
    let path = config_path(&root);
    match fs::read(&path).await {
        Ok(bytes) => match serde_json::from_slice::<DisabledIndexersConfig>(&bytes) {
            Ok(cfg) => Ok(cfg.disabled),
            Err(err) => {
                // Mirror skill_config: the dispatcher's overlay loader
                // fails open, so the UI stays consistent with it.
                tracing::warn!(
                    path = %path.display(),
                    error = %err,
                    "indexers.json malformed; returning empty disabled list"
                );
                Ok(Vec::new())
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(format!("read indexers.json: {e}")),
    }
}

/// Tauri command — persist the project's disabled-indexers list. The
/// frontend invokes this after every toggle; the file is small enough
/// that re-writing the whole thing is the simplest correct strategy.
///
/// Creates `<project>/.awidat/` if missing so the very first toggle on
/// a fresh project still succeeds.
#[tauri::command]
pub async fn write_disabled_indexers(
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

    let cfg = DisabledIndexersConfig {
        version: CURRENT_VERSION,
        disabled: sorted,
    };
    let json =
        serde_json::to_vec_pretty(&cfg).map_err(|e| format!("serialize indexers.json: {e}"))?;
    let path = awidat_dir.join(INDEXERS_CONFIG_FILENAME);
    fs::write(&path, json)
        .await
        .map_err(|e| format!("write indexers.json: {e}"))?;
    Ok(())
}

/// `<project>/.awidat/indexers.json`.
fn config_path(project_root: &Path) -> PathBuf {
    project_root.join(AWIDAT_DIR).join(INDEXERS_CONFIG_FILENAME)
}

/// Same guard `skill_config` uses — reject empty / relative inputs
/// before they hit the filesystem and resolve against cwd.
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
        let out = read_disabled_indexers(tmp.path().to_string_lossy().into_owned())
            .await
            .unwrap();
        assert!(out.is_empty());
    }

    #[tokio::test]
    async fn write_then_read_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let names = vec!["face".to_string(), "motion".to_string()];
        write_disabled_indexers(tmp.path().to_string_lossy().into_owned(), names.clone())
            .await
            .unwrap();
        let out = read_disabled_indexers(tmp.path().to_string_lossy().into_owned())
            .await
            .unwrap();
        // Sorted on write — assert the deterministic order, not the
        // input order.
        assert_eq!(out, vec!["face", "motion"]);
    }

    #[tokio::test]
    async fn write_creates_awidat_dir_when_missing() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(!tmp.path().join(".awidat").exists());
        write_disabled_indexers(
            tmp.path().to_string_lossy().into_owned(),
            vec!["face".to_string()],
        )
        .await
        .unwrap();
        assert!(tmp.path().join(".awidat/indexers.json").exists());
    }

    #[tokio::test]
    async fn write_sorts_and_dedupes() {
        let tmp = tempfile::tempdir().unwrap();
        write_disabled_indexers(
            tmp.path().to_string_lossy().into_owned(),
            vec![
                "scenedetect".to_string(),
                "face".to_string(),
                "face".to_string(),
            ],
        )
        .await
        .unwrap();
        let out = read_disabled_indexers(tmp.path().to_string_lossy().into_owned())
            .await
            .unwrap();
        assert_eq!(out, vec!["face", "scenedetect"]);
    }

    #[tokio::test]
    async fn read_returns_empty_on_malformed_json() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".awidat")).unwrap();
        std::fs::write(tmp.path().join(".awidat/indexers.json"), b"{not json").unwrap();
        let out = read_disabled_indexers(tmp.path().to_string_lossy().into_owned())
            .await
            .unwrap();
        // Fail-open: malformed file means "dispatcher runs everything".
        assert!(out.is_empty());
    }

    #[tokio::test]
    async fn read_tolerates_missing_version_field() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".awidat")).unwrap();
        std::fs::write(
            tmp.path().join(".awidat/indexers.json"),
            br#"{"disabled":["face","motion"]}"#,
        )
        .unwrap();
        let out = read_disabled_indexers(tmp.path().to_string_lossy().into_owned())
            .await
            .unwrap();
        assert_eq!(out, vec!["face", "motion"]);
    }

    #[test]
    fn load_disabled_indexers_sync_matches_async_path() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".awidat")).unwrap();
        std::fs::write(
            tmp.path().join(".awidat/indexers.json"),
            br#"{"version":1,"disabled":["face"]}"#,
        )
        .unwrap();
        assert_eq!(
            load_disabled_indexers_sync(tmp.path()),
            vec!["face".to_string()]
        );
    }

    #[test]
    fn load_disabled_indexers_sync_returns_empty_when_missing() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(load_disabled_indexers_sync(tmp.path()).is_empty());
    }

    #[tokio::test]
    async fn write_refuses_relative_paths() {
        let err = write_disabled_indexers("relative/path".to_string(), vec![])
            .await
            .unwrap_err();
        assert!(err.contains("must be absolute"), "got: {err}");
    }

    #[tokio::test]
    async fn read_refuses_relative_paths() {
        let err = read_disabled_indexers("relative/path".to_string())
            .await
            .unwrap_err();
        assert!(err.contains("must be absolute"), "got: {err}");
    }

    #[tokio::test]
    async fn read_refuses_empty_path() {
        let err = read_disabled_indexers(String::new()).await.unwrap_err();
        assert!(err.contains("empty"), "got: {err}");
    }
}
