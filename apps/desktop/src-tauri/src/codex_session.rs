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
use tauri::AppHandle;

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

    fn emit_turn_end(&self, error: Option<String>) {
        emit_turn_end(&self.app, error);
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
    pub async fn launch(app: AppHandle, project_root: PathBuf) -> Result<Self, BridgeError> {
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
        let emitter: Arc<dyn ItemEmitter> =
            Arc::new(TauriEmitter::new(app, project_root.clone()));
        let bridge =
            CodexAppServer::launch(emitter, project_root.clone(), mcp_server_path).await?;
        Ok(Self {
            bridge,
            project_root,
        })
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
