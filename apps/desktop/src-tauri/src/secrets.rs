//! Initialize provider-secret handling for the desktop process.
//!
//! Provider keys live in the shared `montage-secrets` vault. Rust callers read
//! them through `montage_secrets::get`, and MCP subprocesses receive the keys
//! through their per-child env map at launch time in `montage-index`.
//!
//! We deliberately avoid mutating the global process environment from runtime
//! settings commands. On macOS/Unix, `std::env::set_var` is unsafe once other
//! threads may read env vars, and removing a global value cannot distinguish a
//! saved Provider Keys value from a launch-time override.

use std::sync::OnceLock;

use tracing::info;

/// Marker that startup initialization has already run.
static RESOLVED: OnceLock<()> = OnceLock::new();

/// Initialize provider-secret handling once at app startup. Indexer subprocess
/// env hydration is done at child launch, so startup does not touch keychain or
/// global env here.
pub fn resolve_at_startup() {
    if RESOLVED.set(()).is_err() {
        return; // Already ran.
    }
    info!("provider secrets will hydrate per child process at launch");
}
