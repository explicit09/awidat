//! Tauri-side glue around [`awidat_codex_bridge::CodexAppServer`].
//!
//! - [`TauriEmitter`] adapts Tauri's [`AppHandle`] to the bridge's
//!   [`ItemEmitter`] trait so the bridge can push `Item`s onto our
//!   Tauri event bus without depending on Tauri itself.
//! - [`CodexSession`] wraps the live bridge with the `project_root` it
//!   was launched against; the desktop tears it down + relaunches on
//!   project switch (see [`crate::commands::project`]).
//!
//! The MCP server sibling-binary lookup mirrors
//! `crates/cli/src/chat_codex_cmd.rs::awidat_mcp_overrides` (sibling
//! of `current_exe()`'s parent, named `awidat-mcp-server`). Unlike
//! the CLI, we can't assume `current_exe()` is `awidat` — in a
//! packaged Tauri build it's the app bundle binary. The MCP server
//! still has to live next to it for `cargo tauri dev` to find it.

use std::path::PathBuf;
use std::sync::Arc;

use awidat_codex_bridge::{BridgeError, CodexAppServer, ItemEmitter};
use awidat_desktop_protocol::Item;
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
            let state = app.state::<crate::state::AwidatState>();
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
/// [`crate::state::AwidatState`] inside an `Option`; `None` means no
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
        // Awidat tools" produces an agent that runs shell commands
        // instead of view_timeline / apply_edl. That mode is unhelpful
        // for editing; surface it before the user wastes a turn on it.
        if mcp_server_path.is_none() {
            let warning = "awidat-mcp-server binary missing next to awidat-desktop. \
                The agent will fall back to shell-only and won't use Awidat tools \
                (view_timeline, apply_edl, etc.). Build it with \
                `cargo build -p awidat-cli --bin awidat-mcp-server`.";
            tracing::error!("{warning}");
            crate::events::emit_item(
                &app,
                awidat_desktop_protocol::Item::Error {
                    id: awidat_desktop_protocol::Id::new("awidat-mcp-missing"),
                    message: warning.to_string(),
                },
            );
        }
        let emitter: Arc<dyn ItemEmitter> = Arc::new(TauriEmitter::new(app, project_root.clone()));
        // Per-format editorial addendum (Podcast / Shorts / Tutorial /
        // Other). Reads project type from the OTIO and assembles the
        // matching playbook; rides on `developer_instructions` so the
        // agent gets it without us touching codex's base prompt.
        let developer_instructions = Some(awidat_core::system_prompt::assemble_for_project(
            &project_root,
        ));
        // Progressive-disclosure skills catalog. L1 (name + description)
        // lands in every turn input as a contextual fragment; the agent
        // calls `load_skill(name='...')` for the L2 body. User-installed
        // skills under ~/Library/Application Support/awidat/skills and
        // ~/.config/awidat/skills override bundled. See
        // crates/core/src/skills.rs for the discovery rules.
        let skills_catalog = render_skills_catalog();
        let bridge = CodexAppServer::launch(
            emitter,
            project_root.clone(),
            mcp_server_path,
            developer_instructions,
            skills_catalog,
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
/// Discovery hierarchy from `awidat_core::skills`:
///   1. user roots — `~/Library/Application Support/awidat/skills`
///      and `~/.config/awidat/skills` (user overrides bundled)
///   2. bundled — `<repo>/skills` in dev; in a packaged build,
///      `<install>/share/awidat/skills`. We pick the repo-relative
///      `skills/` dir via the running binary's grandparent (works in
///      `cargo tauri dev`; packaged builds will need a separate
///      resolver later when we ship installers).
///
/// Non-fatal: errors during discovery (malformed SKILL.md, etc.) are
/// logged via `tracing::warn!` and dropped; the agent gets whatever
/// loaded successfully.
fn render_skills_catalog() -> Option<String> {
    use awidat_core::context::ContextualUserFragment;
    use awidat_core::skills::SkillRegistry;

    let user_roots = user_skill_roots();
    let bundled = bundled_skill_root();

    let (registry, errors) =
        SkillRegistry::discover_many(bundled.as_deref(), user_roots.iter().map(PathBuf::as_path));
    for err in errors {
        tracing::warn!(?err, "skill discovery: malformed entry skipped");
    }
    let fragment = registry.l1_fragment()?;
    Some(fragment.render())
}

fn user_skill_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(home) = dirs::home_dir() {
        roots.push(home.join("Library/Application Support/awidat/skills"));
        roots.push(home.join(".config/awidat/skills"));
    }
    roots
}

/// Best-effort bundled-skills root. In `cargo tauri dev` the binary
/// lives at `<repo>/target/debug/awidat-desktop`, so the skills dir
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

/// Resolve the sibling `awidat-mcp-server` binary path so the bridge
/// can inject it as a `mcp_servers.awidat.command` config override.
///
/// `None` means we couldn't find it; the bridge then runs codex
/// without our MCP tools (matching the pre-step-3 behavior).
fn resolve_mcp_server_binary() -> Option<PathBuf> {
    let self_exe = std::env::current_exe().ok()?;
    let parent = self_exe.parent()?;
    let candidate = parent.join("awidat-mcp-server");
    if candidate.exists() {
        Some(candidate)
    } else {
        tracing::warn!(
            path = %candidate.display(),
            "awidat-mcp-server sibling binary missing; agent will run without Awidat tools"
        );
        None
    }
}
