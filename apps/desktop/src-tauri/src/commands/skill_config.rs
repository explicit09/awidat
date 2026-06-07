//! Per-project skill enable/disable + version-pinning config.
//!
//! The Skills surface (`apps/desktop/src/shell/SkillsSurface.tsx`) lets
//! the editor toggle individual skills on or off for the active project
//! AND pin a specific version / provenance layer (Wave 5 B2). The state
//! lives in `<project>/.montage/skills.json` so it survives reloads,
//! syncs through file-based project sharing (Dropbox/git/etc.), and is
//! visible from CLI invocations that don't load the React store.
//! `codex_session::render_skills_catalog()` reads the same file when
//! assembling the L1 catalog so disabled / pinned constraints apply
//! before the bridge ever sees a skill.
//!
//! Schema v2:
//!
//! ```json
//! {
//!   "version": 2,
//!   "disabled": ["auto-cutter"],
//!   "pinned": [
//!     {
//!       "name": "podcast-editor",
//!       "version": "1.2.0",
//!       "provenance": "project"
//!     }
//!   ]
//! }
//! ```
//!
//! - `version` — schema version. v1 (pre-Wave-5-B2) had no `pinned`;
//!   the loader migrates v1 → v2 on read by defaulting `pinned: []`.
//!   Migration is one-way: we always write v2.
//! - `disabled` — skill names the user disabled. Takes precedence over
//!   `pinned` (a disabled skill stays disabled even if a pin would
//!   otherwise select it).
//! - `pinned[*].name` — skill name to constrain.
//! - `pinned[*].version` — optional; when set, only skills whose
//!   `SkillMeta::version` equals this string load. Unresolved pin →
//!   warn + skip the skill entirely.
//! - `pinned[*].provenance` — optional; one of `"bundled"` / `"user"`
//!   / `"project"`. When set, only the matching discovery layer is
//!   considered. Unresolved pin → warn + skip.
//! - Missing file or malformed JSON => "no constraints". The agent
//!   loads everything. Fail-open by design — a broken file should
//!   never silently break the editor's loadout.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tokio::fs;

/// Subdirectory that holds `skills.json` and other montage-managed state
/// files (proxies, notes, thumbnails, …). Mirrors the convention used
/// across `commands/transcode.rs`, `commands/notes.rs`, etc.
const MONTAGE_DIR: &str = ".montage";
/// Filename for the skill enable/disable config.
const SKILLS_CONFIG_FILENAME: &str = "skills.json";
/// Schema version we write today. Bump alongside any breaking
/// `SkillConfig` shape change.
const CURRENT_VERSION: u32 = 2;

/// One pin entry — locks a skill to a specific version and/or
/// discovery layer. At least one of `version` / `provenance` should be
/// `Some` for the pin to do anything useful; a pin with both `None`
/// is a no-op (kept rather than rejected so manual edits don't crash
/// the loader).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct PinnedSkill {
    /// Skill name (matching `SkillMeta::name`) the pin applies to.
    pub name: String,
    /// Required exact version (`MAJOR.MINOR.PATCH`). `None` = any version.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Required discovery layer — `"bundled"`, `"user"`, or `"project"`.
    /// `None` = any layer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<String>,
}

/// On-disk shape. Fields default-initialize so partial files (e.g. only
/// `disabled` written by a hand-edit) still load cleanly. A v1 file
/// missing `pinned` reads as an empty pin list — the one-way v1 → v2
/// migration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SkillConfig {
    /// Schema version. The loader honors v1 (no `pinned`) and v2
    /// transparently via `#[serde(default)]`; we always write v2.
    #[serde(default = "default_version")]
    pub version: u32,
    /// Skill names (matching `SkillMeta::name`) the user disabled.
    /// Order is not significant; we sort on write for stable diffs.
    /// Takes precedence over `pinned`.
    #[serde(default)]
    pub disabled: Vec<String>,
    /// Version / provenance pins per skill name. Empty when no pins
    /// declared (default for a freshly-migrated v1 file).
    #[serde(default)]
    pub pinned: Vec<PinnedSkill>,
}

/// Back-compat alias — old code referenced the type by its v1 name.
/// New code should use `SkillConfig`.
#[allow(dead_code)]
pub type DisabledSkillsConfig = SkillConfig;

fn default_version() -> u32 {
    CURRENT_VERSION
}

/// Read the per-project disabled-skills list. Returns an empty vec when
/// the file is absent or malformed — see module docs for the fail-mode
/// rationale.
///
/// Synchronous helper so [`crate::codex_session`] can call it during
/// session launch without spinning up a separate async task.
///
/// Wave 5 B2 added [`load_skill_config_sync`] which returns the full
/// config (including `pinned`); this helper stays as a thin shim for
/// older call sites and tests that only need the disabled list.
#[allow(dead_code)]
pub fn load_disabled_skills_sync(project_root: &Path) -> Vec<String> {
    load_skill_config_sync(project_root).disabled
}

/// Read the full per-project skill config. Returns the default
/// (empty disabled + empty pinned) when the file is absent or
/// malformed. Used by `codex_session::render_skills_catalog`.
pub fn load_skill_config_sync(project_root: &Path) -> SkillConfig {
    let path = config_path(project_root);
    let Ok(bytes) = std::fs::read(&path) else {
        return SkillConfig::default();
    };
    match serde_json::from_slice::<SkillConfig>(&bytes) {
        Ok(cfg) => cfg,
        Err(err) => {
            tracing::warn!(
                path = %path.display(),
                error = %err,
                "skills.json malformed; treating as no constraints"
            );
            SkillConfig::default()
        }
    }
}

/// Tauri command — read the project's `skills.json` and return the
/// disabled skill names. Empty vec = nothing disabled (including
/// missing file). Kept for backwards compatibility with code that
/// only cared about disabled state.
#[tauri::command]
pub async fn read_disabled_skills(project_path: String) -> Result<Vec<String>, String> {
    Ok(read_skill_config(project_path).await?.disabled)
}

/// Tauri command — return the full skill config (disabled + pinned).
/// Used by the Skills surface to render pin status next to each card.
#[tauri::command]
pub async fn read_skill_config(project_path: String) -> Result<SkillConfig, String> {
    let root = validate_project_root(&project_path)?;
    let path = config_path(&root);
    match fs::read(&path).await {
        Ok(bytes) => match serde_json::from_slice::<SkillConfig>(&bytes) {
            Ok(cfg) => Ok(cfg),
            Err(err) => {
                // Front-end never sees the error — the agent's catalog
                // reader follows the same fail-open rule, so we keep
                // the UI consistent with it.
                tracing::warn!(
                    path = %path.display(),
                    error = %err,
                    "skills.json malformed; returning default"
                );
                Ok(SkillConfig::default())
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(SkillConfig::default()),
        Err(e) => Err(format!("read skills.json: {e}")),
    }
}

/// Tauri command — persist the project's disabled-skills list. The
/// frontend invokes this after every toggle. Preserves any existing
/// `pinned` entries by reading the current file first, mutating only
/// the `disabled` field, then writing back.
///
/// Creates `<project>/.montage/` if missing so the very first toggle on
/// a fresh project still succeeds.
#[tauri::command]
pub async fn write_disabled_skills(
    project_path: String,
    disabled: Vec<String>,
) -> Result<(), String> {
    let root = validate_project_root(&project_path)?;
    // Read the existing config so we don't clobber any `pinned`
    // entries that the user (or another mutation) set.
    let mut cfg = read_existing_or_default(&root).await;
    cfg.disabled = sort_and_dedupe(disabled);
    cfg.version = CURRENT_VERSION;
    write_config_to_disk(&root, &cfg).await
}

/// Tauri command — persist the full skill config (disabled + pinned).
/// The Skills surface calls this when the editor adds or removes a
/// version / provenance pin. Sorts + dedupes the disabled list on the
/// way out so writes are stable across machines.
#[tauri::command]
pub async fn write_skill_config(project_path: String, config: SkillConfig) -> Result<(), String> {
    let root = validate_project_root(&project_path)?;
    let cfg = SkillConfig {
        version: CURRENT_VERSION,
        disabled: sort_and_dedupe(config.disabled),
        pinned: sort_pins(config.pinned),
    };
    write_config_to_disk(&root, &cfg).await
}

/// Shared write helper — creates the `.montage/` dir, serializes, and
/// flushes the file. Internal-only.
async fn write_config_to_disk(root: &Path, cfg: &SkillConfig) -> Result<(), String> {
    let montage_dir = root.join(MONTAGE_DIR);
    fs::create_dir_all(&montage_dir)
        .await
        .map_err(|e| format!("create {MONTAGE_DIR}/: {e}"))?;
    let json = serde_json::to_vec_pretty(cfg).map_err(|e| format!("serialize skills.json: {e}"))?;
    let path = montage_dir.join(SKILLS_CONFIG_FILENAME);
    fs::write(&path, json)
        .await
        .map_err(|e| format!("write skills.json: {e}"))?;
    Ok(())
}

/// Read the config from disk if present, else default. Internal helper
/// used by `write_disabled_skills` to preserve existing pin state.
async fn read_existing_or_default(root: &Path) -> SkillConfig {
    let path = config_path(root);
    match fs::read(&path).await {
        Ok(bytes) => serde_json::from_slice::<SkillConfig>(&bytes).unwrap_or_default(),
        Err(_) => SkillConfig::default(),
    }
}

fn sort_and_dedupe(mut v: Vec<String>) -> Vec<String> {
    v.sort();
    v.dedup();
    v
}

fn sort_pins(mut pins: Vec<PinnedSkill>) -> Vec<PinnedSkill> {
    pins.sort_by(|a, b| a.name.cmp(&b.name));
    // Dedupe by name — last-write-wins for the same skill.
    let mut seen = std::collections::HashSet::new();
    pins.retain(|p| seen.insert(p.name.clone()));
    pins
}

/// `<project>/.montage/skills.json`.
fn config_path(project_root: &Path) -> PathBuf {
    project_root.join(MONTAGE_DIR).join(SKILLS_CONFIG_FILENAME)
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
        write_disabled_skills(tmp.path().to_string_lossy().into_owned(), names.clone())
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
    async fn write_creates_montage_dir_when_missing() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(!tmp.path().join(".montage").exists());
        write_disabled_skills(
            tmp.path().to_string_lossy().into_owned(),
            vec!["auto-cutter".to_string()],
        )
        .await
        .unwrap();
        assert!(tmp.path().join(".montage/skills.json").exists());
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
        std::fs::create_dir_all(tmp.path().join(".montage")).unwrap();
        std::fs::write(tmp.path().join(".montage/skills.json"), b"{not json").unwrap();
        let out = read_disabled_skills(tmp.path().to_string_lossy().into_owned())
            .await
            .unwrap();
        // Fail-open: malformed file means "agent loads everything".
        assert!(out.is_empty());
    }

    #[tokio::test]
    async fn read_tolerates_missing_version_field() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".montage")).unwrap();
        std::fs::write(
            tmp.path().join(".montage/skills.json"),
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
        std::fs::create_dir_all(tmp.path().join(".montage")).unwrap();
        std::fs::write(
            tmp.path().join(".montage/skills.json"),
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

    // --- Wave 5 B2 — pinning ---

    /// v1 file (no `pinned` field) reads cleanly with empty pin list —
    /// the one-way migration. The sync loader is what `codex_session`
    /// uses, so this is the critical path.
    #[test]
    fn load_skill_config_migrates_v1_to_v2() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".montage")).unwrap();
        std::fs::write(
            tmp.path().join(".montage/skills.json"),
            br#"{"version":1,"disabled":["a"]}"#,
        )
        .unwrap();
        let cfg = load_skill_config_sync(tmp.path());
        assert_eq!(cfg.disabled, vec!["a".to_string()]);
        assert!(
            cfg.pinned.is_empty(),
            "v1 reads as empty pin list; got {:?}",
            cfg.pinned
        );
    }

    /// v2 round-trip — write_skill_config + read_skill_config preserve
    /// pinned entries with version and provenance constraints.
    #[tokio::test]
    async fn write_then_read_skill_config_v2_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = SkillConfig {
            version: CURRENT_VERSION,
            disabled: vec!["disabled-skill".to_string()],
            pinned: vec![
                PinnedSkill {
                    name: "podcast-editor".to_string(),
                    version: Some("1.2.0".to_string()),
                    provenance: Some("project".to_string()),
                },
                PinnedSkill {
                    name: "auto-cutter".to_string(),
                    version: Some("1.0.0".to_string()),
                    provenance: None,
                },
            ],
        };
        write_skill_config(tmp.path().to_string_lossy().into_owned(), cfg.clone())
            .await
            .unwrap();
        let out = read_skill_config(tmp.path().to_string_lossy().into_owned())
            .await
            .unwrap();
        assert_eq!(out.version, CURRENT_VERSION);
        assert_eq!(out.disabled, vec!["disabled-skill".to_string()]);
        // Pins are sorted on write for stable diffs.
        assert_eq!(out.pinned.len(), 2);
        assert_eq!(out.pinned[0].name, "auto-cutter");
        assert_eq!(out.pinned[0].version.as_deref(), Some("1.0.0"));
        assert_eq!(out.pinned[1].name, "podcast-editor");
        assert_eq!(out.pinned[1].provenance.as_deref(), Some("project"));
    }

    /// `write_disabled_skills` must not clobber existing `pinned`
    /// entries — they're written from a separate UI affordance and
    /// shouldn't disappear when the user toggles a skill.
    #[tokio::test]
    async fn write_disabled_preserves_existing_pins() {
        let tmp = tempfile::tempdir().unwrap();
        let initial = SkillConfig {
            version: CURRENT_VERSION,
            disabled: vec![],
            pinned: vec![PinnedSkill {
                name: "pinned-skill".to_string(),
                version: Some("1.0.0".to_string()),
                provenance: None,
            }],
        };
        write_skill_config(tmp.path().to_string_lossy().into_owned(), initial)
            .await
            .unwrap();
        // Now toggle a skill off via the disabled-only writer.
        write_disabled_skills(
            tmp.path().to_string_lossy().into_owned(),
            vec!["different-skill".to_string()],
        )
        .await
        .unwrap();
        let out = read_skill_config(tmp.path().to_string_lossy().into_owned())
            .await
            .unwrap();
        assert_eq!(out.disabled, vec!["different-skill".to_string()]);
        assert_eq!(out.pinned.len(), 1);
        assert_eq!(out.pinned[0].name, "pinned-skill");
    }

    /// Pins dedupe by name on write — last entry wins.
    #[tokio::test]
    async fn write_skill_config_dedupes_pins_by_name() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = SkillConfig {
            version: CURRENT_VERSION,
            disabled: vec![],
            pinned: vec![
                PinnedSkill {
                    name: "dup".to_string(),
                    version: Some("1.0.0".to_string()),
                    provenance: None,
                },
                PinnedSkill {
                    name: "dup".to_string(),
                    version: Some("2.0.0".to_string()),
                    provenance: None,
                },
            ],
        };
        write_skill_config(tmp.path().to_string_lossy().into_owned(), cfg)
            .await
            .unwrap();
        let out = read_skill_config(tmp.path().to_string_lossy().into_owned())
            .await
            .unwrap();
        assert_eq!(out.pinned.len(), 1);
    }

    /// File on disk should always advertise the current schema version
    /// (we always write v2) even when the input declared v1.
    #[tokio::test]
    async fn write_always_emits_current_version() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = SkillConfig {
            version: 1,
            disabled: vec!["a".to_string()],
            pinned: vec![],
        };
        write_skill_config(tmp.path().to_string_lossy().into_owned(), cfg)
            .await
            .unwrap();
        let raw = std::fs::read_to_string(tmp.path().join(".montage/skills.json")).unwrap();
        assert!(
            raw.contains("\"version\": 2"),
            "expected current version; got:\n{raw}"
        );
    }
}
