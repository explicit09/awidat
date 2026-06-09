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

use std::{collections::BTreeMap, fmt};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::trace;

/// Service name we register all entries under in the OS keychain.
pub const SERVICE: &str = "montage";
/// Legacy service name used before the Montage rename.
pub const LEGACY_SERVICE: &str = "awidat";
/// Account name used to store the serialized provider secret vault.
pub const VAULT_ACCOUNT: &str = "secrets_vault";

/// Versioned provider secret vault.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct SecretVault {
    pub version: u32,
    #[serde(default)]
    pub providers: BTreeMap<String, VaultSecret>,
}

impl Default for SecretVault {
    fn default() -> Self {
        Self {
            version: 1,
            providers: BTreeMap::new(),
        }
    }
}

#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct VaultSecret {
    pub value: String,
    pub updated_at: String,
}

impl fmt::Debug for VaultSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VaultSecret")
            .field("value", &"<redacted>")
            .field("updated_at", &self.updated_at)
            .finish()
    }
}

/// Configuration status for a provider secret.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderSecretStatus {
    NotSet,
    Configured,
}

impl SecretVault {
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    pub fn get(&self, account: &str) -> Option<&str> {
        self.providers
            .get(account)
            .map(|secret| secret.value.as_str())
            .filter(|value| !value.is_empty())
    }

    pub fn set(&mut self, account: &str, value: &str) {
        if value.is_empty() {
            self.remove(account);
            return;
        }

        self.providers.insert(
            account.to_string(),
            VaultSecret {
                value: value.to_string(),
                updated_at: "1970-01-01T00:00:00Z".to_string(),
            },
        );
    }

    pub fn remove(&mut self, account: &str) {
        self.providers.remove(account);
    }

    pub fn status(&self, account: &str) -> ProviderSecretStatus {
        match self.get(account) {
            Some(_) => ProviderSecretStatus::Configured,
            None => ProviderSecretStatus::NotSet,
        }
    }
}

pub fn resolve_from_env_or_vault(
    env_value: Option<&str>,
    vault: &SecretVault,
    account: &str,
) -> Option<String> {
    env_value
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| vault.get(account).map(str::to_string))
}

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
    /// Serialized provider vault data could not be decoded.
    #[error("provider key vault is corrupt: {message}")]
    CorruptVault { message: String },
}

trait SecretBackend {
    fn get_password(&self, account: &str) -> Result<Option<String>, keyring::Error>;
    fn set_password(&self, account: &str, value: &str) -> Result<(), keyring::Error>;
}

struct KeychainSecretBackend;

impl SecretBackend for KeychainSecretBackend {
    fn get_password(&self, account: &str) -> Result<Option<String>, keyring::Error> {
        let entry = keyring::Entry::new(SERVICE, account)?;
        match entry.get_password() {
            Ok(value) => Ok(Some(value)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn set_password(&self, account: &str, value: &str) -> Result<(), keyring::Error> {
        let entry = keyring::Entry::new(SERVICE, account)?;
        entry.set_password(value)
    }
}

/// Fetch a secret. Tries `env_var_name` first, then the OS keychain entry
/// `(SERVICE, account)`, then the legacy `(LEGACY_SERVICE, account)`.
/// Returns `Ok(None)` if none are set —
/// callers decide whether absence is fatal.
pub fn get(env_var_name: &str, account: &str) -> Result<Option<String>, SecretError> {
    if let Ok(value) = std::env::var(env_var_name)
        && !value.is_empty()
    {
        trace!(env_var = env_var_name, "secret resolved from env var");
        return Ok(Some(value));
    }

    for service in secret_read_services() {
        let entry = keyring::Entry::new(service, account).map_err(|e| SecretError::Backend {
            account: account.into(),
            source: e,
        })?;
        match entry.get_password() {
            Ok(value) => {
                trace!(account, service, "secret resolved from keychain");
                return Ok(Some(value));
            }
            Err(keyring::Error::NoEntry) => {}
            Err(e) => {
                return Err(SecretError::Backend {
                    account: account.into(),
                    source: e,
                });
            }
        }
    }
    Ok(None)
}

fn secret_read_services() -> [&'static str; 2] {
    [SERVICE, LEGACY_SERVICE]
}

/// Load the serialized provider secret vault from the OS keychain.
///
/// A missing vault entry means no provider secrets have been stored yet, so this
/// returns the default empty vault.
pub fn load_vault() -> Result<SecretVault, SecretError> {
    load_vault_with_backend(&KeychainSecretBackend)
}

fn load_vault_with_backend(backend: &impl SecretBackend) -> Result<SecretVault, SecretError> {
    let Some(json) = backend
        .get_password(VAULT_ACCOUNT)
        .map_err(|e| SecretError::Backend {
            account: VAULT_ACCOUNT.to_string(),
            source: e,
        })?
    else {
        return Ok(SecretVault::default());
    };

    SecretVault::from_json(&json).map_err(|e| SecretError::CorruptVault {
        message: e.to_string(),
    })
}

/// Store the serialized provider secret vault in a single OS keychain entry.
pub fn save_vault(vault: &SecretVault) -> Result<(), SecretError> {
    save_vault_with_backend(&KeychainSecretBackend, vault)
}

fn save_vault_with_backend(
    backend: &impl SecretBackend,
    vault: &SecretVault,
) -> Result<(), SecretError> {
    let json = vault.to_json().map_err(|e| SecretError::CorruptVault {
        message: e.to_string(),
    })?;
    backend
        .set_password(VAULT_ACCOUNT, &json)
        .map_err(|e| SecretError::Backend {
            account: VAULT_ACCOUNT.to_string(),
            source: e,
        })
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
    /// Deepgram API key — used by transcript and speech-to-text services.
    pub const DEEPGRAM_API_KEY: &str = "deepgram_api_key";
    /// Pexels API key — used by the b-roll search/use tools (Phase 3).
    pub const PEXELS_API_KEY: &str = "pexels_api_key";
    /// OpenRouter API key — used by generated-media video providers.
    pub const OPENROUTER_API_KEY: &str = "openrouter_api_key";
    /// X API bearer token — used for trend/context reads.
    pub const X_BEARER_TOKEN: &str = "x_bearer_token";
}

/// Env-var names corresponding to [`accounts`]. Kept in lockstep.
pub mod env_vars {
    /// Override for [`super::accounts::HF_TOKEN`].
    pub const HF_TOKEN: &str = "HF_TOKEN";
    /// Override for [`super::accounts::ANTHROPIC_API_KEY`].
    pub const ANTHROPIC_API_KEY: &str = "ANTHROPIC_API_KEY";
    /// Override for [`super::accounts::DEEPGRAM_API_KEY`].
    pub const DEEPGRAM_API_KEY: &str = "DEEPGRAM_API_KEY";
    /// Override for [`super::accounts::PEXELS_API_KEY`].
    pub const PEXELS_API_KEY: &str = "PEXELS_API_KEY";
    /// Override for [`super::accounts::OPENROUTER_API_KEY`].
    pub const OPENROUTER_API_KEY: &str = "OPENROUTER_API_KEY";
    /// Override for [`super::accounts::X_BEARER_TOKEN`].
    pub const X_BEARER_TOKEN: &str = "X_BEARER_TOKEN";
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[derive(Default)]
    struct MemorySecretBackend {
        entries: RefCell<BTreeMap<String, String>>,
        fail_get: bool,
        fail_set: bool,
    }

    impl MemorySecretBackend {
        fn with_get_error() -> Self {
            Self {
                fail_get: true,
                ..Self::default()
            }
        }

        fn with_set_error() -> Self {
            Self {
                fail_set: true,
                ..Self::default()
            }
        }

        fn accounts(&self) -> Vec<String> {
            self.entries.borrow().keys().cloned().collect()
        }

        fn insert(&self, account: &str, value: &str) {
            self.entries
                .borrow_mut()
                .insert(account.to_string(), value.to_string());
        }
    }

    fn synthetic_keyring_error(operation: &str) -> keyring::Error {
        keyring::Error::Invalid("test_backend".to_string(), operation.to_string())
    }

    impl SecretBackend for MemorySecretBackend {
        fn get_password(&self, account: &str) -> Result<Option<String>, keyring::Error> {
            if self.fail_get {
                return Err(synthetic_keyring_error("get"));
            }
            Ok(self.entries.borrow().get(account).cloned())
        }

        fn set_password(&self, account: &str, value: &str) -> Result<(), keyring::Error> {
            if self.fail_set {
                return Err(synthetic_keyring_error("set"));
            }
            self.entries
                .borrow_mut()
                .insert(account.to_string(), value.to_string());
            Ok(())
        }
    }

    // The real env-var ↔ keychain interaction is exercised by manual smoke
    // (we deliberately don't write to the user's keychain from tests, and
    // mutating env in tests is `unsafe` in edition 2024 + multi-threaded by
    // the test runner's default).
    #[test]
    fn account_constants_are_distinct_from_env_var_names() {
        assert_ne!(accounts::HF_TOKEN, env_vars::HF_TOKEN);
        assert_ne!(accounts::ANTHROPIC_API_KEY, env_vars::ANTHROPIC_API_KEY);
        assert_ne!(accounts::DEEPGRAM_API_KEY, env_vars::DEEPGRAM_API_KEY);
        assert_ne!(accounts::PEXELS_API_KEY, env_vars::PEXELS_API_KEY);
        assert_ne!(accounts::OPENROUTER_API_KEY, env_vars::OPENROUTER_API_KEY);
        assert_ne!(accounts::X_BEARER_TOKEN, env_vars::X_BEARER_TOKEN);
    }

    #[test]
    fn service_name_is_stable() {
        // If anyone changes this, they need to migrate users. Loud test
        // catches it in review.
        assert_eq!(SERVICE, "montage");
    }

    #[test]
    fn legacy_service_name_is_read_fallback() {
        assert_eq!(LEGACY_SERVICE, "awidat");
        assert_eq!(secret_read_services(), ["montage", "awidat"]);
    }

    #[test]
    fn load_vault_returns_default_when_keychain_entry_missing() -> Result<(), SecretError> {
        let backend = MemorySecretBackend::default();
        let vault = load_vault_with_backend(&backend)?;
        assert_eq!(vault, SecretVault::default());
        Ok(())
    }

    #[test]
    fn save_then_load_vault_uses_single_keychain_account() -> Result<(), SecretError> {
        let backend = MemorySecretBackend::default();
        let mut vault = SecretVault::default();
        vault.set(accounts::DEEPGRAM_API_KEY, "dg-secret");
        save_vault_with_backend(&backend, &vault)?;
        assert_eq!(backend.accounts(), vec![VAULT_ACCOUNT.to_string()]);
        let loaded = load_vault_with_backend(&backend)?;
        assert_eq!(loaded.get(accounts::DEEPGRAM_API_KEY), Some("dg-secret"));
        Ok(())
    }

    #[test]
    fn load_vault_maps_malformed_json_to_corrupt_vault() {
        let backend = MemorySecretBackend::default();
        backend.insert(VAULT_ACCOUNT, "not-json");
        match load_vault_with_backend(&backend) {
            Err(SecretError::CorruptVault { message }) => {
                assert!(message.contains("expected ident"));
            }
            other => panic!("expected corrupt vault error, got {other:?}"),
        }
    }

    #[test]
    fn load_vault_maps_backend_get_error_to_vault_account() {
        let backend = MemorySecretBackend::with_get_error();
        match load_vault_with_backend(&backend) {
            Err(SecretError::Backend { account, .. }) => {
                assert_eq!(account, VAULT_ACCOUNT);
            }
            other => panic!("expected backend error, got {other:?}"),
        }
    }

    #[test]
    fn save_vault_maps_backend_set_error_to_vault_account() {
        let backend = MemorySecretBackend::with_set_error();
        let vault = SecretVault::default();
        match save_vault_with_backend(&backend, &vault) {
            Err(SecretError::Backend { account, .. }) => {
                assert_eq!(account, VAULT_ACCOUNT);
            }
            other => panic!("expected backend error, got {other:?}"),
        }
    }

    #[test]
    fn vault_round_trip_redacts_and_finds_provider() -> Result<(), serde_json::Error> {
        let mut vault = SecretVault::default();
        vault.set(accounts::OPENROUTER_API_KEY, "sk-openrouter");
        assert_eq!(
            vault.get(accounts::OPENROUTER_API_KEY),
            Some("sk-openrouter")
        );
        assert_eq!(
            vault.status(accounts::OPENROUTER_API_KEY),
            ProviderSecretStatus::Configured
        );
        let json = vault.to_json()?;
        let parsed = SecretVault::from_json(&json)?;
        assert_eq!(
            parsed.get(accounts::OPENROUTER_API_KEY),
            Some("sk-openrouter")
        );
        Ok(())
    }

    #[test]
    fn env_lookup_wins_over_vault_lookup() {
        let mut vault = SecretVault::default();
        vault.set(accounts::HF_TOKEN, "vault-hf");
        let resolved = resolve_from_env_or_vault(Some("env-hf"), &vault, accounts::HF_TOKEN);
        assert_eq!(resolved.as_deref(), Some("env-hf"));
    }

    #[test]
    fn empty_env_lookup_falls_back_to_vault() {
        let mut vault = SecretVault::default();
        vault.set(accounts::HF_TOKEN, "vault-hf");
        let resolved = resolve_from_env_or_vault(Some(""), &vault, accounts::HF_TOKEN);
        assert_eq!(resolved.as_deref(), Some("vault-hf"));
    }

    #[test]
    fn vault_debug_redacts_secret_values() {
        let mut vault = SecretVault::default();
        vault.set(accounts::OPENROUTER_API_KEY, "sk-openrouter");
        let debug = format!("{vault:?}");
        assert!(!debug.contains("sk-openrouter"));
        assert!(debug.contains("<redacted>"));
    }

    #[test]
    fn setting_empty_vault_value_removes_provider() {
        let mut vault = SecretVault::default();
        vault.set(accounts::OPENROUTER_API_KEY, "sk-openrouter");
        vault.set(accounts::OPENROUTER_API_KEY, "");
        assert_eq!(vault.get(accounts::OPENROUTER_API_KEY), None);
        assert_eq!(
            vault.status(accounts::OPENROUTER_API_KEY),
            ProviderSecretStatus::NotSet
        );
    }

    #[test]
    fn parsed_empty_vault_value_is_not_configured() -> Result<(), serde_json::Error> {
        let json = r#"{"version":1,"providers":{"openrouter_api_key":{"value":"","updated_at":"1970-01-01T00:00:00Z"}}}"#;
        let vault = SecretVault::from_json(json)?;
        assert_eq!(vault.get(accounts::OPENROUTER_API_KEY), None);
        assert_eq!(
            vault.status(accounts::OPENROUTER_API_KEY),
            ProviderSecretStatus::NotSet
        );
        Ok(())
    }
}
