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
//!                  "account_name": "you@gmail.com", "expires_at": 1717123456 },
//!   "tiktok":    null,
//!   "instagram": null
//! }
//! ```
//!
//! `null` is "no creds yet" — distinguishable from a key being absent
//! entirely (which would mean "we don't know this provider"). Each
//! provider key is independently optional so we never have to mutate
//! more than one slot per write.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::errors::ProviderError;

/// File name under `<config_dir>/awidat/`.
const FILE_NAME: &str = "publishing.json";

/// Per-provider credential blob.
///
/// Fields stay `Option`-typed because providers vary in what they
/// return (TikTok doesn't issue refresh tokens for some app types,
/// Instagram's long-lived tokens have no `expires_at`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Credentials {
    /// Bearer token the provider's REST API expects.
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
    /// Read the credential for one provider key. Returns `None` when
    /// the slot is absent, explicitly `null`, or the file is missing.
    pub fn get(&self, key: &str) -> Option<&Credentials> {
        self.providers.get(key).and_then(|slot| slot.as_ref())
    }

    /// Write (or clear, when `creds` is `None`) one provider's slot.
    pub fn set(&mut self, key: &str, creds: Option<Credentials>) {
        self.providers.insert(key.to_string(), creds);
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
