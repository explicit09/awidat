//! Metadata-only flat-file store for the publishing providers.
//!
//! As of W6.A4 every *secret* (access token, refresh token, BYO
//! `client_secret`) lives in the OS keychain — see
//! [`super::keychain`]. This file persists only non-secret metadata:
//!
//! - `account_name` — display label ("you@gmail.com", "@handle").
//! - `expires_at` — Unix-epoch seconds at which the stored access
//!   token stops being valid; renderable in Settings
//!   without ever touching the secret itself.
//! - `client_id` — the user's BYO OAuth-app public identifier.
//!   Not secret (it ships in every authorize URL the
//!   user opens) so plain JSON is fine.
//! - `migrated_at` — set the first time this slot is rewritten by the
//!   keychain path. Lets us short-circuit the legacy
//!   migration on subsequent loads.
//!
//! # On-disk shape
//!
//! ```jsonc
//! {
//!   "youtube":   { "account_name": "you@gmail.com", "expires_at": 1717123456,
//!                  "client_id": "…apps.googleusercontent.com",
//!                  "migrated_at": 1717123000 },
//!   "tiktok":    null,
//!   "instagram": { "client_id": "…", "migrated_at": 1717123000 }
//! }
//! ```
//!
//! `null` is "no creds yet" — distinguishable from a key being absent
//! entirely (which would mean "we don't know this provider").
//!
//! # Legacy migration
//!
//! Pre-W6.A4 files inlined `access_token` / `refresh_token` /
//! `client_credentials.client_secret`. [`load_from`] detects those
//! fields on read, moves the secrets into the keychain via
//! [`super::keychain::Keychain`], stamps `migrated_at`, and returns the
//! cleaned-up shape. The next [`save_to`] then writes the secret-free
//! JSON so subsequent loads skip the migration path entirely.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::errors::ProviderError;
use super::keychain::{Keychain, KeychainError, TokenKind};

/// File name under `<config_dir>/montage/`.
const FILE_NAME: &str = "publishing.json";

/// Per-provider metadata blob. *No secret material* lives in this
/// struct — secrets are in the OS keychain, keyed by
/// `(provider, kind)`.
///
/// The struct stays open-ended (`#[serde(default)]` on every field) so
/// future metadata additions don't break existing files.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Credentials {
    /// User-facing label such as `"you@gmail.com"` or `"@handle"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_name: Option<String>,
    /// Unix epoch seconds at which the stored access token stops
    /// being valid. Renderable in Settings without reading the secret.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,
    /// User's BYO OAuth-app public identifier (Google calls this
    /// `client_id`, TikTok `client_key`, Meta `app_id`). Public by
    /// construction — it ships in every authorize URL the user opens
    /// in a browser. The companion `client_secret` lives in the
    /// keychain under [`TokenKind::ClientSecret`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    /// Unix epoch seconds at which this slot was last rewritten via
    /// the keychain path. Set once a migration completes (or a fresh
    /// OAuth flow finishes); used to short-circuit redundant legacy
    /// rewrites on every load.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub migrated_at: Option<i64>,
}

impl Credentials {
    /// `true` once the user has pasted a BYO `client_id`. The
    /// `client_secret` half is checked separately against the keychain.
    pub fn has_client_id(&self) -> bool {
        self.client_id
            .as_deref()
            .map(|s| !s.is_empty())
            .unwrap_or(false)
    }

    /// Clear the OAuth-issued metadata while preserving the BYO
    /// `client_id`. Returns `true` if anything was cleared.
    ///
    /// Note: callers must additionally delete the keychain entries
    /// (access + refresh) — this struct knows nothing about the secret
    /// half. See [`super::oauth::disconnect_provider`].
    pub fn clear_oauth_metadata(&mut self) -> bool {
        let had = self.account_name.is_some() || self.expires_at.is_some();
        self.account_name = None;
        self.expires_at = None;
        had
    }
}

/// Whole-file shape — `provider_key → metadata | null`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PublishingStore {
    #[serde(flatten)]
    pub providers: HashMap<String, Option<Credentials>>,
}

impl PublishingStore {
    /// Read the metadata for one provider key. Returns `None` when
    /// the slot is absent, explicitly `null`, or the file is missing.
    pub fn get(&self, key: &str) -> Option<&Credentials> {
        self.providers.get(key).and_then(|slot| slot.as_ref())
    }

    /// Write (or clear, when `creds` is `None`) one provider's slot.
    pub fn set(&mut self, key: &str, creds: Option<Credentials>) {
        self.providers.insert(key.to_string(), creds);
    }

    /// Mutable handle to an existing slot's [`Credentials`]. Inserts a
    /// fresh `Credentials::default()` when the slot is absent / null
    /// so callers (OAuth completion, BYO paste, …) can mutate fields
    /// directly without dancing around `Option`s.
    pub fn get_or_insert(&mut self, key: &str) -> &mut Credentials {
        self.providers
            .entry(key.to_string())
            .or_insert_with(|| Some(Credentials::default()))
            .get_or_insert_with(Credentials::default)
    }
}

/// Resolve `<config_dir>/montage/publishing.json`.
pub fn default_store_path() -> Result<PathBuf, ProviderError> {
    let cfg = dirs::config_dir()
        .ok_or_else(|| ProviderError::Io("could not resolve platform config_dir".into()))?;
    Ok(cfg.join("montage").join(FILE_NAME))
}

// ---- Raw deserialization shape: tolerates legacy plaintext fields ----
//
// We can't use `Credentials` directly for deserialization because
// pre-W6.A4 files inlined secrets and we want to keep loading those
// files (and migrate their secrets into the keychain) instead of
// erroring out the moment a user upgrades.

#[derive(Debug, Default, Deserialize)]
struct RawClientCredentials {
    #[serde(default)]
    client_id: String,
    #[serde(default)]
    client_secret: String,
}

#[derive(Debug, Default, Deserialize)]
struct RawCredentials {
    // New metadata fields (post-migration).
    #[serde(default)]
    account_name: Option<String>,
    #[serde(default)]
    expires_at: Option<i64>,
    #[serde(default)]
    client_id: Option<String>,
    #[serde(default)]
    migrated_at: Option<i64>,
    // Legacy secret fields (pre-W6.A4). Captured here so we can move
    // them to the keychain on load and strip from the saved shape.
    #[serde(default)]
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    client_credentials: Option<RawClientCredentials>,
}

#[derive(Debug, Default, Deserialize)]
struct RawStore {
    #[serde(flatten)]
    providers: HashMap<String, Option<RawCredentials>>,
}

/// Load from an explicit path. Missing file → empty store (no error).
///
/// Triggers the W6.A4 legacy-migration pass: any plaintext secrets
/// found on disk are moved into the keychain (via [`Keychain`]) and
/// stripped from the in-memory store. The caller is expected to call
/// [`save_to`] afterwards to persist the cleaned shape; we deliberately
/// don't write the file from `load_from` because read-only callers
/// (Settings status polling) shouldn't be triggering disk writes.
pub async fn load_from(path: &std::path::Path) -> Result<PublishingStore, ProviderError> {
    load_with_keychain(path, &Keychain::new()).await
}

/// Variant of [`load_from`] that takes an explicit [`Keychain`]. Used
/// by tests that install the mock backend before exercising the
/// migration path.
pub async fn load_with_keychain(
    path: &std::path::Path,
    keychain: &Keychain,
) -> Result<PublishingStore, ProviderError> {
    let raw_str = match tokio::fs::read_to_string(path).await {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(PublishingStore::default()),
        Err(e) => return Err(ProviderError::Io(format!("read {}: {e}", path.display()))),
    };
    let raw: RawStore = montage_proto::serde_robust::from_json_str(&raw_str)
        .map_err(|e| ProviderError::Io(format!("parse {}: {e}", path.display())))?;

    let mut providers: HashMap<String, Option<Credentials>> =
        HashMap::with_capacity(raw.providers.len());
    for (key, slot) in raw.providers {
        let migrated = slot.map(|raw| migrate_slot(&key, raw, keychain));
        providers.insert(key, migrated);
    }
    Ok(PublishingStore { providers })
}

/// Migrate one raw slot. Pure when no legacy fields are present —
/// just maps `RawCredentials → Credentials`. When legacy plaintext
/// secrets are present, writes each to the keychain (best-effort: a
/// failed write logs a warning and leaves the legacy field in place
/// so the next save preserves it).
fn migrate_slot(provider: &str, raw: RawCredentials, keychain: &Keychain) -> Credentials {
    let already_migrated = raw.migrated_at.is_some();
    let mut creds = Credentials {
        account_name: raw.account_name,
        expires_at: raw.expires_at,
        // Prefer the new flat `client_id` field; fall back to the
        // legacy `client_credentials.client_id` if only that's present.
        client_id: raw.client_id.or_else(|| {
            raw.client_credentials
                .as_ref()
                .map(|cc| cc.client_id.clone())
                .filter(|s| !s.is_empty())
        }),
        migrated_at: raw.migrated_at,
    };

    let mut wrote_any = false;
    let mut wrote_all = true;

    if !raw.access_token.is_empty() {
        match keychain.store_token(provider, TokenKind::AccessToken, &raw.access_token) {
            Ok(()) => {
                wrote_any = true;
                tracing::info!(
                    provider,
                    "migrated legacy plaintext access_token into OS keychain",
                );
            }
            Err(e) => {
                wrote_all = false;
                tracing::warn!(
                    provider,
                    error = %e,
                    "keychain write failed during migration — keeping legacy plaintext until next attempt",
                );
            }
        }
    }
    if let Some(refresh) = raw.refresh_token.as_deref().filter(|s| !s.is_empty()) {
        match keychain.store_token(provider, TokenKind::RefreshToken, refresh) {
            Ok(()) => {
                wrote_any = true;
                tracing::info!(
                    provider,
                    "migrated legacy plaintext refresh_token into OS keychain",
                );
            }
            Err(e) => {
                wrote_all = false;
                tracing::warn!(
                    provider,
                    error = %e,
                    "keychain refresh_token write failed during migration",
                );
            }
        }
    }
    if let Some(cc) = raw.client_credentials.as_ref() {
        if !cc.client_secret.is_empty() {
            match keychain.store_token(provider, TokenKind::ClientSecret, &cc.client_secret) {
                Ok(()) => {
                    wrote_any = true;
                    tracing::info!(
                        provider,
                        "migrated legacy plaintext client_secret into OS keychain",
                    );
                }
                Err(e) => {
                    wrote_all = false;
                    tracing::warn!(
                        provider,
                        error = %e,
                        "keychain client_secret write failed during migration",
                    );
                }
            }
        }
    }

    // Stamp migrated_at when we either (a) wrote at least one secret
    // successfully, or (b) the file already had no secret material to
    // begin with (post-migration files re-loaded after a clean save).
    // The "all writes failed" case is intentionally NOT stamped so the
    // next load re-tries the migration.
    if wrote_all && (wrote_any || !already_migrated) {
        creds.migrated_at.get_or_insert_with(now_unix_seconds);
    }
    creds
}

/// Unix epoch seconds — `i64` shape matches `expires_at` so we can
/// compare without conversion. Falls back to 0 on a clock that pre-
/// dates the epoch (only possible on a broken VM).
fn now_unix_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Persist the store to an explicit path. Creates parent dirs and
/// writes atomically (tempfile + rename) so a crash mid-write leaves
/// either the old file or the new one — never a half-written one.
pub async fn save_to(path: &std::path::Path, store: &PublishingStore) -> Result<(), ProviderError> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| ProviderError::Io(format!("create dir {}: {e}", parent.display())))?;
    }
    let buf = serde_json::to_vec_pretty(store)
        .map_err(|e| ProviderError::Io(format!("serialize store: {e}")))?;
    // Same-directory tempfile so the rename is a filesystem-level swap.
    let tmp = path.with_extension("json.tmp");
    tokio::fs::write(&tmp, &buf)
        .await
        .map_err(|e| ProviderError::Io(format!("write {}: {e}", tmp.display())))?;
    tokio::fs::rename(&tmp, path).await.map_err(|e| {
        ProviderError::Io(format!(
            "rename {} → {}: {e}",
            tmp.display(),
            path.display()
        ))
    })?;
    Ok(())
}

/// Map a [`KeychainError`] into the publishing error surface so OAuth
/// helpers can `?`-propagate without scattering map_err calls.
pub(super) fn keychain_to_provider(e: KeychainError) -> ProviderError {
    ProviderError::Io(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::publishing::keychain::install_mock_backend;

    /// Generate a unique provider key for each test so parallel tests
    /// don't observe each other's writes via the shared (process-wide)
    /// mock keychain backend.
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

    fn sample_creds() -> Credentials {
        Credentials {
            account_name: Some("you@gmail.com".into()),
            expires_at: Some(1_717_123_456),
            client_id: Some("…apps.googleusercontent.com".into()),
            migrated_at: Some(1_717_123_000),
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
        // Metadata-only round trip — the keychain isn't involved when
        // there are no secrets to migrate.
        install_mock_backend();
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("publishing.json");

        let mut store = PublishingStore::default();
        store.set("youtube", Some(sample_creds()));
        store.set("tiktok", None);
        save_to(&path, &store).await.unwrap();

        let reloaded = load_from(&path).await.unwrap();
        assert_eq!(reloaded.get("youtube"), Some(&sample_creds()));
        // `null` slot serialises distinct from "absent".
        assert!(reloaded.providers.contains_key("tiktok"));
        assert!(reloaded.get("tiktok").is_none());
        assert!(!reloaded.providers.contains_key("instagram"));
        assert!(reloaded.get("instagram").is_none());
    }

    #[tokio::test]
    async fn save_creates_parent_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let nested = tmp.path().join("a").join("b").join("publishing.json");
        save_to(&nested, &PublishingStore::default()).await.unwrap();
        assert!(nested.is_file());
    }

    #[tokio::test]
    async fn save_is_atomic_no_tempfile_leak() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("publishing.json");
        let mut store = PublishingStore::default();
        store.set("youtube", Some(sample_creds()));
        save_to(&path, &store).await.unwrap();
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
    async fn migration_from_legacy_plaintext() {
        // Brief-named test: a JSON file with plaintext secrets is
        // loaded, the secrets move to the keychain, and the next save
        // strips them from disk.
        install_mock_backend();
        let provider = unique_provider("yt-migrate");
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("publishing.json");
        let legacy = format!(
            r#"{{
                "{provider}": {{
                    "access_token": "ya29.legacy",
                    "refresh_token": "1//legacy-refresh",
                    "account_name": "old@gmail.com",
                    "expires_at": 1,
                    "client_credentials": {{
                        "client_id": "old-client-id",
                        "client_secret": "old-client-secret"
                    }}
                }}
            }}"#,
        );
        tokio::fs::write(&path, &legacy).await.unwrap();

        let store = load_from(&path).await.unwrap();
        let creds = store.get(&provider).expect("slot present");
        // Metadata preserved; client_id lifted out of client_credentials.
        assert_eq!(creds.account_name.as_deref(), Some("old@gmail.com"));
        assert_eq!(creds.expires_at, Some(1));
        assert_eq!(creds.client_id.as_deref(), Some("old-client-id"));
        assert!(creds.migrated_at.is_some(), "migration stamps timestamp");

        // Secrets landed in the keychain.
        let kc = Keychain::new();
        assert_eq!(
            kc.read_token(&provider, TokenKind::AccessToken)
                .expect("read access")
                .as_deref(),
            Some("ya29.legacy"),
        );
        assert_eq!(
            kc.read_token(&provider, TokenKind::RefreshToken)
                .expect("read refresh")
                .as_deref(),
            Some("1//legacy-refresh"),
        );
        assert_eq!(
            kc.read_token(&provider, TokenKind::ClientSecret)
                .expect("read client_secret")
                .as_deref(),
            Some("old-client-secret"),
        );

        // Save the (now migrated) store and confirm the on-disk JSON
        // no longer carries any secret material.
        save_to(&path, &store).await.unwrap();
        let rewritten = tokio::fs::read_to_string(&path).await.unwrap();
        assert!(!rewritten.contains("ya29.legacy"));
        assert!(!rewritten.contains("legacy-refresh"));
        assert!(!rewritten.contains("old-client-secret"));
        assert!(rewritten.contains("old-client-id")); // public id stays
        assert!(rewritten.contains("migrated_at"));

        // Reload the rewritten file — migration is idempotent
        // (migrated_at stays, no spurious re-writes).
        let reloaded = load_from(&path).await.unwrap();
        let creds2 = reloaded.get(&provider).expect("slot");
        assert_eq!(creds2.migrated_at, creds.migrated_at);
    }

    #[tokio::test]
    async fn migration_is_idempotent_when_no_legacy_fields() {
        // Already-migrated file → reload preserves the stamp, no extra
        // keychain writes attempted.
        install_mock_backend();
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("publishing.json");
        let stamped = r#"{
            "youtube": {
                "account_name": "you@gmail.com",
                "expires_at": 9999,
                "client_id": "cid",
                "migrated_at": 1234567
            }
        }"#;
        tokio::fs::write(&path, stamped).await.unwrap();
        let store = load_from(&path).await.unwrap();
        let creds = store.get("youtube").expect("slot");
        assert_eq!(creds.migrated_at, Some(1234567));
        assert_eq!(creds.client_id.as_deref(), Some("cid"));
    }

    #[test]
    fn has_client_id_distinguishes_empty_from_real() {
        let mut c = Credentials::default();
        assert!(!c.has_client_id());
        c.client_id = Some(String::new());
        assert!(!c.has_client_id());
        c.client_id = Some("real".into());
        assert!(c.has_client_id());
    }

    #[test]
    fn clear_oauth_metadata_preserves_client_id() {
        let mut c = Credentials {
            account_name: Some("a".into()),
            expires_at: Some(1),
            client_id: Some("id".into()),
            migrated_at: Some(1),
        };
        assert!(c.clear_oauth_metadata(), "had metadata to clear");
        assert!(c.account_name.is_none());
        assert!(c.expires_at.is_none());
        assert_eq!(c.client_id.as_deref(), Some("id"));
        // Idempotent.
        assert!(!c.clear_oauth_metadata());
    }

    #[tokio::test]
    async fn get_or_insert_creates_default_slot() {
        let mut store = PublishingStore::default();
        let slot = store.get_or_insert("youtube");
        assert!(!slot.has_client_id());
        slot.client_id = Some("cid".into());
        assert!(store.get("youtube").is_some_and(|c| c.has_client_id()));
    }

    #[test]
    fn default_store_path_is_under_montage_namespace() {
        let p = default_store_path().unwrap();
        assert_eq!(p.file_name().unwrap(), "publishing.json");
        assert_eq!(
            p.parent().unwrap().file_name().unwrap(),
            "montage",
            "publishing.json must live in the montage config namespace",
        );
    }
}
