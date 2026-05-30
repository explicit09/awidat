//! The Awidat ↔ codex authentication boundary.
//!
//! Awidat's agent is powered by the vendored codex harness, which authenticates
//! to OpenAI two ways: **Sign in with ChatGPT** (OAuth — spends the user's
//! ChatGPT plan) or an **API key** (billed at standard API rates). This crate is
//! the single, UI-agnostic place that drives codex's own auth machinery
//! ([`codex_login`]) so awidat surfaces (desktop, CLI) can present an in-app
//! auth-mode chooser without reimplementing OAuth, token refresh, or storage.
//!
//! Everything is written *where codex reads it* — the same `CODEX_HOME` and the
//! same credential store mode — so a login performed through awidat is
//! immediately visible to the running agent.
//!
//! ## The one policy-sensitive knob
//!
//! ChatGPT sign-in reuses codex's first-party OAuth client id. OpenAI has
//! neither sanctioned nor prohibited third-party reuse, so we keep that id in a
//! single env-overridable place ([`oauth_client_id`]) — if the policy changes we
//! swap one constant, not a flow. The API-key path is the only mode OpenAI
//! officially supports for third-party apps, so it is kept first-class.

mod env;
mod login;
mod status;
mod validate;

pub use env::AuthEnv;
pub use login::{LoginHandle, begin_chatgpt_login, logout, set_api_key};
pub use status::{AuthModeKind, AuthStatus, WalletLabel, status};
pub use validate::validate_api_key;

/// Re-exported so callers can construct an [`AuthEnv`] with an explicit store
/// mode without depending on codex crates directly.
pub use codex_login::AuthCredentialsStoreMode;

/// Environment variable that overrides which OAuth client id ChatGPT sign-in
/// uses. Empty/unset falls back to codex's first-party client.
pub const OAUTH_CLIENT_ID_ENV: &str = "AWIDAT_OAUTH_CLIENT_ID";

/// The OAuth client id used for "Sign in with ChatGPT".
///
/// Defaults to codex's first-party client id (the only client that bills a
/// user's ChatGPT subscription for inference). Override via
/// [`OAUTH_CLIENT_ID_ENV`] to point at a different registered client.
///
/// This indirection is the project's escape hatch: reusing codex's client is
/// policy-unsanctioned, so if OpenAI ships a real third-party program (or
/// clamps down) we change behaviour by setting one env var.
pub fn oauth_client_id() -> String {
    std::env::var(OAUTH_CLIENT_ID_ENV)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| codex_login::CLIENT_ID.to_string())
}

/// Errors surfaced by this crate. Fail loud with full context — callers (Tauri
/// commands, CLI) map these to user-facing strings.
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    /// The supplied API key failed validation before any disk write.
    #[error("invalid API key: {0}")]
    InvalidApiKey(String),

    /// `CODEX_HOME` could not be resolved (e.g. it points at a missing path).
    #[error("could not resolve CODEX_HOME: {0}")]
    Home(#[source] std::io::Error),

    /// A filesystem / codex-login operation failed.
    #[error(transparent)]
    Io(std::io::Error),
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    // One test, run sequentially, because all cases mutate the same process-global
    // env var — separate `#[test]`s would race under cargo's parallel runner.
    #[test]
    fn oauth_client_id_default_override_and_blank() {
        // SAFETY: this is the only test touching OAUTH_CLIENT_ID_ENV, and it
        // restores the var before returning, so no other test observes the
        // mutation.
        unsafe { std::env::remove_var(OAUTH_CLIENT_ID_ENV) };
        assert_eq!(
            oauth_client_id(),
            codex_login::CLIENT_ID,
            "unset must fall back to codex first-party client"
        );

        unsafe { std::env::set_var(OAUTH_CLIENT_ID_ENV, "app_custom_test_client") };
        assert_eq!(
            oauth_client_id(),
            "app_custom_test_client",
            "non-empty override must win"
        );

        unsafe { std::env::set_var(OAUTH_CLIENT_ID_ENV, "   ") };
        assert_eq!(
            oauth_client_id(),
            codex_login::CLIENT_ID,
            "blank override must be ignored"
        );

        unsafe { std::env::remove_var(OAUTH_CLIENT_ID_ENV) };
}
