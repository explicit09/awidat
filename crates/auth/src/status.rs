//! Classifying the current credential into a UI-ready status snapshot.

use codex_login::{AuthDotJson, load_auth_dot_json};
use serde::{Deserialize, Serialize};

use crate::env::AuthEnv;

/// Which credential the running agent will use — i.e. *which wallet gets charged*.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthModeKind {
    /// Signed in with ChatGPT; inference spends the user's ChatGPT plan.
    ChatGpt,
    /// API key; inference is billed to the user's OpenAI Platform account.
    ApiKey,
    /// A ChatGPT workspace agent-identity token.
    AgentIdentity,
    /// No credentials found.
    None,
}

/// Human-readable "which wallet gets charged" copy. The single source of truth
/// for the transparency text the UI shows, so the chooser, the persistent status
/// chip, and any CLI output stay consistent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WalletLabel {
    /// Short name, e.g. "ChatGPT subscription".
    pub title: String,
    /// One-sentence explanation of what gets charged.
    pub detail: String,
}

impl WalletLabel {
    fn for_mode(mode: AuthModeKind) -> Self {
        match mode {
            AuthModeKind::ChatGpt => Self {
                title: "ChatGPT subscription".to_string(),
                detail: "Uses your ChatGPT plan's Codex allowance — no per-token API charges, \
                         subject to your plan's usage limits."
                    .to_string(),
            },
            AuthModeKind::ApiKey => Self {
                title: "OpenAI API key".to_string(),
                detail: "Billed per-token to your OpenAI Platform account at standard API rates."
                    .to_string(),
            },
            AuthModeKind::AgentIdentity => Self {
                title: "Agent identity token".to_string(),
                detail: "Uses a ChatGPT workspace agent-identity token.".to_string(),
            },
            AuthModeKind::None => Self {
                title: "Not signed in".to_string(),
                detail: "No OpenAI credentials found — sign in with ChatGPT or add an API key."
                    .to_string(),
            },
        }
    }
}

/// A snapshot of the effective auth state, ready to serialize across the Tauri
/// boundary or render in the chooser.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthStatus {
    /// The effective mode.
    pub mode: AuthModeKind,
    /// Transparency copy for `mode`.
    pub wallet: WalletLabel,
    /// Masked identifier (e.g. `sk-…wxyz`) — never the full secret. `None` when
    /// no safe hint exists (ChatGPT tokens carry no plaintext key here).
    pub account_hint: Option<String>,
    /// True when the effective mode comes from an environment variable rather
    /// than stored `auth.json`. Env wins, mirroring codex's own precedence.
    pub via_env: bool,
    /// When `via_env`, the *name* of the overriding variable (e.g.
    /// `CODEX_API_KEY`), so the UI can tell the user exactly which variable to
    /// unset instead of guessing. `None` when not env-derived.
    pub env_var: Option<String>,
}

impl AuthStatus {
    /// Build a stored-auth status (not env-derived).
    fn new(mode: AuthModeKind, account_hint: Option<String>, via_env: bool) -> Self {
        Self {
            mode,
            wallet: WalletLabel::for_mode(mode),
            account_hint,
            via_env,
            env_var: None,
        }
    }

    /// Build an env-override status, recording which variable is responsible.
    fn from_env(mode: AuthModeKind, account_hint: Option<String>, env_var: &str) -> Self {
        Self {
            env_var: Some(env_var.to_string()),
            ..Self::new(mode, account_hint, true)
        }
    }
}

/// Read the effective auth state.
///
/// Mirrors the precedence of codex's own turn-auth resolver (`load_auth`): env
/// credentials win over stored `auth.json`, in the order `CODEX_API_KEY` then
/// `CODEX_ACCESS_TOKEN`. Otherwise we classify the stored `auth.json`. A read
/// error is treated as "signed out" (and logged) rather than propagated — status
/// is a display concern, not a place to fail a turn.
///
/// Note `OPENAI_API_KEY` is deliberately *not* an override here: the bridge runs
/// codex with `enable_codex_api_key_env: true`, but `load_auth` only consults
/// `CODEX_API_KEY` / `CODEX_ACCESS_TOKEN` for turns — `OPENAI_API_KEY` affects
/// only realtime-voice sessions (which awidat doesn't use), so reporting it as
/// the active wallet would misstate what turns are billed to.
pub fn status(env: &AuthEnv) -> AuthStatus {
    if let Some(status) = env_override_status() {
        return status;
    }
    match load_auth_dot_json(&env.codex_home, env.store_mode) {
        Ok(Some(auth)) => classify(&auth),
        Ok(None) => AuthStatus::new(AuthModeKind::None, None, false),
        Err(err) => {
            tracing::warn!(error = %err, "failed to read auth.json; reporting signed-out");
            AuthStatus::new(AuthModeKind::None, None, false)
        }
    }
}

/// Classify a stored payload. Delegates to [`classify_parts`] so the decision
/// logic is unit-testable without constructing a real `TokenData`.
fn classify(auth: &AuthDotJson) -> AuthStatus {
    classify_parts(
        auth.tokens.is_some(),
        auth.openai_api_key.as_deref(),
        auth.agent_identity.is_some(),
    )
}

/// Pure classification core.
///
/// `tokens` is checked *first* on purpose: after a ChatGPT login codex may also
/// stash an auto-generated API key (the openai/codex#2000 behaviour), so the
/// presence of OAuth tokens must win — the user is on their subscription, not
/// paying per-token.
fn classify_parts(has_tokens: bool, api_key: Option<&str>, has_agent_identity: bool) -> AuthStatus {
    if has_tokens {
        AuthStatus::new(AuthModeKind::ChatGpt, None, false)
    } else if let Some(key) = api_key {
        AuthStatus::new(AuthModeKind::ApiKey, Some(mask_secret(key)), false)
    } else if has_agent_identity {
        AuthStatus::new(AuthModeKind::AgentIdentity, None, false)
    } else {
        AuthStatus::new(AuthModeKind::None, None, false)
    }
}

/// The env credential the running agent would use for turns, classified, if any.
///
/// Order matches codex's `load_auth`: `CODEX_API_KEY` (→ API key) then
/// `CODEX_ACCESS_TOKEN` (→ agent identity). Both carry `via_env: true` so the UI
/// can warn that an environment variable is overriding stored auth.
fn env_override_status() -> Option<AuthStatus> {
    if let Some(key) = read_env_var(codex_login::CODEX_API_KEY_ENV_VAR) {
        return Some(AuthStatus::from_env(
            AuthModeKind::ApiKey,
            Some(mask_secret(&key)),
            codex_login::CODEX_API_KEY_ENV_VAR,
        ));
    }
    if read_env_var(codex_login::CODEX_ACCESS_TOKEN_ENV_VAR).is_some() {
        return Some(AuthStatus::from_env(
            AuthModeKind::AgentIdentity,
            None,
            codex_login::CODEX_ACCESS_TOKEN_ENV_VAR,
        ));
    }
    None
}

/// Read an env var, trimmed, treating empty as unset — matches codex's own
/// `read_non_empty_env_var`.
fn read_env_var(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// Mask a secret to a safe hint: `sk-…` plus the last four characters. Never
/// returns more than four characters of the original.
fn mask_secret(secret: &str) -> String {
    let mut tail: Vec<char> = secret.chars().rev().take(4).collect();
    tail.reverse();
    let tail: String = tail.into_iter().collect();
    format!("sk-…{tail}")
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::env::AuthEnv;
    use codex_login::AuthCredentialsStoreMode;
    use tempfile::TempDir;

    #[test]
    fn tokens_present_means_chatgpt() {
        let status = classify_parts(true, None, false);
        assert_eq!(status.mode, AuthModeKind::ChatGpt);
        assert_eq!(status.account_hint, None);
    }

    #[test]
    fn tokens_win_over_a_stashed_api_key() {
        // The openai/codex#2000 case: ChatGPT login also wrote an auto API key.
        let status = classify_parts(true, Some("sk-auto-generated-1234"), false);
        assert_eq!(status.mode, AuthModeKind::ChatGpt);
    }

    #[test]
    fn api_key_only_means_api_key_with_masked_hint() {
        let status = classify_parts(false, Some("sk-proj-abcdefghwxyz"), false);
        assert_eq!(status.mode, AuthModeKind::ApiKey);
        assert_eq!(status.account_hint.as_deref(), Some("sk-…wxyz"));
    }

    #[test]
    fn agent_identity_is_classified() {
        let status = classify_parts(false, None, true);
        assert_eq!(status.mode, AuthModeKind::AgentIdentity);
    }

    #[test]
    fn nothing_stored_means_none() {
        let status = classify_parts(false, None, false);
        assert_eq!(status.mode, AuthModeKind::None);
        assert_eq!(status.account_hint, None);
    }

    #[test]
    fn wallet_copy_is_attached_per_mode() {
        assert_eq!(
            classify_parts(true, None, false).wallet.title,
            "ChatGPT subscription"
        );
        assert_eq!(
            classify_parts(false, Some("sk-proj-abcdefghwxyz"), false)
                .wallet
                .title,
            "OpenAI API key"
        );
    }

    #[test]
    fn mask_secret_never_leaks_more_than_four_chars() {
        assert_eq!(mask_secret("sk-proj-supersecretvalue1234"), "sk-…1234");
        assert_eq!(mask_secret("ab"), "sk-…ab");
    }

    #[test]
    fn classify_reads_an_api_key_round_trip_from_disk() {
        // Full pipeline (write via codex-login → load → classify) without env or
        // network. Guards the wiring, not just the pure core.
        let home = TempDir::new().unwrap();
        let env = AuthEnv::new(home.path().to_path_buf(), AuthCredentialsStoreMode::File);
        codex_login::login_with_api_key(
            &env.codex_home,
            "sk-proj-rounDtripKey0123456789",
            env.store_mode,
        )
        .unwrap();

        let stored = load_auth_dot_json(&env.codex_home, env.store_mode)
            .unwrap()
            .unwrap();
        let status = classify(&stored);
        assert_eq!(status.mode, AuthModeKind::ApiKey);
        assert_eq!(status.account_hint.as_deref(), Some("sk-…6789"));
    }
}
