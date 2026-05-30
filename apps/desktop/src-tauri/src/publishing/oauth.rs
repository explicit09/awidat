//! OAuth + shared stub helpers used by every provider.
//!
//! Each platform's authorize URL differs in `client_id`, redirect URI,
//! and scope set, but the shape of the work — pick a `state` nonce,
//! url-encode params, hand back an [`OAuthChallenge`] — is identical.
//! Same goes for the stub-phase `is_configured` / `status` /
//! `upload` bodies: the only thing that varies is the provider's
//! storage key and the dev-console URL.
//! Centralising both here keeps the per-provider files focused on the
//! one platform-specific thing each really owns: the URL template.
//!
//! # Secret material
//!
//! Post-W6.A4 every secret (access token, refresh token, client
//! secret) lives in the OS keychain via [`super::keychain`]. The
//! `publishing.json` file under [`super::storage`] keeps only
//! non-secret metadata. The "blocked writes" stance the pre-keychain
//! revision carried is gone — `set_client_credentials` and
//! `stub_complete_oauth` now succeed end-to-end.

use std::time::{SystemTime, UNIX_EPOCH};

use super::ai_disclosure::provider_log_line;
use super::errors::ProviderError;
use super::keychain::{Keychain, TokenKind};
use super::storage::{self, keychain_to_provider};
use super::types::{ConnectionStatus, OAuthChallenge, UploadParams, UploadResult};

/// Placeholder marker the provider stubs embed in their authorize
/// URLs. When the user pastes a real `client_id` into their config,
/// this is the literal string they replace.
pub const CLIENT_ID_PLACEHOLDER: &str = "YOUR_CLIENT_ID_HERE";

/// Local loopback used for the OAuth redirect. We keep the port
/// stable so the user only has to whitelist it once in the dev
/// console. W6.A1 spins up a one-shot HTTP server bound to this
/// address.
pub const REDIRECT_URI: &str = "http://127.0.0.1:8419/oauth/callback";

/// Pick a fresh state nonce. Cryptographically uninteresting — this
/// is CSRF defense, not authentication — so a monotonically-unique
/// time value plus a process-counter is enough.
pub fn fresh_state() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("awidat-{nanos:x}-{seq:x}")
}

/// Url-encode one query value.
pub fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        let safe = byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~');
        if safe {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// Build a `key=value` query string from name/value pairs.
pub fn query_string(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| format!("{k}={}", percent_encode(v)))
        .collect::<Vec<_>>()
        .join("&")
}

/// Convenience builder: assemble an authorize URL from a base + pairs.
pub fn build_authorize(base: &str, extra_pairs: &[(&str, &str)], state: &str) -> OAuthChallenge {
    let mut pairs: Vec<(&str, &str)> = Vec::with_capacity(extra_pairs.len() + 2);
    pairs.extend_from_slice(extra_pairs);
    pairs.push(("redirect_uri", REDIRECT_URI));
    pairs.push(("state", state));
    let qs = query_string(&pairs);
    let url = format!("{base}?{qs}");
    OAuthChallenge {
        url,
        state: state.to_string(),
    }
}

// ---- Shared implementations ----

/// Cheap check used by `is_configured` — has a stored access token
/// in the keychain? A slot that only carries BYO client credentials
/// counts as "not configured" (the OAuth flow hasn't completed yet).
pub async fn has_credentials(store_path: &std::path::Path, key: &str) -> bool {
    // We don't fail loudly on a keychain read error here — `is_configured`
    // is polled every time the Settings modal opens, and a transient
    // backend hiccup should not cascade into a flashing UI. Treat
    // errors as "not configured" and let the OAuth path surface the
    // real issue when the user clicks Connect.
    Keychain::new()
        .read_token(key, TokenKind::AccessToken)
        .ok()
        .flatten()
        .filter(|t| !t.is_empty())
        .is_some()
        && storage::load_from(store_path)
            .await
            .ok()
            .and_then(|s| s.get(key).cloned())
            .is_some()
}

/// Read the slot and build a `ConnectionStatus` snapshot. Missing /
/// unreadable storage collapses to the default "not connected" status.
pub async fn load_status(store_path: &std::path::Path, key: &str) -> ConnectionStatus {
    let meta = storage::load_from(store_path)
        .await
        .ok()
        .and_then(|s| s.get(key).cloned());
    let has_access = Keychain::new()
        .read_token(key, TokenKind::AccessToken)
        .ok()
        .flatten()
        .filter(|t| !t.is_empty())
        .is_some();
    match (meta, has_access) {
        (Some(c), true) => ConnectionStatus {
            connected: true,
            account_name: c.account_name,
            expires_at: c.expires_at,
        },
        _ => ConnectionStatus::default(),
    }
}

/// Read the user's BYO `client_id` for a provider, falling back to
/// the placeholder when unset. Centralised so per-provider authorize
/// URL builders all pick up the same substitution rule.
pub async fn client_id_for(store_path: &std::path::Path, key: &str) -> String {
    storage::load_from(store_path)
        .await
        .ok()
        .and_then(|s| s.get(key).cloned())
        .and_then(|c| c.client_id)
        .filter(|id| !id.is_empty())
        .unwrap_or_else(|| CLIENT_ID_PLACEHOLDER.to_string())
}

/// Persist `(client_id, client_secret)` for a provider. The public
/// `client_id` lands in `publishing.json`; the secret half lands in
/// the OS keychain.
///
/// Empty strings for either field are rejected — the BYO contract is
/// all-or-nothing (a half-set state would silently fall back to the
/// placeholder `client_id` mid-OAuth, which is the worst failure mode).
pub async fn set_client_credentials(
    store_path: &std::path::Path,
    key: &str,
    client_id: String,
    client_secret: String,
) -> Result<(), ProviderError> {
    if client_id.trim().is_empty() || client_secret.trim().is_empty() {
        return Err(ProviderError::OAuthFailed(
            "client_id and client_secret must both be non-empty".into(),
        ));
    }

    // Secret first — if the keychain write fails we don't want a stale
    // metadata-only state on disk telling the UI "configured".
    Keychain::new()
        .store_token(key, TokenKind::ClientSecret, &client_secret)
        .map_err(keychain_to_provider)?;

    let mut store = storage::load_from(store_path).await?;
    let slot = store.get_or_insert(key);
    slot.client_id = Some(client_id);
    // Stamp migrated_at so a load-then-save cycle from this point is
    // recognised as already-on-the-new-path.
    slot.migrated_at.get_or_insert_with(now_unix_seconds);
    storage::save_to(store_path, &store).await
}

/// Read the *presence* of BYO client credentials — never returns the
/// secret itself, only `(client_id_set, client_secret_set)` flags.
pub async fn get_client_credentials_state(
    store_path: &std::path::Path,
    key: &str,
) -> ClientCredentialsState {
    let client_id_set = storage::load_from(store_path)
        .await
        .ok()
        .and_then(|s| s.get(key).cloned())
        .map(|c| c.has_client_id())
        .unwrap_or(false);
    // Keychain check: any error → treat as "not set" so the UI doesn't
    // flicker. The real issue surfaces when the user re-pastes.
    let client_secret_set = Keychain::new()
        .read_token(key, TokenKind::ClientSecret)
        .ok()
        .flatten()
        .filter(|s| !s.is_empty())
        .is_some();
    ClientCredentialsState {
        client_id_set,
        client_secret_set,
    }
}

/// Wire shape for `get_client_credentials_state`. Booleans only — the
/// actual `client_secret` value never leaves the backend.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ClientCredentialsState {
    pub client_id_set: bool,
    pub client_secret_set: bool,
}

/// Shared `disconnect` — clears the OAuth-issued tokens (keychain +
/// metadata) but preserves the user's BYO `client_id` / `client_secret`
/// so they don't have to re-paste. Returns `Ok(())` even when there
/// were no tokens to clear (idempotent).
pub async fn disconnect_provider(
    store_path: &std::path::Path,
    key: &str,
) -> Result<(), ProviderError> {
    // Always attempt the keychain delete — even if the metadata file
    // is missing a stored access token could linger from a corrupted
    // load. Errors here become Io kind so the frontend can show "try
    // again" rather than the misleading "not configured".
    Keychain::new()
        .delete_oauth_tokens(key)
        .map_err(keychain_to_provider)?;

    let mut store = storage::load_from(store_path).await?;
    // Decide whether to keep the slot around. We keep it if the user
    // still has BYO client credentials configured — wiping the slot
    // there would force them to re-paste on reconnect.
    let keep_slot = store
        .get(key)
        .map(|c| c.has_client_id())
        .unwrap_or(false);
    if keep_slot {
        let slot = store.get_or_insert(key);
        slot.clear_oauth_metadata();
    } else {
        store.set(key, None);
    }
    storage::save_to(store_path, &store).await
}

/// Shared `complete_oauth`. The real token exchange (POST to the
/// provider's `/token` endpoint) lands in W5.A2+; today we accept the
/// authorisation code as evidence the flow reached us and persist it
/// to the keychain as the access token so downstream gates (status
/// pill, upload kickoff) light up end-to-end.
///
/// This is the entry point that the pre-W6.A4 revision intentionally
/// fenced off with `KEYCHAIN_REQUIRED`. With keychain storage now
/// available, the fence comes down — writes succeed.
pub async fn stub_complete_oauth(
    store_path: &std::path::Path,
    key: &str,
    code: String,
) -> Result<(), ProviderError> {
    let code = code.trim();
    if code.is_empty() {
        return Err(ProviderError::OAuthFailed(
            "empty authorization code".into(),
        ));
    }

    // TODO(W5.A2+): replace this with the real authorization-code →
    // access_token / refresh_token exchange for each provider. For now
    // we stash the raw code under TokenKind::AccessToken so the
    // is_configured / status / upload gates flip the way they would
    // post-exchange.
    let kc = Keychain::new();
    kc.store_token(key, TokenKind::AccessToken, code)
        .map_err(keychain_to_provider)?;

    let mut store = storage::load_from(store_path).await?;
    let slot = store.get_or_insert(key);
    // We don't know account_name / expires_at until the real exchange
    // lands — leave them None so the UI shows the bare "Connected"
    // label rather than inventing data.
    slot.migrated_at.get_or_insert_with(now_unix_seconds);
    storage::save_to(store_path, &store).await
}

/// Shared stub for `upload`. Two-tier behaviour:
///
/// 1. No credentials → [`ProviderError::NotConfigured`] (frontend opens
///    the OAuth flow).
/// 2. Credentials present → [`ProviderError::Unsupported`] with a
///    pointer to the platform's dev console (real upload code ships
///    in W5.A2+).
pub async fn stub_upload(
    store_path: &std::path::Path,
    key: &str,
    dev_console_url: &str,
    disclosure_flag_name: &str,
    params: UploadParams,
) -> Result<UploadResult, ProviderError> {
    if !has_credentials(store_path, key).await {
        return Err(ProviderError::NotConfigured);
    }
    let disclosure_hint = match params.ai_disclosure.as_ref() {
        Some(d) if d.has_synthetic_content => {
            let line = provider_log_line(disclosure_flag_name, d);
            tracing::info!(provider = key, "{line}");
            format!(" [{line}]")
        }
        _ => String::new(),
    };
    Err(ProviderError::Unsupported(format!(
        "Real upload requires OAuth credentials — register your app at {dev_console_url}{disclosure_hint}",
    )))
}

/// Unix epoch seconds — local copy avoids a `pub use` from storage.rs.
fn now_unix_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::publishing::keychain::install_mock_backend;

    /// Generate a unique provider key per test so parallel tests using
    /// the shared mock keychain don't observe each other's writes.
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
    fn fresh_state_is_unique() {
        let a = fresh_state();
        let b = fresh_state();
        assert_ne!(a, b);
        assert!(a.starts_with("awidat-"));
    }

    #[test]
    fn percent_encode_keeps_unreserved() {
        assert_eq!(percent_encode("AZaz09-_.~"), "AZaz09-_.~");
    }

    #[test]
    fn percent_encode_escapes_reserved() {
        assert_eq!(percent_encode(" "), "%20");
        assert_eq!(percent_encode("/"), "%2F");
        assert_eq!(percent_encode("?"), "%3F");
        assert_eq!(percent_encode("&"), "%26");
        assert_eq!(percent_encode(":"), "%3A");
    }

    #[tokio::test]
    async fn client_id_for_unset_returns_placeholder() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("publishing.json");
        let id = client_id_for(&path, "youtube").await;
        assert_eq!(id, CLIENT_ID_PLACEHOLDER);
    }

    #[tokio::test]
    async fn set_client_credentials_persists_id_and_secret() {
        install_mock_backend();
        let provider = unique_provider("yt-set");
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("publishing.json");
        set_client_credentials(
            &path,
            &provider,
            "real-client-id.apps.googleusercontent.com".into(),
            "secret-shh".into(),
        )
        .await
        .unwrap();

        // client_id substitution flips off the placeholder.
        let id = client_id_for(&path, &provider).await;
        assert_eq!(id, "real-client-id.apps.googleusercontent.com");

        // BYO presence flags reflect both halves.
        let state = get_client_credentials_state(&path, &provider).await;
        assert!(state.client_id_set);
        assert!(state.client_secret_set);

        // client_secret is in the keychain, NOT in the JSON file.
        let on_disk = tokio::fs::read_to_string(&path).await.unwrap();
        assert!(!on_disk.contains("secret-shh"));
        assert!(on_disk.contains("real-client-id"));
        assert_eq!(
            Keychain::new()
                .read_token(&provider, TokenKind::ClientSecret)
                .unwrap()
                .as_deref(),
            Some("secret-shh"),
        );
    }

    #[tokio::test]
    async fn set_client_credentials_rejects_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("publishing.json");
        let err = set_client_credentials(&path, "youtube", "".into(), "x".into())
            .await
            .unwrap_err();
        assert_eq!(err.kind(), "oauth_failed");
    }

    #[tokio::test]
    async fn disconnect_preserves_client_credentials() {
        install_mock_backend();
        let provider = unique_provider("yt-disconn");
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("publishing.json");
        // Seed BYO + a completed OAuth flow.
        set_client_credentials(&path, &provider, "cid".into(), "csec".into())
            .await
            .unwrap();
        stub_complete_oauth(&path, &provider, "auth-code".into())
            .await
            .unwrap();
        assert!(has_credentials(&path, &provider).await);

        disconnect_provider(&path, &provider).await.unwrap();
        assert!(!has_credentials(&path, &provider).await);
        let state = get_client_credentials_state(&path, &provider).await;
        assert!(
            state.client_id_set && state.client_secret_set,
            "BYO client creds must outlive disconnect",
        );
        let id = client_id_for(&path, &provider).await;
        assert_eq!(id, "cid");
    }

    #[tokio::test]
    async fn disconnect_without_byo_drops_slot() {
        install_mock_backend();
        let provider = unique_provider("yt-drop");
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("publishing.json");
        // Seed only a completed OAuth (no BYO creds) and confirm
        // disconnect collapses the slot entirely rather than leaving
        // an empty stub behind.
        stub_complete_oauth(&path, &provider, "code".into())
            .await
            .unwrap();
        assert!(has_credentials(&path, &provider).await);
        disconnect_provider(&path, &provider).await.unwrap();
        assert!(!has_credentials(&path, &provider).await);
        let store = storage::load_from(&path).await.unwrap();
        assert!(store.get(&provider).is_none());
    }

    #[tokio::test]
    async fn stub_complete_oauth_unblocks_after_keychain_landed() {
        // Pre-W6.A4 the helper returned Unsupported / KEYCHAIN_REQUIRED;
        // post this commit it persists the code into the keychain and
        // flips has_credentials true. This is the load-bearing
        // assertion that this task actually unblocks the W5 write path.
        install_mock_backend();
        let provider = unique_provider("yt-unblock");
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("publishing.json");
        stub_complete_oauth(&path, &provider, "fake-code".into())
            .await
            .unwrap();
        assert!(has_credentials(&path, &provider).await);
        assert_eq!(
            Keychain::new()
                .read_token(&provider, TokenKind::AccessToken)
                .unwrap()
                .as_deref(),
            Some("fake-code"),
        );
    }

    #[tokio::test]
    async fn stub_complete_oauth_rejects_empty_code() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("publishing.json");
        let err = stub_complete_oauth(&path, "youtube", "   ".into())
            .await
            .unwrap_err();
        assert_eq!(err.kind(), "oauth_failed");
    }

    #[test]
    fn build_authorize_appends_state_and_redirect() {
        let challenge = build_authorize(
            "https://example.com/o/authorize",
            &[("client_id", CLIENT_ID_PLACEHOLDER), ("scope", "upload")],
            "fixed-state",
        );
        assert_eq!(challenge.state, "fixed-state");
        assert!(
            challenge
                .url
                .starts_with("https://example.com/o/authorize?")
        );
        assert!(challenge.url.contains("client_id=YOUR_CLIENT_ID_HERE"));
        assert!(challenge.url.contains("scope=upload"));
        assert!(challenge.url.contains("state=fixed-state"));
        assert!(
            challenge
                .url
                .contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A8419%2Foauth%2Fcallback"),
            "got {}",
            challenge.url,
        );
    }
}
