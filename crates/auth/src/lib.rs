//! The Montage ↔ codex authentication boundary.
//!
//! Montage's agent is powered by the vendored codex harness, which authenticates
//! to OpenAI two ways: **Sign in with ChatGPT** (OAuth — spends the user's
//! ChatGPT plan) or an **API key** (billed at standard API rates). This crate is
//! the single, UI-agnostic place that drives codex's own auth machinery
//! ([`codex_login`]) so montage surfaces (desktop, CLI) can present an in-app
//! auth-mode chooser without reimplementing OAuth, token refresh, or storage.
//!
//! Everything is written *where codex reads it* — the same `CODEX_HOME` and the
//! same credential store mode — so a login performed through montage is
//! immediately visible to the running agent.
//!
//! ## The one policy-sensitive knob
//!
//! ChatGPT sign-in requires an explicitly configured OAuth client id. The API-key
//! path is the only mode OpenAI officially supports for third-party apps, so it
//! is kept first-class and remains available without OAuth configuration.

mod env;
mod login;
mod status;
mod validate;

pub use env::{AuthEnv, ForcedMethod};
pub use login::{LoginHandle, begin_chatgpt_login, logout, set_api_key};
pub use status::{AuthModeKind, AuthStatus, WalletLabel, status};
pub use validate::validate_api_key;

/// Re-exported so callers can construct an [`AuthEnv`] with an explicit store
/// mode, or retain a cancel handle for a pending login, without depending on
/// codex crates directly.
pub use codex_login::{AuthCredentialsStoreMode, ShutdownHandle};

/// Environment variable that configures which OAuth client id ChatGPT sign-in
/// uses. Empty/unset disables ChatGPT OAuth for public source builds.
pub const OAUTH_CLIENT_ID_ENV: &str = "MONTAGE_OAUTH_CLIENT_ID";

/// The OAuth client id used for "Sign in with ChatGPT".
///
/// Public source builds do not fall back to a bundled ChatGPT OAuth client id.
/// Configure [`OAUTH_CLIENT_ID_ENV`] with a sanctioned client id to enable this
/// flow, or use API-key auth.
///
/// The vendored codex refresh/revoke paths read the same variable so a
/// sanctioned client id stays consistent across the full OAuth lifecycle.
pub fn oauth_client_id() -> Result<String, AuthError> {
    match std::env::var(OAUTH_CLIENT_ID_ENV)
        .ok()
        .filter(|value| !value.trim().is_empty())
    {
        Some(override_id) => Ok(override_id),
        None => Err(AuthError::ChatGptOAuthNotConfigured),
    }
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

    /// The action is disallowed by a managed-install `forced_login_method` policy.
    #[error("{0}")]
    ForbiddenByPolicy(String),

    /// ChatGPT OAuth is unavailable because no sanctioned client id was supplied.
    #[error(
        "ChatGPT OAuth is not configured for this build. Set MONTAGE_OAUTH_CLIENT_ID to a sanctioned client id or use API-key auth."
    )]
    ChatGptOAuthNotConfigured,

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
        assert!(
            matches!(oauth_client_id(), Err(AuthError::ChatGptOAuthNotConfigured)),
            "unset must not fall back to codex first-party client"
        );

        unsafe { std::env::set_var(OAUTH_CLIENT_ID_ENV, "app_custom_test_client") };
        assert_eq!(
            oauth_client_id().unwrap(),
            "app_custom_test_client",
            "non-empty override must win"
        );

        unsafe { std::env::set_var(OAUTH_CLIENT_ID_ENV, "   ") };
        assert!(
            matches!(oauth_client_id(), Err(AuthError::ChatGptOAuthNotConfigured)),
            "blank override must not fall back to codex first-party client"
        );

        unsafe { std::env::remove_var(OAUTH_CLIENT_ID_ENV) };
    }
}
