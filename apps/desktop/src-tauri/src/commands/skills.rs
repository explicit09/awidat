//! Skills surface — Tauri commands that back the in-app Skills tab.
//!
//! The frontend Skills surface lists every bundled (and user-overridden)
//! skill so the editor can see what editorial workflows the agent has
//! at its disposal. The agent itself still loads skills via
//! `codex_session.rs::render_skills_catalog()` — we deliberately do
//! NOT touch that path here. These commands are read-only UI plumbing.
//!
//! Discovery mirrors `codex_session.rs`:
//!   1. Project-root override (`<project>/skills/`) when present —
//!      lets a project ship its own editorial loadout.
//!   2. Bundled (`<repo>/skills/` in dev) — same walk-up resolution
//!      `bundled_skill_root()` in codex_session uses.
//!   3. User overrides under
//!      `~/Library/Application Support/awidat/skills` and
//!      `~/.config/awidat/skills`. User entries win on name conflicts.
//!
//! Both commands use the existing `awidat_core::skills::SkillRegistry`
//! parser — we don't reimplement YAML frontmatter parsing here.

use std::path::PathBuf;

use awidat_core::skills::SkillRegistry;
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::state::AwidatState;

/// One skill row for the Skills surface. Mirrors the L1 fields from
/// `SkillMeta` plus the absolute path so the frontend can show "where
/// is this from" and `read_skill_body` can resolve it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillEntry {
    /// Stable id (directory name). Used to call `read_skill_body`.
    pub name: String,
    /// Same as `name` today — kept distinct so future skills can
    /// carry a human-formatted display name without breaking the id.
    pub display_name: String,
    /// One-line summary from the frontmatter.
    pub description: String,
    /// Optional `when_to_use` field, allowed multi-line. The bundled
    /// skills don't set this today, but `SKILL.md` from external
    /// authors might — we surface it when present and fall back to
    /// `description` in the UI when absent.
    pub when_to_use: Option<String>,
    /// Optional semver-ish version. `None` when the frontmatter
    /// didn't declare one; the parser substitutes `"0.1.0"` in
    /// `SkillMeta::version`, so we always have something to show.
    pub version: Option<String>,
    /// Absolute path to the skill's `SKILL.md`. The frontend uses
    /// this to render a "source" hint and the body-fetch command
    /// uses the directory name to look up the skill again.
    pub path: String,
}

/// List every discoverable skill, deduplicated by name. Order is
/// alphabetical (matches `SkillRegistry::all()`).
///
/// Errors are non-fatal — malformed skills are skipped and logged.
/// An empty list is a valid response (no bundled skills found).
#[tauri::command]
pub async fn list_skills(state: State<'_, AwidatState>) -> Result<Vec<SkillEntry>, String> {
    let project_root = state.project_root.lock().await.clone();
    tokio::task::spawn_blocking(move || Ok(collect_skills(project_root)))
        .await
        .map_err(|e| format!("list_skills join: {e}"))?
}

/// Return the raw `SKILL.md` body (frontmatter stripped) for the
/// named skill. Returns an error when the skill doesn't exist —
/// the UI's "select a skill" path should always pass a valid name
/// from the `list_skills` result.
#[tauri::command]
pub async fn read_skill_body(
    state: State<'_, AwidatState>,
    name: String,
) -> Result<String, String> {
    let project_root = state.project_root.lock().await.clone();
    tokio::task::spawn_blocking(move || {
        let registry = build_registry(project_root);
        registry
            .get(&name)
            .map(|s| s.body.clone())
            .ok_or_else(|| format!("skill '{name}' not found"))
    })
    .await
    .map_err(|e| format!("read_skill_body join: {e}"))?
}

/// Build the registry from project-override + bundled + user roots.
fn build_registry(project_root: Option<PathBuf>) -> SkillRegistry {
    let bundled = bundled_skill_root();
    let project_override = project_root.and_then(project_skill_root);
    let user_roots = user_skill_roots();

    // Layering: bundled first (lowest priority), then user roots,
    // then the project override (highest priority — a per-project
    // workflow trumps a personal one).
    let mut overlay_roots: Vec<PathBuf> = Vec::new();
    overlay_roots.extend(user_roots);
    if let Some(p) = project_override {
        overlay_roots.push(p);
    }

    let (registry, errors) =
        SkillRegistry::discover_many(bundled.as_deref(), overlay_roots.iter().map(PathBuf::as_path));
    for err in errors {
        tracing::warn!(?err, "skills surface: malformed entry skipped");
    }
    registry
}

fn collect_skills(project_root: Option<PathBuf>) -> Vec<SkillEntry> {
    let registry = build_registry(project_root);
    registry
        .all()
        .map(|s| SkillEntry {
            name: s.meta.name.clone(),
            display_name: s.meta.name.clone(),
            description: s.meta.description.clone(),
            // The current `SkillMeta` struct doesn't expose
            // `when_to_use` separately — the field is reserved for
            // future frontmatter extensions and intentionally None
            // for now so the UI's fallback (description) is what
            // renders.
            when_to_use: None,
            version: Some(s.meta.version.clone()),
            path: s.root.join("SKILL.md").display().to_string(),
        })
        .collect()
}

/// Per-project skills directory. We mirror the convention used by
/// `awidat skills run` — a project can drop a `skills/` folder at
/// its root to override or add editorial workflows.
fn project_skill_root(project_root: PathBuf) -> Option<PathBuf> {
    let candidate = project_root.join("skills");
    if candidate.is_dir() { Some(candidate) } else { None }
}

/// Mirror of `codex_session::user_skill_roots`. Kept local so this
/// command file is self-contained and doesn't reach into private
/// helpers in the session module.
fn user_skill_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(home) = dirs::home_dir() {
        roots.push(home.join("Library/Application Support/awidat/skills"));
        roots.push(home.join(".config/awidat/skills"));
    }
    roots
}

/// Mirror of `codex_session::bundled_skill_root`. In `cargo tauri
/// dev` the binary lives at `<repo>/target/debug/awidat-desktop`, so
/// the skills dir is `<repo>/skills`. Walk up three dirs from the
/// binary path.
fn bundled_skill_root() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let candidate = exe
        .parent()? // target/debug
        .parent()? // target
        .parent()? // repo root
        .join("skills");
    if candidate.is_dir() {
        Some(candidate)
    } else {
        None
    }
}
