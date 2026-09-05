//! Per-tool-call context for the Montage MCP server.
//!
//! Resolves the project root from the environment, with a current-directory
//! fallback, for tools running in the Codex-spawned server process.

use std::path::PathBuf;

/// The environment variable `chat-codex` sets (or the user sets in
/// their shell) to tell the MCP server which Montage project to
/// operate on.
pub const PROJECT_ROOT_ENV: &str = "MONTAGE_PROJECT_ROOT";

/// Per-tool-call context. Cheap to construct; tools call
/// [`McpToolCtx::resolve`] inside their handler.
#[derive(Debug, Clone)]
pub struct McpToolCtx {
    /// Absolute path to the Montage project directory the tool should
    /// read/write. Resolved at call time so a long-running MCP
    /// server picks up env-var changes between turns.
    pub project_root: PathBuf,
}

impl McpToolCtx {
    /// Resolve the per-call context from the environment.
    ///
    /// Order of precedence:
    /// 1. `MONTAGE_PROJECT_ROOT` env var (set by `chat-codex` from the
    ///    user's shell).
    /// 2. The process's `current_dir()` — typically the user's cwd
    ///    when they launched `montage`.
    ///
    /// Falling back to cwd surfaces a `tracing::warn!` so a tool call
    /// outside a project is greppable in logs. Tools that need to
    /// hard-fail when no project is configured can compare against
    /// the warning or check `std::env::var_os(PROJECT_ROOT_ENV)`
    /// themselves.
    pub fn resolve() -> Self {
        let project_root = match std::env::var_os(PROJECT_ROOT_ENV) {
            Some(value) => PathBuf::from(value),
            None => {
                tracing::warn!(
                    env = PROJECT_ROOT_ENV,
                    "env not set; falling back to current_dir() for project_root"
                );
                std::env::current_dir().unwrap_or_default()
            }
        };
        Self { project_root }
    }
}
