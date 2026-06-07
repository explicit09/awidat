//! OS-keychain-backed secret storage with environment-variable fallback.
//!
//! Per `PLAN.md` §10.4: API keys live in the OS keychain, not in config
//! files. The fallback to env vars exists for CI / first-run / containers
//! where the keychain isn't available.
//!
//! # Conventions
//!
//! - Service name: `"montage"`.
//! - Account names: lowercase-snake — `"hf_token"`, `"anthropic_api_key"`.
//! - Env-var names: SCREAMING_SNAKE — `HF_TOKEN`, `ANTHROPIC_API_KEY`.
//!
//! # Order of resolution
//!
//! [`get`] tries env var first, then keychain. Env-first because:
//! 1. CI and ephemeral runners only have env vars; keychain calls error out.
//! 2. Override-by-env is the universal "set this for this one run" idiom.

use thiserror::Error;
use tracing::trace;

/// Service name we register all entries under in the OS keychain.
pub const SERVICE: &str = "montage";

/// Errors talking to the keychain. Missing-secret is *not* an error — see
/// [`get`] which returns `Ok(None)`.
#[derive(Debug, Error)]
pub enum SecretError {
    /// Underlying keychain backend error (rare in practice — backend not
    /// installed, locked, etc.).
    #[error("keychain backend error for account '{account}': {source}")]
    Backend {
        /// Account that triggered the error.
        account: String,
        /// Underlying error.
        #[source]
        source: keyring::Error,
    },
}

/// Fetch a secret. Tries `env_var_name` first, then the OS keychain entry
/// `(SERVICE, account)`. Returns `Ok(None)` if neither is set —
/// callers decide whether absence is fatal.
pub fn get(env_var_name: &str, account: &str) -> Result<Option<String>, SecretError> {
    if let Ok(value) = std::env::var(env_var_name)
        && !value.is_empty()
    {
        trace!(env_var = env_var_name, "secret resolved from env var");
        return Ok(Some(value));
    }

    let entry = keyring::Entry::new(SERVICE, account).map_err(|e| SecretError::Backend {
        account: account.into(),
        source: e,
    })?;
    match entry.get_password() {
        Ok(value) => {
            trace!(account, "secret resolved from keychain");
            Ok(Some(value))
        }
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(SecretError::Backend {
            account: account.into(),
            source: e,
        }),
    }
}

/// Store a secret in the keychain under `(SERVICE, account)`. Overwrites
/// any existing value.
pub fn set(account: &str, value: &str) -> Result<(), SecretError> {
    let entry = keyring::Entry::new(SERVICE, account).map_err(|e| SecretError::Backend {
        account: account.into(),
        source: e,
    })?;
    entry.set_password(value).map_err(|e| SecretError::Backend {
        account: account.into(),
        source: e,
    })
}

/// Delete a secret from the keychain. No-op if it doesn't exist.
pub fn delete(account: &str) -> Result<(), SecretError> {
    let entry = keyring::Entry::new(SERVICE, account).map_err(|e| SecretError::Backend {
        account: account.into(),
        source: e,
    })?;
    match entry.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(SecretError::Backend {
            account: account.into(),
            source: e,
        }),
    }
}

/// Account names we use across the codebase. Defining them as constants
/// rather than literals keeps the env-var ↔ account mapping in one place.
pub mod accounts {
    /// HuggingFace token — needed for `pyannote` diarization model download.
    pub const HF_TOKEN: &str = "hf_token";
    /// Anthropic API key — used by `topic-mcp` premium labeling and the
    /// Week 3 agent loop.
    pub const ANTHROPIC_API_KEY: &str = "anthropic_api_key";
    /// Pexels API key — used by the b-roll search/use tools (Phase 3).
    pub const PEXELS_API_KEY: &str = "pexels_api_key";
    /// OpenRouter API key — used by generated-media video providers.
    pub const OPENROUTER_API_KEY: &str = "openrouter_api_key";
}

/// Env-var names corresponding to [`accounts`]. Kept in lockstep.
pub mod env_vars {
    /// Override for [`super::accounts::HF_TOKEN`].
    pub const HF_TOKEN: &str = "HF_TOKEN";
    /// Override for [`super::accounts::ANTHROPIC_API_KEY`].
    pub const ANTHROPIC_API_KEY: &str = "ANTHROPIC_API_KEY";
    /// Override for [`super::accounts::PEXELS_API_KEY`].
    pub const PEXELS_API_KEY: &str = "PEXELS_API_KEY";
    /// Override for [`super::accounts::OPENROUTER_API_KEY`].
    pub const OPENROUTER_API_KEY: &str = "OPENROUTER_API_KEY";
}

#[cfg(test)]
mod tests {
    use super::*;

    // The real env-var ↔ keychain interaction is exercised by manual smoke
    // (we deliberately don't write to the user's keychain from tests, and
    // mutating env in tests is `unsafe` in edition 2024 + multi-threaded by
    // the test runner's default).
    #[test]
    fn account_constants_are_distinct_from_env_var_names() {
        assert_ne!(accounts::HF_TOKEN, env_vars::HF_TOKEN);
        assert_ne!(accounts::ANTHROPIC_API_KEY, env_vars::ANTHROPIC_API_KEY);
        assert_ne!(accounts::PEXELS_API_KEY, env_vars::PEXELS_API_KEY);
        assert_ne!(accounts::OPENROUTER_API_KEY, env_vars::OPENROUTER_API_KEY);
    }

    #[test]
    fn service_name_is_stable() {
        // If anyone changes this, they need to migrate users. Loud test
        // catches it in review.
        assert_eq!(SERVICE, "montage");
    }
}
