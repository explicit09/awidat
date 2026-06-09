//! Resolve API keys once at app startup and propagate them into the
//! process env so MCP subprocesses (indexers, etc.) inherit them.
//!
//! # Why this exists
//!
//! `montage-secrets` checks env first, then the OS keychain. The
//! desktop's `Session::new` flow uses that, so the agent loop works
//! whether the key lives in env or keychain. But MCP indexer
//! subprocesses spawned by `montage-index::run` only see the parent
//! process's env — they don't get the keychain. So when a user
//! stored their `ANTHROPIC_API_KEY` in the macOS keychain, the
//! `editorial-moments` indexer (which calls Claude Haiku) fails
//! with "ANTHROPIC_API_KEY not in env."
//!
//! The fix: at app startup, ask `montage-secrets` for each key we
//! care about; if it came from keychain, write it to the parent
//! process's env via `std::env::set_var` so subprocesses inherit.
//! ONE keychain read per process lifetime, no matter how many
//! indexer runs / Sessions / agent calls happen later.
//!
//! # Dev note: keychain prompts on every launch
//!
//! macOS gates keychain access by codesigning identity. A
//! `cargo run` / `tauri dev` debug binary has an ad-hoc signature
//! that changes across rebuilds, so macOS prompts on every launch
//! once the user clicks "Always Allow" for a given codesign.
//!
//! Release builds prefetch by default so subprocesses inherit secrets
//! without extra setup. Debug builds do not: a `cargo run` / `tauri
//! dev` binary's ad-hoc signature changes often enough that eager
//! startup reads can produce a burst of macOS Keychain prompts.
//!
//! Override via `MONTAGE_PREFETCH_KEYCHAIN=1` or `0`. Setting the key
//! in shell env (e.g. `export HF_TOKEN=...`) also avoids both the
//! prompt and the keychain hit entirely.

use std::sync::OnceLock;

use montage_secrets::{accounts, env_vars};
use tracing::{info, warn};

/// Set of well-known keys we resolve at startup.
const RESOLVE_AT_STARTUP: &[(&str, &str)] = &[
    (env_vars::ANTHROPIC_API_KEY, accounts::ANTHROPIC_API_KEY),
    (env_vars::HF_TOKEN, accounts::HF_TOKEN),
    (env_vars::OPENROUTER_API_KEY, accounts::OPENROUTER_API_KEY),
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
    if !prefetch_enabled() {
        info!("skipping startup keychain prefetch");
        return;
    }
    for (env_name, account) in RESOLVE_AT_STARTUP {
        match montage_secrets::get(env_name, account) {
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

fn prefetch_enabled() -> bool {
    prefetch_enabled_for(std::env::var("MONTAGE_PREFETCH_KEYCHAIN").ok().as_deref())
}

fn prefetch_enabled_for(value: Option<&str>) -> bool {
    // Empty string is treated as "not set" so shell defaults that pass
    // through empty values don't force a choice.
    match value.filter(|s| !s.is_empty()) {
        None => default_prefetch_enabled(),
        Some("1" | "true" | "TRUE" | "yes" | "YES" | "on" | "ON") => true,
        Some(_) => false,
    }
}

fn default_prefetch_enabled() -> bool {
    !cfg!(debug_assertions)
}

#[cfg(test)]
mod tests {
    use super::prefetch_enabled_for;

    #[test]
    fn debug_builds_do_not_prefetch_keychain_by_default() {
        if cfg!(debug_assertions) {
            assert!(!prefetch_enabled_for(None));
        }
    }

    #[test]
    fn explicit_prefetch_env_overrides_debug_default() {
        assert!(prefetch_enabled_for(Some("1")));
        assert!(prefetch_enabled_for(Some("yes")));
        assert!(!prefetch_enabled_for(Some("0")));
        assert!(!prefetch_enabled_for(Some("false")));
    }
}
