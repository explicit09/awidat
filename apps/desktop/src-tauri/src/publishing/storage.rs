//! Flat-file credential store for the publishing providers.
//!
//! Real product code should store these in the OS keychain — see the
//! TODO at the bottom of [`store_path`] — but for the W5 scaffolding
//! phase we accept the trade-off: JSON on disk lets the user inspect
//! what's there, and means a missing/corrupt file fails the same way
//! across providers.
//!
//! # On-disk shape
//!
//! ```jsonc
//! {
//!   "youtube":   { "access_token": "...", "refresh_token": "...",
//!                  "account_name": "you@gmail.com", "expires_at": 1717123456,
//!                  "client_credentials": { "client_id": "…", "client_secret": "…" } },
//!   "tiktok":    null,
//!   "instagram": { "access_token": "",
//!                  "client_credentials": { "client_id": "…", "client_secret": "…" } }
//! }
//! ```
//!
//! `null` is "no creds yet" — distinguishable from a key being absent
//! entirely (which would mean "we don't know this provider"). Each
//! provider key is independently optional so we never have to mutate
//! more than one slot per write.
//!
//! `client_credentials` (W5.A5) carries the user's developer-console
//! credentials (BYO OAuth app). It can be present *without* an access
//! token — that's the state right after the user pastes a `client_id`
//! / `client_secret` but before they complete the OAuth flow. Disconnect
//! clears `access_token` + refresh + account + expiry but preserves
//! `client_credentials` so the user doesn't have to re-paste them.
//!
//! ⚠ SECURITY: `client_secret` and `access_token` live in the same
//! plain-JSON file. Keychain integration is deferred to Wave 6 — the
//! Settings UI surfaces this warning to the user.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::errors::ProviderError;

/// File name under `<config_dir>/awidat/`.
const FILE_NAME: &str = "publishing.json";

/// User-supplied OAuth app credentials (W5.A5 BYO credentials).
///
/// These are the values the user gets from a platform's developer
/// console (Google Cloud Console for YouTube, the TikTok devs portal,
/// the Meta developers app dashboard). They feed into the `client_id`
/// query param on `begin_oauth` URLs and into the `client_secret`
/// half of the token-exchange POST.
///
/// Both fields are required for a real OAuth flow — the `Option`
/// shape on the parent struct is the all-or-nothing toggle.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClientCredentials {
    /// The platform-specific OAuth app identifier (Google calls this
    /// `client_id`, TikTok calls it `client_key`, Meta calls it `app_id`).
    /// We use `client_id` as the canonical name.
    pub client_id: String,
    /// The companion secret. ⚠ Stored in plain JSON for now.
    pub client_secret: String,
}

/// Per-provider credential blob.
///
/// Fields stay `Option`-typed because providers vary in what they
/// return (TikTok doesn't issue refresh tokens for some app types,
/// Instagram's long-lived tokens have no `expires_at`).
///
/// `access_token == ""` is the canonical "not authenticated" state.
/// We keep the slot around in that case so `client_credentials` (the
/// user's BYO OAuth app) survives a disconnect.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Credentials {
    /// Bearer token the provider's REST API expects.
    #[serde(default)]
    pub access_token: String,
    /// Refresh token, when the provider issues one.
    #[serde(default)]
    pub refresh_token: Option<String>,
    /// e.g. `"you@gmail.com"` or `"@handle"`.
    #[serde(default)]
    pub account_name: Option<String>,
    /// Unix epoch seconds at which `access_token` stops being valid.
    #[serde(default)]
    pub expires_at: Option<i64>,
    /// User's BYO OAuth app credentials (Settings → Publishing → BYO).
    /// Independent of the access token — survives disconnect.
    #[serde(default)]
    pub client_credentials: Option<ClientCredentials>,
}

impl Credentials {
    /// `true` once the slot holds a usable access token. Empty-string
    /// is the canonical "client credentials registered but user hasn't
    /// completed OAuth" state.
    pub fn has_access_token(&self) -> bool {
        !self.access_token.is_empty()
    }

    /// Clear the OAuth-issued fields, preserving `client_credentials`.
    /// Returns `true` if anything was cleared.
    pub fn clear_tokens(&mut self) -> bool {
        let had_tokens = self.has_access_token()
            || self.refresh_token.is_some()
            || self.account_name.is_some()
            || self.expires_at.is_some();
        self.access_token.clear();
        self.refresh_token = None;
        self.account_name = None;
        self.expires_at = None;
        had_tokens
    }
}

/// Whole-file shape. A `HashMap<String, Option<Credentials>>` lets
/// each slot stay independently null, while leaving room for future
/// providers without forcing a schema migration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PublishingStore {
    /// `provider_key → credentials | null`.
    #[serde(flatten)]
    pub providers: HashMap<String, Option<Credentials>>,
}

impl PublishingStore {
    /// Read the credential slot for one provider key — returns the
    /// raw blob, which may carry only `client_credentials` (no access
    /// token) if the user has pasted BYO credentials but not yet
    /// completed OAuth. Callers that want "is this actually
    /// authenticated?" should additionally check
    /// [`Credentials::has_access_token`].
    ///
    /// Returns `None` when the slot is absent, explicitly `null`, or
    /// the file is missing.
    pub fn get(&self, key: &str) -> Option<&Credentials> {
        self.providers.get(key).and_then(|slot| slot.as_ref())
    }

    /// Read the slot only if it carries a usable access token. This
    /// is the post-W5.A5 successor to bare `get(...).is_some()` —
    /// callers asking "is this provider authenticated?" go through
    /// here so a BYO-only slot isn't mistaken for connected.
    pub fn get_authenticated(&self, key: &str) -> Option<&Credentials> {
        self.get(key).filter(|c| c.has_access_token())
    }

    /// Write (or clear, when `creds` is `None`) one provider's slot.
    pub fn set(&mut self, key: &str, creds: Option<Credentials>) {
        self.providers.insert(key.to_string(), creds);
    }

    /// Mutable handle to an existing slot's `Credentials`. Used by the
    /// BYO-credentials path so the user can paste `client_id` /
    /// `client_secret` before completing OAuth without us having to
    /// fabricate a placeholder access token.
    ///
    /// Inserts a fresh `Credentials::default()` if the slot is absent
    /// or null, then hands back `&mut` so the caller can mutate fields
    /// directly.
    pub fn get_or_insert(&mut self, key: &str) -> &mut Credentials {
        self.providers
            .entry(key.to_string())
            .or_insert_with(|| Some(Credentials::default()))
            .get_or_insert_with(Credentials::default)
    }
}

/// Resolve `<config_dir>/awidat/publishing.json`.
///
/// On macOS this is `~/Library/Application Support/awidat/publishing.json`,
/// on Linux `~/.config/awidat/publishing.json`, on Windows
/// `%APPDATA%\awidat\publishing.json`.
///
/// TODO(W5.A2+): move tokens to the OS keychain
/// (`keyring::Entry::new("awidat", "<provider>")`) and keep this file
/// for the non-secret bits only (account name, expiry).
pub fn default_store_path() -> Result<PathBuf, ProviderError> {
    let cfg = dirs::config_dir().ok_or_else(|| {
        ProviderError::Io("could not resolve platform config_dir".into())
    })?;
    Ok(cfg.join("awidat").join(FILE_NAME))
}

/// Load from an explicit path. Missing file → empty store (no error).
pub async fn load_from(path: &std::path::Path) -> Result<PublishingStore, ProviderError> {
    match tokio::fs::read_to_string(path).await {
        Ok(raw) => serde_json::from_str(&raw)
            .map_err(|e| ProviderError::Io(format!("parse {}: {e}", path.display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(PublishingStore::default()),
        Err(e) => Err(ProviderError::Io(format!(
            "read {}: {e}",
            path.display()
        ))),
    }
}

/// Persist the store to an explicit path. Creates parent dirs and
/// writes atomically (tempfile + rename) so a crash mid-write leaves
/// either the old file or the new one — never a half-written one.
pub async fn save_to(
    path: &std::path::Path,
    store: &PublishingStore,
) -> Result<(), ProviderError> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(|e| {
            ProviderError::Io(format!("create dir {}: {e}", parent.display()))
        })?;
    }
    let buf = serde_json::to_vec_pretty(store)
        .map_err(|e| ProviderError::Io(format!("serialize store: {e}")))?;
    // Same-directory tempfile so the rename is a filesystem-level swap.
    let tmp = path.with_extension("json.tmp");
    tokio::fs::write(&tmp, &buf)
        .await
        .map_err(|e| ProviderError::Io(format!("write {}: {e}", tmp.display())))?;
    tokio::fs::rename(&tmp, path)
        .await
        .map_err(|e| ProviderError::Io(format!("rename {} → {}: {e}", tmp.display(), path.display())))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_creds() -> Credentials {
        Credentials {
            access_token: "ya29.access_xyz".into(),
            refresh_token: Some("1//refresh_abc".into()),
            account_name: Some("you@gmail.com".into()),
            expires_at: Some(1_717_123_456),
            client_credentials: None,
        }
    }

    #[tokio::test]
    async fn load_returns_empty_when_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("publishing.json");
        let store = load_from(&path).await.unwrap();
        assert!(store.providers.is_empty());
        assert!(store.get("youtube").is_none());
    }

    #[tokio::test]
    async fn storage_round_trip() {
        // The named-test from the brief: read/write of credentials.
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("publishing.json");

        let mut store = PublishingStore::default();
        store.set("youtube", Some(sample_creds()));
        store.set("tiktok", None);
        save_to(&path, &store).await.unwrap();

        let reloaded = load_from(&path).await.unwrap();
        assert_eq!(reloaded.get("youtube"), Some(&sample_creds()));
        // `null` slot serialises distinct from "absent": it should
        // round-trip through the map as Some(None).
        assert!(reloaded.providers.contains_key("tiktok"));
        assert!(reloaded.get("tiktok").is_none());
        // A never-set key stays absent.
        assert!(!reloaded.providers.contains_key("instagram"));
        assert!(reloaded.get("instagram").is_none());
    }

    #[tokio::test]
    async fn save_creates_parent_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let nested = tmp.path().join("a").join("b").join("publishing.json");
        let store = PublishingStore::default();
        save_to(&nested, &store).await.unwrap();
        assert!(nested.is_file());
    }

    #[tokio::test]
    async fn save_is_atomic_no_tempfile_leak() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("publishing.json");
        let mut store = PublishingStore::default();
        store.set("youtube", Some(sample_creds()));
        save_to(&path, &store).await.unwrap();
        // tempfile must be cleaned up.
        let tmp_path = path.with_extension("json.tmp");
        assert!(!tmp_path.exists(), "tempfile leaked after rename");
    }

    #[tokio::test]
    async fn load_returns_io_error_on_malformed_json() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("publishing.json");
        tokio::fs::write(&path, "{not json").await.unwrap();
        let err = load_from(&path).await.unwrap_err();
        assert_eq!(err.kind(), "io");
    }

    #[tokio::test]
    async fn old_format_without_client_credentials_round_trips() {
        // Existing publishing.json files (W5.A1-A4 era) had no
        // `client_credentials` field. We must continue to parse them
        // — the field is `#[serde(default)]` so an absent field
        // deserialises to `None`.
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("publishing.json");
        let legacy = r#"{
            "youtube": {
                "access_token": "ya29.legacy",
                "refresh_token": "r",
                "account_name": "old@gmail.com",
                "expires_at": 1
            }
        }"#;
        tokio::fs::write(&path, legacy).await.unwrap();
        let store = load_from(&path).await.unwrap();
        let creds = store.get("youtube").expect("youtube slot present");
        assert_eq!(creds.access_token, "ya29.legacy");
        assert!(
            creds.client_credentials.is_none(),
            "legacy file has no BYO creds",
        );
    }

    #[test]
    fn has_access_token_distinguishes_empty_from_real() {
        let mut c = Credentials::default();
        assert!(!c.has_access_token());
        c.access_token = "real".into();
        assert!(c.has_access_token());
    }

    #[test]
    fn clear_tokens_preserves_client_credentials() {
        let mut c = Credentials {
            access_token: "tok".into(),
            refresh_token: Some("r".into()),
            account_name: Some("a".into()),
            expires_at: Some(1),
            client_credentials: Some(ClientCredentials {
                client_id: "id".into(),
                client_secret: "secret".into(),
            }),
        };
        assert!(c.clear_tokens(), "had tokens to clear");
        assert!(c.access_token.is_empty());
        assert!(c.refresh_token.is_none());
        assert!(c.account_name.is_none());
        assert!(c.expires_at.is_none());
        // Client credentials survive.
        assert_eq!(
            c.client_credentials.as_ref().map(|cc| cc.client_id.as_str()),
            Some("id"),
        );
        // Idempotent.
        assert!(!c.clear_tokens(), "second clear is a no-op");
    }

    #[tokio::test]
    async fn get_or_insert_creates_default_slot() {
        let mut store = PublishingStore::default();
        let slot = store.get_or_insert("youtube");
        assert!(slot.access_token.is_empty());
        slot.client_credentials = Some(ClientCredentials {
            client_id: "cid".into(),
            client_secret: "csec".into(),
        });
        // A slot with only client_credentials is *not* authenticated.
        assert!(store.get_authenticated("youtube").is_none());
        // But it's still readable via the raw get.
        assert!(store.get("youtube").is_some());
    }

    #[test]
    fn default_store_path_is_under_awidat_namespace() {
        // We can't assert the absolute platform path (it varies), but
        // we can assert the file ends up under `awidat/publishing.json`.
        let p = default_store_path().unwrap();
        assert_eq!(p.file_name().unwrap(), "publishing.json");
        assert_eq!(
            p.parent().unwrap().file_name().unwrap(),
            "awidat",
            "publishing.json must live in the awidat config namespace",
        );
    }
}
