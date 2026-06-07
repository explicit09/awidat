//! OS-keychain-backed secret store for publishing OAuth credentials.
//!
//! This module is the single read/write path for *secret* publishing
//! material: provider access tokens, refresh tokens, and the user's
//! BYO OAuth `client_secret`. Non-secret metadata (account display
//! name, expiry, `client_id`) keeps living in `publishing.json` next
//! to the rest of the app's flat-file config — only the secrets move
//! to the platform keystore.
//!
//! # Layout
//!
//! Each `(provider, token kind)` pair is one keychain entry under a
//! fixed service name:
//!
//! - service: `"montage.publishing"`
//! - account: `"<provider>-<kind>"` (e.g. `"youtube-access_token"`,
//!   `"tiktok-refresh_token"`, `"instagram-client_secret"`)
//!
//! Keeping each kind as its own entry — instead of one JSON blob per
//! provider — makes deletes precise (you can revoke just the refresh
//! token), keeps individual values small enough that every OS backend
//! accepts them, and reads as the obvious shape in
//! `Keychain Access.app` / `secretool` for users who go looking.
//!
//! # Backends
//!
//! The `keyring` crate dispatches to:
//!
//! - macOS — Keychain Services (`security-framework`).
//! - Windows — Credential Manager (`windows-sys`).
//! - Linux — kernel keyutils for the in-session cache + Secret Service
//!   (libsecret / gnome-keyring) for persistence across logout. We pull
//!   the `linux-native-async-persistent` feature so libsecret runs over
//!   the same Tokio reactor the rest of the app uses.
//! - FreeBSD/OpenBSD — `dbus-secret-service`.
//!
//! # Testing
//!
//! Tests install an in-process in-memory backend via
//! [`install_mock_backend`]. We don't use the `keyring::mock` builder
//! directly because each `Entry::new(...)` call there constructs a
//! fresh `MockCredential` — so a `store` followed by a `read` against
//! the same `(service, account)` doesn't round-trip.  Instead we hold
//! the test state in a `OnceLock<Mutex<HashMap>>` keyed by
//! `(service, account)` and route every `Keychain` call through it
//! when [`install_mock_backend`] has been called. Production builds
//! always go straight to the OS-native backend.

use std::fmt;

#[cfg(not(test))]
use keyring::Entry;

/// Stable service name registered with every OS backend. Constant per
/// app so the OS UI (Keychain Access on macOS, `seahorse` on Linux,
/// Credential Manager on Windows) groups every montage publishing entry
/// under the same namespace.
const SERVICE: &str = "montage.publishing";

/// Which slot inside a provider's credential set this entry maps to.
///
/// We split the secrets into separate entries so a partial revocation
/// (e.g. delete the refresh token only) is a one-line operation, and
/// so each value stays comfortably under every backend's per-entry
/// size budget (Windows Credential Manager caps at 2560 bytes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    /// Bearer token the provider REST API expects.
    AccessToken,
    /// Long-lived refresh token, when the provider issues one.
    RefreshToken,
    /// User-supplied OAuth-app secret from the BYO credentials flow.
    /// Pairs with the public `client_id` stored in `publishing.json`.
    ClientSecret,
}

impl TokenKind {
    /// Suffix appended after the provider key in the keychain account
    /// name. Lowercase-snake to match other montage keyring entries
    /// (`crates/secrets`).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AccessToken => "access_token",
            Self::RefreshToken => "refresh_token",
            Self::ClientSecret => "client_secret",
        }
    }
}

impl fmt::Display for TokenKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Errors talking to the OS keychain.
///
/// Wraps the underlying backend error with the failing
/// `(provider, kind)` so the log line / surfaced message tells the
/// user which slot blew up — "macOS Keychain denied access to
/// youtube-refresh_token" is a much more actionable error than
/// "keyring: NoStorageAccess".
#[derive(Debug)]
pub struct KeychainError {
    /// Provider key that triggered the error (`"youtube"`, …).
    pub provider: String,
    /// Token slot that failed.
    pub kind: TokenKind,
    /// Human-readable description of the underlying failure. We store
    /// a `String` rather than the raw `keyring::Error` so the same
    /// surface compiles in both `cfg(test)` and production builds
    /// without leaking the underlying backend type through the API.
    pub reason: String,
}

impl fmt::Display for KeychainError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "keychain error for {}-{}: {}",
            self.provider, self.kind, self.reason,
        )
    }
}

impl std::error::Error for KeychainError {}

/// Read/write handle to the OS keychain, scoped to the publishing
/// service. Cheap to construct (`Keychain::new` does no I/O) — callers
/// are encouraged to make one per call site rather than threading a
/// shared instance through every function signature.
#[derive(Debug, Clone)]
pub struct Keychain {
    service: &'static str,
}

impl Default for Keychain {
    fn default() -> Self {
        Self::new()
    }
}

impl Keychain {
    /// Construct using the canonical `"montage.publishing"` service name.
    pub fn new() -> Self {
        Self { service: SERVICE }
    }

    /// Service string this Keychain writes under. Exposed so tests
    /// (and the Settings UI's diagnostic copy) can render the same
    /// namespace the OS UI shows.
    #[allow(dead_code)]
    pub fn service(&self) -> &'static str {
        self.service
    }

    /// Format the account string the way every backend expects.
    /// Centralised so a future rename (e.g. switching the separator)
    /// is a one-line change instead of a `grep -r` cleanup.
    fn account(provider: &str, kind: TokenKind) -> String {
        format!("{provider}-{}", kind.as_str())
    }

    /// Persist `value` under `(provider, kind)`. Overwrites any
    /// existing entry — the OAuth flow re-runs over completed slots
    /// when the user reconnects, so a stale stored value must yield to
    /// the fresh one without an explicit delete.
    pub fn store_token(
        &self,
        provider: &str,
        kind: TokenKind,
        value: &str,
    ) -> Result<(), KeychainError> {
        let account = Self::account(provider, kind);
        backend::store(self.service, &account, value).map_err(|reason| KeychainError {
            provider: provider.to_string(),
            kind,
            reason,
        })
    }

    /// Read the value at `(provider, kind)`. Missing entries are not
    /// an error — they collapse to `Ok(None)` so callers can distinguish
    /// "not yet stored" from "backend exploded".
    pub fn read_token(
        &self,
        provider: &str,
        kind: TokenKind,
    ) -> Result<Option<String>, KeychainError> {
        let account = Self::account(provider, kind);
        backend::read(self.service, &account).map_err(|reason| KeychainError {
            provider: provider.to_string(),
            kind,
            reason,
        })
    }

    /// Delete the entry at `(provider, kind)`. Idempotent — missing
    /// entries return `Ok(())` so callers don't need to special-case
    /// "first time disconnecting" against "second time disconnecting".
    pub fn delete_token(&self, provider: &str, kind: TokenKind) -> Result<(), KeychainError> {
        let account = Self::account(provider, kind);
        backend::delete(self.service, &account).map_err(|reason| KeychainError {
            provider: provider.to_string(),
            kind,
            reason,
        })
    }

    /// Convenience: delete every OAuth-issued token slot for a provider
    /// (access + refresh). Used by `disconnect` to clear the keychain
    /// half of the credential pair while preserving the user's BYO
    /// `client_secret` so they don't have to re-paste it.
    ///
    /// Returns the first error encountered; subsequent slots are still
    /// attempted so a half-clean state isn't left behind.
    pub fn delete_oauth_tokens(&self, provider: &str) -> Result<(), KeychainError> {
        let access = self.delete_token(provider, TokenKind::AccessToken);
        let refresh = self.delete_token(provider, TokenKind::RefreshToken);
        access.and(refresh)
    }
}

// ---- Backend dispatch ----
//
// Production builds talk to the OS keychain via the `keyring` crate.
// Test builds talk to an in-process HashMap keyed by `(service,
// account)` so that store-then-read round-trips work reliably without
// hitting the developer's real keychain (which would otherwise
// trigger Keychain Access permission prompts on every test run).

#[cfg(not(test))]
mod backend {
    use super::Entry;

    pub fn store(service: &str, account: &str, value: &str) -> Result<(), String> {
        let entry = Entry::new(service, account).map_err(|e| e.to_string())?;
        entry.set_password(value).map_err(|e| e.to_string())
    }

    pub fn read(service: &str, account: &str) -> Result<Option<String>, String> {
        let entry = Entry::new(service, account).map_err(|e| e.to_string())?;
        match entry.get_password() {
            Ok(v) => Ok(Some(v)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }

    pub fn delete(service: &str, account: &str) -> Result<(), String> {
        let entry = Entry::new(service, account).map_err(|e| e.to_string())?;
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
}

#[cfg(test)]
mod backend {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};

    fn store_map() -> &'static Mutex<HashMap<String, String>> {
        static STORE: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
        STORE.get_or_init(|| Mutex::new(HashMap::new()))
    }

    fn key(service: &str, account: &str) -> String {
        format!("{service}::{account}")
    }

    pub fn store(service: &str, account: &str, value: &str) -> Result<(), String> {
        let mut guard = store_map().lock().map_err(|e| e.to_string())?;
        guard.insert(key(service, account), value.to_string());
        Ok(())
    }

    pub fn read(service: &str, account: &str) -> Result<Option<String>, String> {
        let guard = store_map().lock().map_err(|e| e.to_string())?;
        Ok(guard.get(&key(service, account)).cloned())
    }

    pub fn delete(service: &str, account: &str) -> Result<(), String> {
        let mut guard = store_map().lock().map_err(|e| e.to_string())?;
        guard.remove(&key(service, account));
        Ok(())
    }
}

/// Install the in-process mock backend for the duration of the test
/// process. No-op outside `#[cfg(test)]` — production builds always
/// go straight to the OS keychain.
///
/// Idempotent. Tests that interact with the keychain should call this
/// before any [`Keychain`] operation as a defensive marker so the test
/// fails loudly if it ever runs against the real backend (e.g. an
/// accidental `cargo test --release` that disables `cfg(test)` for
/// the binary half).
#[cfg(test)]
pub fn install_mock_backend() {
    // The cfg(test) backend is the only thing the test binary links
    // against, so calling this is the explicit "yes I want a mock"
    // signal — it doesn't need to flip a switch.
}

#[cfg(not(test))]
#[allow(dead_code)]
pub fn install_mock_backend() {
    panic!("install_mock_backend is only available in cfg(test) builds");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Generate a unique provider key for each test so parallel tests
    /// don't observe each other's writes via the shared (process-wide)
    /// mock backend.
    fn unique_provider(prefix: &str) -> String {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        format!(
            "{prefix}-{}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
            SEQ.fetch_add(1, Ordering::Relaxed),
        )
    }

    #[test]
    fn store_then_read_round_trips() {
        install_mock_backend();
        let kc = Keychain::new();
        let provider = unique_provider("rt");
        kc.store_token(&provider, TokenKind::AccessToken, "ya29.access_xyz")
            .expect("store ok");
        let got = kc
            .read_token(&provider, TokenKind::AccessToken)
            .expect("read ok");
        assert_eq!(got.as_deref(), Some("ya29.access_xyz"));
    }

    #[test]
    fn read_missing_entry_returns_none() {
        install_mock_backend();
        let kc = Keychain::new();
        let provider = unique_provider("missing");
        let got = kc
            .read_token(&provider, TokenKind::RefreshToken)
            .expect("read ok");
        assert_eq!(got, None);
    }

    #[test]
    fn store_overwrites_existing_value() {
        // Re-running OAuth must replace a stale access token rather
        // than leaving it untouched / requiring a manual delete.
        install_mock_backend();
        let kc = Keychain::new();
        let provider = unique_provider("overwrite");
        kc.store_token(&provider, TokenKind::AccessToken, "v1")
            .expect("store v1");
        kc.store_token(&provider, TokenKind::AccessToken, "v2")
            .expect("store v2");
        let got = kc
            .read_token(&provider, TokenKind::AccessToken)
            .expect("read");
        assert_eq!(got.as_deref(), Some("v2"));
    }

    #[test]
    fn delete_clears_entry() {
        install_mock_backend();
        let kc = Keychain::new();
        let provider = unique_provider("del");
        kc.store_token(&provider, TokenKind::AccessToken, "to-be-deleted")
            .expect("store");
        kc.delete_token(&provider, TokenKind::AccessToken)
            .expect("delete");
        let got = kc
            .read_token(&provider, TokenKind::AccessToken)
            .expect("read after delete");
        assert_eq!(got, None);
    }

    #[test]
    fn delete_missing_entry_is_ok() {
        // Idempotency contract — disconnect → disconnect must not blow up.
        install_mock_backend();
        let kc = Keychain::new();
        let provider = unique_provider("del-missing");
        kc.delete_token(&provider, TokenKind::AccessToken)
            .expect("delete missing");
    }

    #[test]
    fn isolated_per_provider() {
        // youtube's access token and tiktok's access token must not
        // collide in storage. The (provider, kind) tuple is the
        // primary key — the test asserts both halves matter.
        install_mock_backend();
        let kc = Keychain::new();
        let yt = unique_provider("yt-iso");
        let tk = unique_provider("tk-iso");
        kc.store_token(&yt, TokenKind::AccessToken, "youtube-secret")
            .expect("store yt");
        kc.store_token(&tk, TokenKind::AccessToken, "tiktok-secret")
            .expect("store tt");
        assert_eq!(
            kc.read_token(&yt, TokenKind::AccessToken)
                .expect("read yt")
                .as_deref(),
            Some("youtube-secret"),
        );
        assert_eq!(
            kc.read_token(&tk, TokenKind::AccessToken)
                .expect("read tt")
                .as_deref(),
            Some("tiktok-secret"),
        );
    }

    #[test]
    fn isolated_per_token_kind() {
        // Access vs refresh under the same provider are separate slots.
        install_mock_backend();
        let kc = Keychain::new();
        let provider = unique_provider("kinds");
        kc.store_token(&provider, TokenKind::AccessToken, "access-v")
            .expect("store access");
        kc.store_token(&provider, TokenKind::RefreshToken, "refresh-v")
            .expect("store refresh");
        kc.store_token(&provider, TokenKind::ClientSecret, "client-v")
            .expect("store client_secret");
        assert_eq!(
            kc.read_token(&provider, TokenKind::AccessToken)
                .expect("a")
                .as_deref(),
            Some("access-v"),
        );
        assert_eq!(
            kc.read_token(&provider, TokenKind::RefreshToken)
                .expect("r")
                .as_deref(),
            Some("refresh-v"),
        );
        assert_eq!(
            kc.read_token(&provider, TokenKind::ClientSecret)
                .expect("c")
                .as_deref(),
            Some("client-v"),
        );
    }

    #[test]
    fn delete_oauth_tokens_preserves_client_secret() {
        // Disconnect contract: revoke access + refresh but leave the
        // user's BYO client_secret in place so reconnect doesn't force
        // them to re-paste it.
        install_mock_backend();
        let kc = Keychain::new();
        let provider = unique_provider("disconn");
        kc.store_token(&provider, TokenKind::AccessToken, "a")
            .expect("a");
        kc.store_token(&provider, TokenKind::RefreshToken, "r")
            .expect("r");
        kc.store_token(&provider, TokenKind::ClientSecret, "c")
            .expect("c");
        kc.delete_oauth_tokens(&provider).expect("clear oauth");
        assert!(
            kc.read_token(&provider, TokenKind::AccessToken)
                .expect("read a")
                .is_none(),
        );
        assert!(
            kc.read_token(&provider, TokenKind::RefreshToken)
                .expect("read r")
                .is_none(),
        );
        assert_eq!(
            kc.read_token(&provider, TokenKind::ClientSecret)
                .expect("read c")
                .as_deref(),
            Some("c"),
        );
    }

    #[test]
    fn token_kind_string_is_stable() {
        // These suffixes become permanent keychain account names — a
        // rename would orphan existing user entries.
        assert_eq!(TokenKind::AccessToken.as_str(), "access_token");
        assert_eq!(TokenKind::RefreshToken.as_str(), "refresh_token");
        assert_eq!(TokenKind::ClientSecret.as_str(), "client_secret");
    }

    #[test]
    fn service_name_is_stable() {
        // Same reasoning as `crates/secrets`'s service-name test: a
        // rename here strands every existing user's stored credentials.
        let kc = Keychain::new();
        assert_eq!(kc.service(), "montage.publishing");
    }
}
