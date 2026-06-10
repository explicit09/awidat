//! Tauri-side glue around [`montage_codex_bridge::CodexAppServer`].
//!
//! - [`TauriEmitter`] adapts Tauri's [`AppHandle`] to the bridge's
//!   [`ItemEmitter`] trait so the bridge can push `Item`s onto our
//!   Tauri event bus without depending on Tauri itself.
//! - [`CodexSession`] wraps the live bridge with the `project_root` it
//!   was launched against; the desktop tears it down + relaunches on
//!   project switch (see [`crate::commands::project`]).
//!
//! The MCP server sibling-binary lookup mirrors
//! `crates/cli/src/chat_codex_cmd.rs::montage_mcp_overrides` (sibling
//! of `current_exe()`'s parent, named `montage-mcp-server`). Unlike
//! the CLI, we can't assume `current_exe()` is `montage` — in a
//! packaged Tauri build it's the app bundle binary. The MCP server
//! still has to live next to it for `cargo tauri dev` to find it.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use montage_codex_bridge::{BridgeError, CodexAppServer, ItemEmitter};
use montage_desktop_protocol::Item;
use tauri::{AppHandle, Manager};

use crate::events::{emit_item, emit_timeline_changed, emit_turn_end};

/// Bridges the codex-bridge's pure-Rust [`ItemEmitter`] trait onto
/// Tauri's `AppHandle::emit`. Clone-cheap (`AppHandle` is `Clone`).
///
/// `project_root` is bound at construction so `emit_timeline_changed`
/// can identify which project the React side should refetch — the
/// bridge raises that signal without knowing the path itself.
pub struct TauriEmitter {
    app: AppHandle,
    project_root: PathBuf,
}

impl TauriEmitter {
    pub fn new(app: AppHandle, project_root: PathBuf) -> Self {
        Self { app, project_root }
    }
}

impl ItemEmitter for TauriEmitter {
    fn emit_item(&self, item: Item) {
        emit_item(&self.app, item);
    }

    fn emit_turn_end(&self, turn_id: String, error: Option<String>) {
        let app = self.app.clone();
        tauri::async_runtime::spawn(async move {
            let state = app.state::<crate::state::MontageState>();
            let mut guard = state.turn.lock().await;
            if guard.as_ref().map(|turn| turn.id.as_str()) == Some(turn_id.as_str()) {
                guard.take();
            }
            drop(guard);
            emit_turn_end(&app, turn_id, error);
        });
    }

    fn emit_timeline_changed(&self) {
        emit_timeline_changed(&self.app, &self.project_root);
    }
}

/// Live bridge + the project it was launched against. Stored in
/// [`crate::state::MontageState`] inside an `Option`; `None` means no
/// project is open or the previous session was torn down.
pub struct CodexSession {
    pub bridge: CodexAppServer,
    /// Absolute project path the bridge was constructed with. Used to
    /// decide whether `start_turn` can re-use the session or must tear
    /// it down and rebuild (which happens after a project switch).
    pub project_root: PathBuf,
}

impl CodexSession {
    /// Launch a fresh bridge for `project_root`. Caller must have
    /// already verified there isn't an existing session for this
    /// project (otherwise we leak the previous one).
    ///
    /// When `resume_thread_id` is `Some`, the bridge re-attaches to an
    /// existing codex thread (rollout-backed history) instead of
    /// starting a fresh one.
    pub async fn launch(
        app: AppHandle,
        project_root: PathBuf,
        resume_thread_id: Option<String>,
    ) -> Result<Self, BridgeError> {
        let mcp_server_path = resolve_mcp_server_binary();
        // Loud failure on the user-facing event bus when the sibling
        // binary is missing — silently falling back to "codex with no
        // Montage tools" produces an agent that runs shell commands
        // instead of view_timeline / apply_edl. That mode is unhelpful
        // for editing; surface it before the user wastes a turn on it.
        if mcp_server_path.is_none() {
            let warning = "montage-mcp-server binary missing next to montage-desktop. \
                The agent will fall back to shell-only and won't use Montage tools \
                (view_timeline, apply_edl, etc.). Reinstall Montage or report this issue.";
            tracing::error!("{warning}");
            crate::events::emit_item(
                &app,
                montage_desktop_protocol::Item::Error {
                    id: montage_desktop_protocol::Id::new("montage-mcp-missing"),
                    message: warning.to_string(),
                },
            );
        }
        let emitter: Arc<dyn ItemEmitter> = Arc::new(TauriEmitter::new(app, project_root.clone()));
        // Per-format editorial addendum (Podcast / Shorts / Tutorial /
        // Other). Reads project type from the OTIO and assembles the
        // matching playbook; rides on `developer_instructions` so the
        // agent gets it without us touching codex's base prompt.
        let developer_instructions = Some(montage_core::system_prompt::assemble_for_project(
            &project_root,
        ));
        // Progressive-disclosure skills catalog. L1 (name + description)
        // rides on session developer instructions; the agent calls
        // `load_skill(name='...')` for the L2 body. User-installed
        // skills under ~/Library/Application Support/montage/skills and
        // ~/.config/montage/skills override bundled. See
        // crates/core/src/skills.rs for the discovery rules.
        //
        // The catalog is filtered against `<project>/.montage/skills.json`
        // so skills the editor has explicitly disabled in the Skills tab
        // never reach the agent's loadout. Missing/malformed file means
        // "load everything" — see `commands::skill_config` for the
        // schema and fail-mode rules.
        let skills_catalog = render_skills_catalog(&project_root);
        // Recent rejection feedback (Wave 5 C3). Reads the last
        // MAX_FEEDBACK_ENTRIES entries from `.montage/feedback.jsonl` and
        // renders the `<recent_feedback>` L1 fragment. Missing /
        // empty log → None, in which case the bridge skips the section
        // entirely instead of emitting an empty header.
        let recent_feedback = render_recent_feedback(&project_root);
        let bridge = CodexAppServer::launch(
            emitter,
            project_root.clone(),
            mcp_server_path,
            developer_instructions,
            skills_catalog,
            recent_feedback,
            resume_thread_id,
        )
        .await?;
        Ok(Self {
            bridge,
            project_root,
        })
    }
}

/// Discover installed skills and render the L1 catalog as a
/// contextual fragment ready to prepend to a turn input. Returns
/// `None` if no skills are installed (then nothing gets prepended).
///
/// Discovery hierarchy from `montage_core::skills` (lowest priority
/// first; later layers override earlier by skill name, replacing the
/// entry entirely — not a field-by-field merge):
///   1. bundled — `<repo>/skills` in dev; in a packaged build,
///      `<install>/share/montage/skills`. We pick the repo-relative
///      `skills/` dir via the running binary's grandparent (works in
///      `cargo tauri dev`; packaged builds will need a separate
///      resolver later when we ship installers).
///   2. user roots — legacy Awidat skill folders, then
///      `~/.montage/skills`, then platform config skill folders.
///   3. project — `<project>/skills/` — per-project overrides win
///      over both user and bundled. Lets a project ship its own
///      editorial loadout.
///
/// Skills listed in `<project>/.montage/skills.json` under `disabled`
/// are removed from the catalog before rendering — the agent never
/// learns they exist. Missing or malformed config = "nothing disabled".
///
/// Non-fatal: errors during discovery (malformed SKILL.md, etc.) are
/// logged via `tracing::warn!` and dropped; the agent gets whatever
/// loaded successfully.
fn render_skills_catalog(project_root: &Path) -> Option<String> {
    let user_roots = user_skill_roots();
    let bundled = bundled_skill_root();
    let project_override = project_skill_root(project_root);
    let cfg = crate::commands::skill_config::load_skill_config_sync(project_root);
    render_skills_catalog_from_roots(
        bundled.as_deref(),
        &user_roots,
        project_override.as_deref(),
        &cfg.disabled,
        &cfg.pinned,
    )
}

/// Current Montage core version — used to gate skills that declare a
/// minimum compatibility version via `SkillMeta::montage_min_version`.
/// Sourced from the desktop crate's package version at compile time.
/// Skills declaring a higher minimum are skipped with a warning.
const MONTAGE_CORE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Discover the most recent `<= MAX_FEEDBACK_ENTRIES` rejection rows
/// and render the L1 fragment ready to prepend to a turn input.
/// Returns `None` when the log is missing / empty — the bridge then
/// skips the section entirely instead of emitting an empty header.
///
/// Wave 5 C3 — see `crates/core/src/context.rs::RecentFeedbackFragment`
/// for the on-the-wire format. The read side mirrors
/// `commands::skill_config::load_skill_config_sync`: sync read at
/// session-launch time so we don't have to plumb async through the
/// bridge constructor.
fn render_recent_feedback(project_root: &Path) -> Option<String> {
    let entries = crate::commands::feedback::load_recent_feedback_sync(
        project_root,
        montage_core::context::MAX_FEEDBACK_ENTRIES,
    );
    render_recent_feedback_from_entries(&entries)
}

/// Inner core — separated so unit tests can drive rendering off a
/// hand-built entry list without standing up a JSONL file. Empty input
/// → `None` so the caller can skip the section entirely.
fn render_recent_feedback_from_entries(
    entries: &[crate::commands::feedback::FeedbackEntry],
) -> Option<String> {
    use montage_core::context::{ContextualUserFragment, RecentFeedbackFragment};
    if entries.is_empty() {
        return None;
    }
    let lines: Vec<String> = entries.iter().map(format_feedback_line).collect();
    Some(RecentFeedbackFragment { lines }.render())
}

/// Format one rejection row as a bullet line for the L1 fragment.
///
/// Shape:
///   - `<medium> "<title>" — reason: <reason>` when the user supplied
///     a reason.
///   - `<medium> "<title>" (no reason given)` when the user rejected
///     silently — we still surface frequency so the agent can spot
///     "the user just turned down three cuts in a row".
fn format_feedback_line(entry: &crate::commands::feedback::FeedbackEntry) -> String {
    match entry.reason.as_deref() {
        Some(reason) if !reason.trim().is_empty() => format!(
            "  - {} \"{}\" — reason: {}",
            entry.medium,
            entry.title,
            reason.trim()
        ),
        _ => format!("  - {} \"{}\" (no reason given)", entry.medium, entry.title),
    }
}

/// Inner core — separated so unit tests can drive discovery against
/// a `tempfile::TempDir`-backed skills root without touching the
/// per-user paths or the running binary's grandparent.
fn render_skills_catalog_from_roots(
    bundled_root: Option<&Path>,
    user_roots: &[PathBuf],
    project_root: Option<&Path>,
    disabled: &[String],
    pinned: &[crate::commands::skill_config::PinnedSkill],
) -> Option<String> {
    use montage_core::context::{AvailableSkillsFragment, ContextualUserFragment};

    // Build a name → resolved (skill, provenance) map by walking each
    // layer ourselves. We can't use `SkillRegistry::discover_many`
    // here because pin resolution needs to choose between layers per
    // skill (a `provenance: "bundled"` pin must reach back past a
    // project override), and the registry's overlay model collapses
    // that information.
    let layers = collect_layered_skills(bundled_root, user_roots, project_root);

    let disabled_set: std::collections::HashSet<&str> =
        disabled.iter().map(String::as_str).collect();
    let pin_table: std::collections::HashMap<&str, &crate::commands::skill_config::PinnedSkill> =
        pinned.iter().map(|p| (p.name.as_str(), p)).collect();

    // Resolve each skill to its winning layer. Names are collected
    // from every layer; the loop picks the right `Skill` per name.
    let mut names: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    for (name, _) in layers.bundled.iter() {
        names.insert(name.as_str());
    }
    for (name, _) in layers.user.iter() {
        names.insert(name.as_str());
    }
    for (name, _) in layers.project.iter() {
        names.insert(name.as_str());
    }

    let mut skill_lines: Vec<String> = Vec::new();
    for name in names {
        // Disabled wins over every pin. The CLAUDE.md / module doc
        // contract is explicit: pinning never re-enables a disabled
        // skill.
        if disabled_set.contains(name) {
            continue;
        }
        let resolved = match pin_table.get(name) {
            Some(pin) => resolve_pinned(name, pin, &layers),
            None => resolve_default(name, &layers),
        };
        let Some(skill) = resolved else { continue };
        // Compatibility gate. A skill that needs a newer Montage than
        // we're running is silently skipped with a warning; the
        // editor sees a missing entry on the Skills tab card (handled
        // there) and the agent never learns it existed.
        if !is_compatible(&skill.meta.montage_min_version) {
            tracing::warn!(
                skill = %name,
                required = ?skill.meta.montage_min_version,
                current = %MONTAGE_CORE_VERSION,
                "skipping skill: requires newer Montage core"
            );
            continue;
        }
        skill_lines.push(format!(
            "  - {}: {}",
            skill.meta.name, skill.meta.description
        ));
    }

    if skill_lines.is_empty() {
        return None;
    }
    Some(AvailableSkillsFragment { skill_lines }.render())
}

/// Internal: per-layer `name → Skill` index. Bundled-first ordering
/// matches the discovery hierarchy. Used by `render_skills_catalog_from_roots`
/// for per-skill layer selection during pin resolution.
struct LayeredSkills {
    bundled: std::collections::BTreeMap<String, montage_core::skills::Skill>,
    user: std::collections::BTreeMap<String, montage_core::skills::Skill>,
    project: std::collections::BTreeMap<String, montage_core::skills::Skill>,
}

fn collect_layered_skills(
    bundled_root: Option<&Path>,
    user_roots: &[PathBuf],
    project_root: Option<&Path>,
) -> LayeredSkills {
    use montage_core::skills::SkillRegistry;
    let load =
        |root: Option<&Path>| -> std::collections::BTreeMap<String, montage_core::skills::Skill> {
            let mut out = std::collections::BTreeMap::new();
            let Some(root) = root else {
                return out;
            };
            let (reg, errors) =
                SkillRegistry::discover_many(Some(root), std::iter::empty::<&Path>());
            for err in errors {
                tracing::warn!(?err, "skill discovery: malformed entry skipped");
            }
            for s in reg.all() {
                out.insert(s.meta.name.clone(), s.clone());
            }
            out
        };

    // User roots may be multiple paths (legacy `~/.montage/skills` +
    // platform config dir) — later wins on name conflict, mirroring
    // the previous overlay behavior.
    let mut user_map = std::collections::BTreeMap::new();
    for root in user_roots {
        let layer = load(Some(root.as_path()));
        user_map.extend(layer);
    }

    LayeredSkills {
        bundled: load(bundled_root),
        user: user_map,
        project: load(project_root),
    }
}

/// Default resolution — project > user > bundled. Returns `None` when
/// none of the layers carry the named skill (shouldn't happen since
/// we iterate names sourced from these same layers, but keeps the
/// signature honest).
fn resolve_default<'a>(
    name: &str,
    layers: &'a LayeredSkills,
) -> Option<&'a montage_core::skills::Skill> {
    layers
        .project
        .get(name)
        .or_else(|| layers.user.get(name))
        .or_else(|| layers.bundled.get(name))
}

/// Pin resolution — honors `version` (exact match) and `provenance`
/// (constrain to a single layer). Returns `None` when no layer
/// satisfies the pin, emitting a `tracing::warn!` so the editor can
/// trace why a pinned skill went missing.
fn resolve_pinned<'a>(
    name: &str,
    pin: &crate::commands::skill_config::PinnedSkill,
    layers: &'a LayeredSkills,
) -> Option<&'a montage_core::skills::Skill> {
    // Candidate list filtered by provenance.
    let candidates: Vec<(&str, Option<&montage_core::skills::Skill>)> =
        match pin.provenance.as_deref() {
            Some("bundled") => vec![("bundled", layers.bundled.get(name))],
            Some("user") => vec![("user", layers.user.get(name))],
            Some("project") => vec![("project", layers.project.get(name))],
            Some(other) => {
                tracing::warn!(
                    skill = %name,
                    provenance = %other,
                    "skill pin: unknown provenance value; skipping skill"
                );
                return None;
            }
            None => vec![
                ("project", layers.project.get(name)),
                ("user", layers.user.get(name)),
                ("bundled", layers.bundled.get(name)),
            ],
        };

    // Then filter by version if the pin sets it. The first candidate
    // (highest priority in default ordering) that matches wins.
    for (_layer_label, candidate) in candidates {
        let Some(skill) = candidate else { continue };
        if let Some(required) = pin.version.as_deref() {
            if skill.meta.version != required {
                continue;
            }
        }
        return Some(skill);
    }

    tracing::warn!(
        skill = %name,
        required_version = ?pin.version,
        required_provenance = ?pin.provenance,
        "skill pin: no candidate satisfies the pin; skipping skill"
    );
    None
}

/// Compatibility check — returns `true` when the running Montage is
/// at or above the skill's declared `montage_min_version`. Skills
/// without a declared minimum always pass.
///
/// Versions are normalized to three `u32` components for an ordered
/// comparison; non-numeric or short versions fall back to a lenient
/// "assume compatible" so a malformed string can't lock out an
/// otherwise-loadable skill.
fn is_compatible(min_version: &Option<String>) -> bool {
    let Some(required) = min_version.as_deref() else {
        return true;
    };
    let current = parse_three_part(MONTAGE_CORE_VERSION);
    let needed = parse_three_part(required);
    match (current, needed) {
        (Some(c), Some(n)) => c >= n,
        _ => true,
    }
}

/// Parse `"MAJOR.MINOR.PATCH"` → `(u32, u32, u32)`. Returns `None` on
/// any deviation so the caller can fall back to a lenient default.
fn parse_three_part(s: &str) -> Option<(u32, u32, u32)> {
    let mut parts = s.split('.');
    let major = parts.next()?.parse::<u32>().ok()?;
    let minor = parts.next()?.parse::<u32>().ok()?;
    // Patch may carry a pre-release / build suffix (e.g. "0-alpha");
    // strip everything past the first non-digit so "1.2.0-beta" parses
    // as `1.2.0` for the compatibility check.
    let patch_raw = parts.next()?;
    let patch_digits: String = patch_raw
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    let patch = patch_digits.parse::<u32>().ok()?;
    Some((major, minor, patch))
}

/// Per-user skills directories, in priority order (later entries win
/// on name conflicts). Two layers:
///   - `dirs::config_dir()` joined with `montage/skills` — the platform
///     idiomatic location:
///     - macOS   → `~/Library/Application Support/montage/skills`
///     - Linux   → `~/.config/montage/skills`
///     - Windows → `%APPDATA%\montage\skills`
///   - home-dir roots: legacy `~/.awidat/skills` followed by
///     `~/.montage/skills`.
fn user_skill_roots() -> Vec<PathBuf> {
    user_skill_roots_from(dirs::home_dir(), dirs::config_dir())
}

fn user_skill_roots_from(home_dir: Option<PathBuf>, config_dir: Option<PathBuf>) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(home) = home_dir {
        roots.push(home.join(".awidat/skills"));
        roots.push(home.join(".montage/skills"));
    }
    if let Some(cfg) = config_dir {
        roots.push(cfg.join("awidat/skills"));
        roots.push(cfg.join("montage/skills"));
    }
    roots
}

/// Per-project skills directory. A project can drop a `skills/` folder
/// at its root to override or add editorial workflows. Returns `None`
/// when no such directory exists.
fn project_skill_root(project_root: &Path) -> Option<PathBuf> {
    let candidate = project_root.join("skills");
    if candidate.is_dir() {
        Some(candidate)
    } else {
        None
    }
}

/// Best-effort bundled-skills root. In `cargo tauri dev` the binary
/// lives at `<repo>/target/debug/montage-desktop`, so the skills dir
/// is `<repo>/skills`. Walk up three dirs from the binary path.
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

/// Resolve the sibling `montage-mcp-server` binary path so the bridge
/// can inject it as a `mcp_servers.montage.command` config override.
///
/// `None` means we couldn't find it; the bridge then runs codex
/// without our MCP tools (matching the pre-step-3 behavior).
fn resolve_mcp_server_binary() -> Option<PathBuf> {
    let self_exe = std::env::current_exe().ok()?;
    resolve_mcp_server_binary_from_exe(&self_exe)
}

fn resolve_mcp_server_binary_from_exe(self_exe: &Path) -> Option<PathBuf> {
    let parent = self_exe.parent()?;
    for file_name in ["montage-mcp-server", "montage-mcp-server.exe"] {
        let candidate = parent.join(file_name);
        if candidate.exists() {
            return Some(candidate);
        }
    }
    tracing::warn!(
        path = %parent.display(),
        "montage-mcp-server sibling binary missing; agent will run without Montage tools"
    );
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Build a throwaway skills root containing a fixed set of
    /// minimal SKILL.md files. Used to exercise the catalog filter
    /// without depending on the bundled `skills/` dir layout.
    fn make_skills_root(names: &[&str]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        for name in names {
            let sd = dir.path().join(name);
            fs::create_dir_all(&sd).unwrap();
            let body = format!(
                "---\nname: {name}\ndescription: desc for {name}\n---\n\nbody\n",
                name = name
            );
            fs::write(sd.join("SKILL.md"), body).unwrap();
        }
        dir
    }

    #[test]
    fn render_skills_catalog_includes_all_when_nothing_disabled() {
        let bundled = make_skills_root(&["alpha-skill", "beta-skill"]);
        let rendered = render_skills_catalog_from_roots(Some(bundled.path()), &[], None, &[], &[])
            .expect("catalog non-empty");
        assert!(
            rendered.contains("alpha-skill"),
            "missing alpha: {rendered}"
        );
        assert!(rendered.contains("beta-skill"), "missing beta: {rendered}");
    }

    #[test]
    fn render_skills_catalog_filters_disabled_skill() {
        let bundled = make_skills_root(&["alpha-skill", "beta-skill"]);
        let rendered = render_skills_catalog_from_roots(
            Some(bundled.path()),
            &[],
            None,
            &["alpha-skill".to_string()],
            &[],
        )
        .expect("catalog non-empty");
        assert!(
            !rendered.contains("alpha-skill"),
            "alpha-skill should have been filtered: {rendered}"
        );
        assert!(
            rendered.contains("beta-skill"),
            "beta still expected: {rendered}"
        );
    }

    #[test]
    fn resolve_mcp_server_binary_from_exe_accepts_windows_sidecar_name() {
        let dir = tempfile::tempdir().unwrap();
        let mcp = dir.path().join("montage-mcp-server.exe");
        fs::write(&mcp, b"").unwrap();

        let resolved = resolve_mcp_server_binary_from_exe(&dir.path().join("montage-desktop.exe"));

        assert_eq!(resolved.as_deref(), Some(mcp.as_path()));
    }

    #[test]
    fn render_skills_catalog_returns_none_when_all_disabled() {
        let bundled = make_skills_root(&["only-skill"]);
        let out = render_skills_catalog_from_roots(
            Some(bundled.path()),
            &[],
            None,
            &["only-skill".to_string()],
            &[],
        );
        assert!(out.is_none(), "expected None when every skill is disabled");
    }

    #[test]
    fn render_skills_catalog_ignores_unknown_disabled_names() {
        // A stale entry in `.montage/skills.json` (e.g. for a skill that
        // got removed) must not break the catalog for the survivors.
        let bundled = make_skills_root(&["alpha-skill"]);
        let rendered = render_skills_catalog_from_roots(
            Some(bundled.path()),
            &[],
            None,
            &["ghost-skill".to_string()],
            &[],
        )
        .expect("catalog non-empty");
        assert!(rendered.contains("alpha-skill"));
    }

    #[test]
    fn render_skills_catalog_returns_none_when_no_skills_installed() {
        let empty = tempfile::tempdir().unwrap();
        let out = render_skills_catalog_from_roots(Some(empty.path()), &[], None, &[], &[]);
        assert!(out.is_none());
    }

    /// Wave 4 W4.2 — the rendered L1 catalog the bridge prepends to
    /// every turn must carry the rationale-contract paragraph that
    /// teaches the agent to populate `Item::ProposedEdit::rationale`.
    /// Lives once in `AvailableSkillsFragment` so it appears on every
    /// session — even when only one skill is installed.
    #[test]
    fn render_skills_catalog_carries_rationale_contract() {
        let bundled = make_skills_root(&["only-skill"]);
        let rendered = render_skills_catalog_from_roots(Some(bundled.path()), &[], None, &[], &[])
            .expect("catalog non-empty");
        assert!(
            rendered.contains("## Rationale contract"),
            "rendered catalog must include the rationale contract; got:\n{rendered}"
        );
        assert!(
            rendered.contains("Every proposal you emit MUST include"),
            "rendered catalog must include the MUST-rule; got:\n{rendered}"
        );
    }

    /// Build a skills root where a single named skill embeds the
    /// supplied description, so the priority test can read back the
    /// rendered catalog and tell which layer won.
    fn make_skills_root_with_desc(name: &str, description: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        let sd = dir.path().join(name);
        fs::create_dir_all(&sd).unwrap();
        let body = format!(
            "---\nname: {name}\ndescription: {description}\n---\n\nbody\n",
            name = name,
            description = description
        );
        fs::write(sd.join("SKILL.md"), body).unwrap();
        dir
    }

    /// Wave 5 B1 — priority must be project > user > bundled. The
    /// rendered L1 line for an identically-named skill should carry the
    /// description from the highest-priority layer present.
    #[test]
    fn render_skills_catalog_project_layer_overrides_user_and_bundled() {
        let bundled = make_skills_root_with_desc("shared-skill", "bundled-desc");
        let user = make_skills_root_with_desc("shared-skill", "user-desc");
        let project = make_skills_root_with_desc("shared-skill", "project-desc");
        let rendered = render_skills_catalog_from_roots(
            Some(bundled.path()),
            &[user.path().to_path_buf()],
            Some(project.path()),
            &[],
            &[],
        )
        .expect("catalog non-empty");
        assert!(
            rendered.contains("project-desc"),
            "project must override user+bundled; got:\n{rendered}"
        );
        assert!(
            !rendered.contains("user-desc"),
            "user description must be replaced; got:\n{rendered}"
        );
        assert!(
            !rendered.contains("bundled-desc"),
            "bundled description must be replaced; got:\n{rendered}"
        );
    }

    /// Wave 5 B1 — user layer wins over bundled when no project override
    /// is supplied. Catches a regression where overlay roots get
    /// reordered.
    #[test]
    fn render_skills_catalog_user_layer_overrides_bundled() {
        let bundled = make_skills_root_with_desc("shared-skill", "bundled-desc");
        let user = make_skills_root_with_desc("shared-skill", "user-desc");
        let rendered = render_skills_catalog_from_roots(
            Some(bundled.path()),
            &[user.path().to_path_buf()],
            None,
            &[],
            &[],
        )
        .expect("catalog non-empty");
        assert!(rendered.contains("user-desc"), "user must override bundled");
        assert!(!rendered.contains("bundled-desc"));
    }

    #[test]
    fn user_skill_roots_include_legacy_awidat_locations_before_montage() {
        let roots = user_skill_roots_from(
            Some(PathBuf::from("/home/user")),
            Some(PathBuf::from("/config")),
        );

        assert_eq!(
            roots,
            vec![
                PathBuf::from("/home/user/.awidat/skills"),
                PathBuf::from("/home/user/.montage/skills"),
                PathBuf::from("/config/awidat/skills"),
                PathBuf::from("/config/montage/skills"),
            ]
        );
    }

    // ---- Wave 5 B2 — version + provenance pinning ----

    /// Build a throwaway skills root with a single skill at a chosen
    /// version + description. Lets pin tests differentiate two copies
    /// of the same skill by reading the rendered description back.
    fn make_skills_root_with_version(
        name: &str,
        version: &str,
        description: &str,
    ) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        let sd = dir.path().join(name);
        fs::create_dir_all(&sd).unwrap();
        let body = format!(
            "---\nname: {name}\ndescription: {description}\nversion: \"{version}\"\n---\n\nbody\n"
        );
        fs::write(sd.join("SKILL.md"), body).unwrap();
        dir
    }

    use crate::commands::skill_config::PinnedSkill;

    /// Pinning to v1.0.0 must skip a candidate at v1.1.0 even if it's
    /// the higher-priority layer. The bundled v1.0.0 wins.
    #[test]
    fn render_skills_catalog_version_pin_selects_matching_layer() {
        let bundled = make_skills_root_with_version("auto-cutter", "1.0.0", "v1-zero");
        // A project-layer copy at a different version that the pin
        // refuses to load — the bundled v1.0.0 must take its place.
        let project = make_skills_root_with_version("auto-cutter", "1.1.0", "v1-one");
        let pins = vec![PinnedSkill {
            name: "auto-cutter".to_string(),
            version: Some("1.0.0".to_string()),
            provenance: None,
        }];
        let rendered = render_skills_catalog_from_roots(
            Some(bundled.path()),
            &[],
            Some(project.path()),
            &[],
            &pins,
        )
        .expect("catalog non-empty");
        assert!(
            rendered.contains("v1-zero"),
            "pinned version should be selected; got:\n{rendered}"
        );
        assert!(
            !rendered.contains("v1-one"),
            "non-matching version must be skipped; got:\n{rendered}"
        );
    }

    /// Pinning to a non-existent version must skip the skill entirely
    /// (warn + drop). Other skills carry on.
    #[test]
    fn render_skills_catalog_unsatisfiable_version_pin_drops_skill() {
        let bundled = make_skills_root_with_version("auto-cutter", "1.1.0", "actual");
        let pins = vec![PinnedSkill {
            name: "auto-cutter".to_string(),
            version: Some("1.0.0".to_string()),
            provenance: None,
        }];
        // Only this skill exists; an unsatisfiable pin → empty catalog.
        let out = render_skills_catalog_from_roots(Some(bundled.path()), &[], None, &[], &pins);
        assert!(
            out.is_none(),
            "unsatisfiable pin should drop the only skill"
        );
    }

    /// Pinning to a specific provenance must select that layer even
    /// when a higher-priority layer carries the same skill.
    #[test]
    fn render_skills_catalog_provenance_pin_selects_bundled_over_project() {
        let bundled = make_skills_root_with_desc("shared", "bundled-desc");
        let project = make_skills_root_with_desc("shared", "project-desc");
        let pins = vec![PinnedSkill {
            name: "shared".to_string(),
            version: None,
            provenance: Some("bundled".to_string()),
        }];
        let rendered = render_skills_catalog_from_roots(
            Some(bundled.path()),
            &[],
            Some(project.path()),
            &[],
            &pins,
        )
        .expect("catalog non-empty");
        assert!(
            rendered.contains("bundled-desc"),
            "provenance pin to bundled must override project override; got:\n{rendered}"
        );
        assert!(
            !rendered.contains("project-desc"),
            "project layer must not leak through; got:\n{rendered}"
        );
    }

    /// Disabled list still wins over pins — pinning must not
    /// re-enable a disabled skill (per CLAUDE.md / module contract).
    #[test]
    fn render_skills_catalog_disabled_wins_over_pin() {
        let bundled = make_skills_root_with_version("auto-cutter", "1.0.0", "actual");
        let pins = vec![PinnedSkill {
            name: "auto-cutter".to_string(),
            version: Some("1.0.0".to_string()),
            provenance: None,
        }];
        let out = render_skills_catalog_from_roots(
            Some(bundled.path()),
            &[],
            None,
            &["auto-cutter".to_string()],
            &pins,
        );
        assert!(
            out.is_none(),
            "disabled skill must stay out even when pinned"
        );
    }

    /// Pinning to an unknown provenance value is treated as a
    /// misconfiguration: warn + skip the skill. We don't crash.
    #[test]
    fn render_skills_catalog_unknown_provenance_drops_skill() {
        let bundled = make_skills_root_with_desc("only-skill", "actual");
        let pins = vec![PinnedSkill {
            name: "only-skill".to_string(),
            version: None,
            provenance: Some("homebrew".to_string()),
        }];
        let out = render_skills_catalog_from_roots(Some(bundled.path()), &[], None, &[], &pins);
        assert!(out.is_none(), "unknown provenance must skip the skill");
    }

    /// Compatibility gate — a skill requiring a future Montage is
    /// silently dropped with a warning. Since `MONTAGE_CORE_VERSION`
    /// is sourced from the package's cargo version we can't pick a
    /// universally "newer" constant without coupling to it, so we
    /// use a 99.x.x sentinel that's safely in the future.
    #[test]
    fn render_skills_catalog_skips_future_required_skills() {
        let dir = tempfile::tempdir().unwrap();
        let sd = dir.path().join("future-skill");
        fs::create_dir_all(&sd).unwrap();
        fs::write(
            sd.join("SKILL.md"),
            "---\nname: future-skill\ndescription: needs newer Montage\nmontage_min_version: \"99.0.0\"\n---\n\nbody\n",
        )
        .unwrap();
        let out = render_skills_catalog_from_roots(Some(dir.path()), &[], None, &[], &[]);
        assert!(
            out.is_none(),
            "future-version skill must be filtered from the catalog"
        );
    }

    /// Compatibility gate — a skill declaring a min version we
    /// already meet must load normally.
    #[test]
    fn render_skills_catalog_loads_compatible_skill() {
        let dir = tempfile::tempdir().unwrap();
        let sd = dir.path().join("safe-skill");
        fs::create_dir_all(&sd).unwrap();
        fs::write(
            sd.join("SKILL.md"),
            "---\nname: safe-skill\ndescription: compat\nmontage_min_version: \"0.0.0\"\n---\n\nbody\n",
        )
        .unwrap();
        let rendered = render_skills_catalog_from_roots(Some(dir.path()), &[], None, &[], &[])
            .expect("catalog non-empty");
        assert!(rendered.contains("safe-skill"));
    }

    /// `parse_three_part` accepts MAJOR.MINOR.PATCH, ignores a build
    /// suffix on the patch component, and rejects malformed input.
    #[test]
    fn parse_three_part_handles_suffix_and_bad_input() {
        assert_eq!(parse_three_part("1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_three_part("1.2.3-beta"), Some((1, 2, 3)));
        assert_eq!(parse_three_part("not-a-version"), None);
        assert_eq!(parse_three_part("1.2"), None);
    }

    /// `is_compatible` is lenient when versions don't parse — a
    /// malformed declared minimum can't lock the editor out of a
    /// skill they otherwise have installed.
    #[test]
    fn is_compatible_lenient_when_versions_malformed() {
        assert!(is_compatible(&None));
        assert!(is_compatible(&Some("not-a-version".to_string())));
    }

    // ---- Wave 5 C3 — recent rejection feedback fragment ----

    use crate::commands::feedback::FeedbackEntry;

    fn feedback_entry(ts: i64, medium: &str, title: &str, reason: Option<&str>) -> FeedbackEntry {
        FeedbackEntry {
            ts,
            medium: medium.into(),
            title: title.into(),
            rationale: None,
            reason: reason.map(str::to_string),
        }
    }

    /// Empty entry list → caller skips the section entirely. We don't
    /// emit an empty `<recent_feedback>` block.
    #[test]
    fn render_recent_feedback_returns_none_when_no_entries() {
        let out = render_recent_feedback_from_entries(&[]);
        assert!(
            out.is_none(),
            "empty feedback must skip the section, got: {out:?}"
        );
    }

    /// One entry with a reason renders the `— reason: <text>` shape;
    /// the marker tags are present so compaction can find + strip the
    /// fragment later.
    #[test]
    fn render_recent_feedback_renders_with_reason_shape() {
        let entries = vec![feedback_entry(
            1,
            "cut",
            "Trim 0:12 — 0:12.42",
            Some("Too aggressive"),
        )];
        let rendered = render_recent_feedback_from_entries(&entries).expect("non-empty");
        assert!(rendered.starts_with("<recent_feedback>"));
        assert!(rendered.ends_with("</recent_feedback>"));
        // Shape: medium → quoted title → em-dash → "reason:" → text.
        assert!(
            rendered.contains("  - cut \"Trim 0:12 — 0:12.42\" — reason: Too aggressive"),
            "wrong shape: {rendered}"
        );
        // Behavioural intro is present.
        assert!(rendered.contains("Avoid repeating these patterns"));
    }

    /// Silent reject (no reason) renders `(no reason given)` so the
    /// agent still sees the frequency without inventing a reason.
    #[test]
    fn render_recent_feedback_renders_no_reason_shape() {
        let entries = vec![feedback_entry(1, "color", "Apply teal-orange", None)];
        let rendered = render_recent_feedback_from_entries(&entries).expect("non-empty");
        assert!(
            rendered.contains("  - color \"Apply teal-orange\" (no reason given)"),
            "missing no-reason marker: {rendered}"
        );
        // No stray "reason:" prefix when the user didn't supply one.
        assert!(
            !rendered.contains("reason: \n") && !rendered.contains("reason:  "),
            "blank reason leaked: {rendered}"
        );
    }

    /// Whitespace-only reason should be treated as no reason — we don't
    /// want to render `reason: ` followed by nothing.
    #[test]
    fn render_recent_feedback_whitespace_reason_falls_back_to_no_reason() {
        let entries = vec![feedback_entry(1, "broll", "Insert", Some("   "))];
        let rendered = render_recent_feedback_from_entries(&entries).expect("non-empty");
        assert!(rendered.contains("(no reason given)"), "got: {rendered}");
    }

    /// Multiple entries → one bullet per row, preserving the caller's
    /// order. (The caller — `load_recent_feedback_sync` — already
    /// orders newest-first.)
    #[test]
    fn render_recent_feedback_preserves_input_order() {
        let entries = vec![
            feedback_entry(3, "cut", "third", Some("a")),
            feedback_entry(2, "broll", "second", None),
            feedback_entry(1, "audio", "first", Some("c")),
        ];
        let rendered = render_recent_feedback_from_entries(&entries).expect("non-empty");
        let third_at = rendered.find("third").expect("third row missing");
        let second_at = rendered.find("second").expect("second row missing");
        let first_at = rendered.find("first").expect("first row missing");
        assert!(third_at < second_at);
        assert!(second_at < first_at);
    }

    /// Integration with the JSONL log path: a fresh project (no log)
    /// returns `None` so the bridge skips the section.
    #[test]
    fn render_recent_feedback_returns_none_when_log_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let out = render_recent_feedback(tmp.path());
        assert!(out.is_none(), "fresh project must skip the section");
    }

    /// Integration: write two rows to `.montage/feedback.jsonl`, the
    /// session-launch helper must surface them in the rendered fragment.
    #[test]
    fn render_recent_feedback_reads_jsonl_log_at_launch() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join(".montage");
        std::fs::create_dir_all(&dir).unwrap();
        // ts ascending in the file; the sync reader reverses to
        // newest-first, so "newer" appears above "older" in the output.
        let lines = format!(
            "{}\n{}\n",
            serde_json::json!({"ts":100,"medium":"cut","title":"older","rationale":null,"reason":"a"}),
            serde_json::json!({"ts":200,"medium":"broll","title":"newer","rationale":null,"reason":null}),
        );
        std::fs::write(dir.join("feedback.jsonl"), lines).unwrap();
        let rendered = render_recent_feedback(tmp.path()).expect("non-empty");
        assert!(rendered.contains("newer"));
        assert!(rendered.contains("older"));
        // Newest-first: "newer" appears before "older" in the rendered
        // block.
        let newer_at = rendered.find("newer").unwrap();
        let older_at = rendered.find("older").unwrap();
        assert!(
            newer_at < older_at,
            "rendered block should be newest-first; got:\n{rendered}"
        );
        // Newer row had no reason — rendered as "(no reason given)".
        assert!(rendered.contains("(no reason given)"));
        // Older row had reason "a" — rendered as "reason: a".
        assert!(rendered.contains("reason: a"));
    }

    /// Task-spec scenario — pin to v1.0.0 with both v1.0.0 (bundled)
    /// and v1.1.0 (user) installed. Only v1.0.0 wins. Asserted via
    /// the description so we know exactly which copy made it through.
    #[test]
    fn pin_to_v1_0_with_two_copies_only_v1_0_wins() {
        let bundled = make_skills_root_with_version("auto-cutter", "1.0.0", "the-good-one");
        let user = make_skills_root_with_version("auto-cutter", "1.1.0", "the-new-one");
        let pins = vec![PinnedSkill {
            name: "auto-cutter".to_string(),
            version: Some("1.0.0".to_string()),
            provenance: None,
        }];
        let rendered = render_skills_catalog_from_roots(
            Some(bundled.path()),
            &[user.path().to_path_buf()],
            None,
            &[],
            &pins,
        )
        .expect("catalog non-empty");
        assert!(rendered.contains("the-good-one"), "pinned v1.0.0 must win");
        assert!(
            !rendered.contains("the-new-one"),
            "v1.1.0 must be skipped under the pin; got:\n{rendered}"
        );
    }
}
