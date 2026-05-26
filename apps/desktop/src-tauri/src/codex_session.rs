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

use crate::events::{emit_item, emit_turn_end};

/// Bridges the codex-bridge's pure-Rust [`ItemEmitter`] trait onto
/// Tauri's `AppHandle::emit`. Clone-cheap (`AppHandle` is `Clone`).
pub struct TauriEmitter {
    app: AppHandle,
}

impl TauriEmitter {
    pub fn new(app: AppHandle) -> Self {
        Self { app }
    }
}

impl ItemEmitter for TauriEmitter {
    fn emit_item(&self, item: Item) {
        emit_item(&self.app, item);
    }

    fn emit_turn_end(&self, error: Option<String>) {
        emit_turn_end(&self.app, error);
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
        let emitter: Arc<dyn ItemEmitter> = Arc::new(TauriEmitter::new(app));
        let mcp_server_path = resolve_mcp_server_binary();
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
