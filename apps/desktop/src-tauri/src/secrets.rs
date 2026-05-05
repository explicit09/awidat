//! Resolve API keys once at app startup and propagate them into the
//! process env so MCP subprocesses (indexers, etc.) inherit them.
//!
//! # Why this exists
//!
//! `awidat-secrets` checks env first, then the OS keychain. The
//! desktop's `Session::new` flow uses that, so the agent loop works
//! whether the key lives in env or keychain. But MCP indexer
//! subprocesses spawned by `awidat-index::run` only see the parent
//! process's env — they don't get the keychain. So when a user
//! stored their `ANTHROPIC_API_KEY` in the macOS keychain, the
//! `editorial-moments` indexer (which calls Claude Haiku) fails
//! with "ANTHROPIC_API_KEY not in env."
//!
//! The fix: at app startup, ask `awidat-secrets` for each key we
//! care about; if it came from keychain, write it to the parent
//! process's env via `std::env::set_var` so subprocesses inherit.
//! ONE keychain read per process lifetime, no matter how many
//! indexer runs / Sessions / agent calls happen later.
//!
//! # Dev note: keychain prompts on every launch
//!
//! macOS gates keychain access by codesigning identity. A
//! `cargo run` debug binary has an ad-hoc signature that changes
//! across rebuilds, so macOS prompts on every launch. The release
//! `.app` bundle from `tauri build` is properly signed and prompts
//! exactly once (click "Always Allow"). Until we ship release, set
//! `ANTHROPIC_API_KEY` in your shell env to avoid the prompt
//! entirely.

use std::sync::OnceLock;

use awidat_secrets::{accounts, env_vars};
use tracing::{info, warn};

/// Set of well-known keys we resolve at startup.
const RESOLVE_AT_STARTUP: &[(&str, &str)] = &[
    (env_vars::ANTHROPIC_API_KEY, accounts::ANTHROPIC_API_KEY),
    (env_vars::HF_TOKEN, accounts::HF_TOKEN),
];

/// Marker that startup resolution has already run. We don't store
/// the resolved values here — they live in the process env after
/// resolution, where MCP subprocesses can inherit them.
static RESOLVED: OnceLock<()> = OnceLock::new();

/// Resolve every known API key once at app startup. Idempotent —
/// subsequent calls are a no-op via `OnceLock`. Called from
/// `lib.rs::run` before any code that might spawn an MCP subprocess.
///
/// Behavior per key:
/// - If already in env: leave as-is (env is canonical).
/// - If in keychain only: copy to env so subprocesses inherit.
/// - If in neither: silently skip (the agent loop will fail with a
///   clear error when it actually needs the key).
pub fn resolve_at_startup() {
    if RESOLVED.set(()).is_err() {
        return; // Already ran.
    }
    for (env_name, account) in RESOLVE_AT_STARTUP {
        match awidat_secrets::get(env_name, account) {
            Ok(Some(value)) if std::env::var(env_name).is_err() => {
                // Came from keychain (env was unset before the get
                // call), or we'd have hit the env branch). Export to
                // env so subprocesses inherit.
                //
                // SAFETY: `std::env::set_var` is `unsafe` in edition
                // 2024 because cross-thread env mutation is racy on
                // some POSIX systems. We call this exactly once at
                // app startup before any threads spawn that would
                // read the env, so the racy window is empty.
                #[allow(unsafe_code)]
                unsafe {
                    std::env::set_var(env_name, &value);
                }
                info!(env_name, "exported keychain secret to env");
            }
            Ok(Some(_)) => {
                // Already in env — nothing to do.
            }
            Ok(None) => {
                // Neither env nor keychain. Not an error here; the
                // first piece of code that needs the key will fail
                // with a clear "set X or store via keychain" message.
            }
            Err(e) => {
                warn!(error = %e, env_name, "failed to resolve secret at startup");
            }
        }
    }
}
