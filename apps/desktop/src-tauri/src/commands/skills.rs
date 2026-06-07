//! Skills surface — Tauri commands that back the in-app Skills tab.
//!
//! The frontend Skills surface lists every bundled (and user-overridden)
//! skill so the editor can see what editorial workflows the agent has
//! at its disposal. The agent itself still loads skills via
//! `codex_session.rs::render_skills_catalog()` — we deliberately do
//! NOT touch that path here. These commands are read-only UI plumbing.
//!
//! Discovery mirrors `codex_session.rs` (lowest priority first; later
//! layers override earlier on skill-name conflict, replacing entries
//! entirely — not a field-by-field merge):
//!   1. Bundled (`<repo>/skills/` in dev) — same walk-up resolution
//!      `bundled_skill_root()` in codex_session uses.
//!   2. User overrides under the platform config dir
//!      (`~/Library/Application Support/montage/skills` on macOS,
//!      `~/.config/montage/skills` on Linux, `%APPDATA%\montage\skills`
//!      on Windows) plus the legacy `~/.montage/skills`.
//!   3. Project-root override (`<project>/skills/`) when present —
//!      a per-project workflow trumps user + bundled.
//!
//! Every returned `SkillEntry` carries a `provenance` discriminator
//! ("bundled" / "user" / "project") so the frontend can render a chip
//! showing where the skill came from. The provenance reflects which
//! layer the registry actually resolved the skill from after overrides.
//!
//! Both commands use the existing `montage_core::skills::SkillRegistry`
//! parser — we don't reimplement YAML frontmatter parsing here.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use montage_core::skills::{SkillRegistry, is_hidden_skill_dir};
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::state::MontageState;

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
    /// Which discovery layer this skill was resolved from after
    /// overrides applied. One of `"bundled"`, `"user"`, `"project"`.
    /// The Skills tab renders a provenance chip from this field.
    pub provenance: String,
}

/// Discovery layer identifier — kept as a tiny enum internally and
/// serialized as a lowercase string for the frontend chip.
#[derive(Debug, Clone, Copy)]
enum Provenance {
    Bundled,
    User,
    Project,
}

impl Provenance {
    fn as_str(self) -> &'static str {
        match self {
            Self::Bundled => "bundled",
            Self::User => "user",
            Self::Project => "project",
        }
    }
}

/// List every discoverable skill, deduplicated by name. Order is
/// alphabetical (matches `SkillRegistry::all()`).
///
/// Errors are non-fatal — malformed skills are skipped and logged.
/// An empty list is a valid response (no bundled skills found).
#[tauri::command]
pub async fn list_skills(state: State<'_, MontageState>) -> Result<Vec<SkillEntry>, String> {
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
    state: State<'_, MontageState>,
    name: String,
) -> Result<String, String> {
    let project_root = state.project_root.lock().await.clone();
    tokio::task::spawn_blocking(move || {
        let layers = DiscoveryLayers::resolve(project_root);
        let registry = build_registry(&layers);
        registry
            .get(&name)
            .map(|s| s.body.clone())
            .ok_or_else(|| format!("skill '{name}' not found"))
    })
    .await
    .map_err(|e| format!("read_skill_body join: {e}"))?
}

/// Resolved discovery layers (bundled root, user roots, project root).
/// Cached as paths so `collect_skills` can run the priority sweep that
/// labels each skill with its provenance — and so `build_registry`
/// stays a thin layer-stitcher.
struct DiscoveryLayers {
    bundled: Option<PathBuf>,
    user: Vec<PathBuf>,
    project: Option<PathBuf>,
}

impl DiscoveryLayers {
    fn resolve(project_root: Option<PathBuf>) -> Self {
        Self {
            bundled: bundled_skill_root(),
            user: user_skill_roots(),
            project: project_root.and_then(project_skill_root),
        }
    }

    /// Flatten the user + project overlays into a single ordered vec
    /// (mid → high priority). Bundled stays separate because
    /// `SkillRegistry::discover_many` takes it as a dedicated arg.
    fn overlays(&self) -> Vec<PathBuf> {
        let mut roots = Vec::with_capacity(self.user.len() + 1);
        roots.extend(self.user.iter().cloned());
        if let Some(p) = &self.project {
            roots.push(p.clone());
        }
        roots
    }
}

/// Build the registry from bundled + user + project layers.
fn build_registry(layers: &DiscoveryLayers) -> SkillRegistry {
    let overlay_roots = layers.overlays();
    let (registry, errors) = SkillRegistry::discover_many(
        layers.bundled.as_deref(),
        overlay_roots.iter().map(PathBuf::as_path),
    );
    for err in errors {
        tracing::warn!(?err, "skills surface: malformed entry skipped");
    }
    registry
}

/// Compute a `name → provenance` lookup table that mirrors what
/// `SkillRegistry::discover_many` would have produced. We scan each
/// layer for SKILL.md-containing subdirectories and overwrite the
/// entry on conflict — same precedence rule the registry uses, so
/// the chip is guaranteed to match the resolved entry.
fn provenance_table(layers: &DiscoveryLayers) -> HashMap<String, Provenance> {
    let mut table: HashMap<String, Provenance> = HashMap::new();
    let mut record = |root: &Path, label: Provenance| {
        let Ok(entries) = std::fs::read_dir(root) else {
            return;
        };
        for entry in entries.flatten() {
            let p = entry.path();
            if !p.is_dir() {
                continue;
            }
            // Same hidden-dir filter the registry applies — keeps the
            // `_template/` scaffold and dotfile noise out of the
            // Skills tab list. Mirrors `SkillRegistry::discover_many`.
            if is_hidden_skill_dir(&p) {
                continue;
            }
            if !p.join("SKILL.md").is_file() {
                continue;
            }
            let Some(name) = p.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            table.insert(name.to_string(), label);
        }
    };
    if let Some(b) = &layers.bundled {
        record(b, Provenance::Bundled);
    }
    for u in &layers.user {
        record(u, Provenance::User);
    }
    if let Some(p) = &layers.project {
        record(p, Provenance::Project);
    }
    table
}

fn collect_skills(project_root: Option<PathBuf>) -> Vec<SkillEntry> {
    let layers = DiscoveryLayers::resolve(project_root);
    let registry = build_registry(&layers);
    let provenance = provenance_table(&layers);
    registry
        .all()
        .map(|s| {
            let chip = provenance
                .get(&s.meta.name)
                .copied()
                // Fallback: skill loaded but no layer matched it (e.g.
                // a future-added discovery source). Treat as bundled so
                // the chip never disappears.
                .unwrap_or(Provenance::Bundled);
            SkillEntry {
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
                provenance: chip.as_str().to_string(),
            }
        })
        .collect()
}

/// Per-project skills directory. We mirror the convention used by
/// `montage skills run` — a project can drop a `skills/` folder at
/// its root to override or add editorial workflows.
fn project_skill_root(project_root: PathBuf) -> Option<PathBuf> {
    let candidate = project_root.join("skills");
    if candidate.is_dir() {
        Some(candidate)
    } else {
        None
    }
}

/// Mirror of `codex_session::user_skill_roots`. Kept local so this
/// command file is self-contained and doesn't reach into private
/// helpers in the session module.
///
/// Order matches the session module (legacy `~/.montage/skills` then
/// the platform config dir). The "primary" user skills dir — the one
/// the Skills tab opens via `reveal_user_skills_dir` and auto-creates
/// on first launch — is `user_skills_primary_dir`, which always
/// resolves to the platform `dirs::config_dir()` location.
fn user_skill_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(legacy) = dirs::home_dir().map(|h| h.join(".montage/skills")) {
        roots.push(legacy);
    }
    if let Some(cfg) = user_skills_primary_dir() {
        roots.push(cfg);
    }
    roots
}

/// The "canonical" per-user skills directory — the one we surface to
/// the editor as "the place to drop new skills". Lives under
/// `dirs::config_dir()` so it follows the platform convention:
///
///   macOS   → `~/Library/Application Support/montage/skills`
///   Linux   → `~/.config/montage/skills`
///   Windows → `%APPDATA%\montage\skills`
fn user_skills_primary_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|c| c.join("montage").join("skills"))
}

/// Ensure the per-user skills directory exists and return its absolute
/// path. The frontend calls this when the Skills tab opens so the
/// "Open skills folder" affordance always lands somewhere real, even
/// on a fresh install where the user hasn't dropped any skills yet.
///
/// Returns the resolved path on success. An empty directory is the
/// expected fresh-install state — the discovery walk over it yields
/// no skills, which is fine; bundled skills still load. Errors are
/// returned as strings so the React side can surface them in the
/// Skills tab if creation fails (e.g. permissions on a locked-down
/// home directory).
#[tauri::command]
pub async fn ensure_user_skills_dir() -> Result<String, String> {
    let path = user_skills_primary_dir()
        .ok_or_else(|| "could not resolve user skills directory".to_string())?;
    tokio::task::spawn_blocking(move || {
        std::fs::create_dir_all(&path)
            .map_err(|e| format!("create user skills dir {}: {e}", path.display()))?;
        Ok(path.display().to_string())
    })
    .await
    .map_err(|e| format!("ensure_user_skills_dir join: {e}"))?
}

/// Scaffold a new skill folder from the bundled `_template/SKILL.md`.
/// The frontend "+ New skill" button calls this; on success the
/// returned path is the absolute location of the freshly written
/// `SKILL.md`, which the React side reveals + selects.
///
/// `target` selects the destination layer:
///   - `"user"`    → `user_skills_primary_dir()` (created if missing)
///   - `"project"` → `<project_root>/skills/`    (created if missing)
///
/// The template body has `{{name}}` and `{{description}}` placeholders
/// that we substitute literally before writing. The directory itself
/// is named after the skill (kebab-case enforced by the frontend).
///
/// Errors:
///   - unknown target value
///   - "project" requested without an active project
///   - bundled template missing (dev tree out of sync)
///   - skill already exists at the target path (no clobber)
///   - filesystem IO failures
#[tauri::command]
pub async fn create_skill(
    state: State<'_, MontageState>,
    target: String,
    name: String,
    description: String,
) -> Result<String, String> {
    let project_root = state.project_root.lock().await.clone();
    tokio::task::spawn_blocking(move || create_skill_impl(target, name, description, project_root))
        .await
        .map_err(|e| format!("create_skill join: {e}"))?
}

/// Synchronous worker for `create_skill` so the logic is testable
/// without standing up a Tauri runtime. The `resolver` closure picks
/// the destination + template paths — in production it dispatches to
/// `user_skills_primary_dir` / `bundled_skill_root`; tests inject
/// temp dirs.
fn create_skill_impl(
    target: String,
    name: String,
    description: String,
    project_root: Option<PathBuf>,
) -> Result<String, String> {
    let bundled =
        bundled_skill_root().ok_or_else(|| "bundled skills root not found".to_string())?;
    let user = user_skills_primary_dir();
    create_skill_at(
        &target,
        &name,
        &description,
        project_root.as_deref(),
        &bundled,
        user.as_deref(),
    )
}

/// Plain-paths variant of `create_skill_impl`. Pure FS — no globals
/// queried. Drives both production and tests.
fn create_skill_at(
    target: &str,
    name: &str,
    description: &str,
    project_root: Option<&Path>,
    bundled_root: &Path,
    user_root: Option<&Path>,
) -> Result<String, String> {
    let dest_root = match target {
        "user" => user_root
            .ok_or_else(|| "could not resolve user skills directory".to_string())?
            .to_path_buf(),
        "project" => {
            let root = project_root.ok_or_else(|| "no project is open".to_string())?;
            root.join("skills")
        }
        other => {
            return Err(format!(
                "unknown target '{other}' (expected 'user' or 'project')"
            ));
        }
    };
    std::fs::create_dir_all(&dest_root)
        .map_err(|e| format!("create {}: {e}", dest_root.display()))?;

    let skill_dir = dest_root.join(name);
    if skill_dir.exists() {
        return Err(format!(
            "Skill '{name}' already exists at {}",
            skill_dir.display()
        ));
    }

    let template_path = bundled_root.join("_template").join("SKILL.md");
    let template = std::fs::read_to_string(&template_path)
        .map_err(|e| format!("read template {}: {e}", template_path.display()))?;
    let rendered = template
        .replace("{{name}}", name)
        .replace("{{description}}", description);

    std::fs::create_dir_all(&skill_dir)
        .map_err(|e| format!("create {}: {e}", skill_dir.display()))?;
    let md_path = skill_dir.join("SKILL.md");
    std::fs::write(&md_path, rendered).map_err(|e| format!("write {}: {e}", md_path.display()))?;

    Ok(md_path.display().to_string())
}

/// Return the absolute path to the bundled skills authoring guide
/// (`docs/skills-authoring.md`) so the Skills tab footer can open it
/// in the system Markdown viewer / editor. The doc lives next to
/// `skills/` at the repo / install root; we reuse `bundled_skill_root`
/// to walk up to the same parent.
#[tauri::command]
pub async fn skills_authoring_guide_path() -> Result<String, String> {
    tokio::task::spawn_blocking(|| {
        let bundled =
            bundled_skill_root().ok_or_else(|| "bundled skills root not found".to_string())?;
        let candidate = bundled
            .parent()
            .map(|p| p.join("docs").join("skills-authoring.md"))
            .ok_or_else(|| "could not resolve docs root".to_string())?;
        if candidate.is_file() {
            Ok(candidate.display().to_string())
        } else {
            Err(format!(
                "authoring guide not found at {}",
                candidate.display()
            ))
        }
    })
    .await
    .map_err(|e| format!("skills_authoring_guide_path join: {e}"))?
}

/// Mirror of `codex_session::bundled_skill_root`. In `cargo tauri
/// dev` the binary lives at `<repo>/target/debug/montage-desktop`, so
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Stage a minimal bundled root with the `_template/SKILL.md`
    /// the scaffolder reads. Mirrors the real placeholders.
    fn stage_bundled(root: &Path) {
        let tpl = root.join("_template");
        std::fs::create_dir_all(&tpl).unwrap();
        std::fs::write(
            tpl.join("SKILL.md"),
            "---\nname: {{name}}\ndescription: {{description}}\n---\n\n# {{name}}\n",
        )
        .unwrap();
    }

    #[test]
    fn create_skill_writes_user_layer_file() {
        let tmp = tempfile::tempdir().unwrap();
        let bundled = tmp.path().join("bundled");
        let user = tmp.path().join("user");
        std::fs::create_dir_all(&user).unwrap();
        stage_bundled(&bundled);

        let path = create_skill_at(
            "user",
            "my-new-skill",
            "Does a thing.",
            None,
            &bundled,
            Some(&user),
        )
        .expect("create_skill_at succeeds");
        assert!(path.ends_with("my-new-skill/SKILL.md"));

        let body = std::fs::read_to_string(&path).unwrap();
        assert!(body.contains("name: my-new-skill"));
        assert!(body.contains("description: Does a thing."));
        assert!(
            !body.contains("{{name}}"),
            "placeholders must be substituted"
        );
    }

    #[test]
    fn create_skill_writes_project_layer_file() {
        let tmp = tempfile::tempdir().unwrap();
        let bundled = tmp.path().join("bundled");
        let project = tmp.path().join("proj");
        std::fs::create_dir_all(&project).unwrap();
        stage_bundled(&bundled);

        let path = create_skill_at(
            "project",
            "proj-skill",
            "Project-scoped.",
            Some(&project),
            &bundled,
            None,
        )
        .expect("project target succeeds");
        assert!(path.starts_with(project.join("skills").to_string_lossy().as_ref()));
        assert!(std::fs::metadata(&path).is_ok());
    }

    #[test]
    fn create_skill_rejects_duplicate_name() {
        let tmp = tempfile::tempdir().unwrap();
        let bundled = tmp.path().join("bundled");
        let user = tmp.path().join("user");
        std::fs::create_dir_all(&user).unwrap();
        stage_bundled(&bundled);

        create_skill_at("user", "dup", "first", None, &bundled, Some(&user))
            .expect("first call writes");
        let err = create_skill_at("user", "dup", "second", None, &bundled, Some(&user))
            .expect_err("second call must fail");
        assert!(err.contains("already exists"), "unexpected error: {err}");
    }

    #[test]
    fn create_skill_rejects_project_target_without_project_root() {
        let tmp = tempfile::tempdir().unwrap();
        let bundled = tmp.path().join("bundled");
        stage_bundled(&bundled);

        let err = create_skill_at("project", "x", "d", None, &bundled, None)
            .expect_err("no project must fail");
        assert!(err.contains("no project is open"), "got: {err}");
    }

    #[test]
    fn create_skill_rejects_unknown_target() {
        let tmp = tempfile::tempdir().unwrap();
        let bundled = tmp.path().join("bundled");
        stage_bundled(&bundled);

        let err = create_skill_at("bogus", "x", "d", None, &bundled, None)
            .expect_err("unknown target must fail");
        assert!(err.contains("unknown target"), "got: {err}");
    }
}
