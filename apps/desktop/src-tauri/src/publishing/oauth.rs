//! OAuth + shared stub helpers used by every provider.
//!
//! Each platform's authorize URL differs in `client_id`, redirect URI,
//! and scope set, but the shape of the work — pick a `state` nonce,
//! url-encode params, hand back an [`OAuthChallenge`] — is identical.
//! Same goes for the stub-phase `is_configured` / `status` /
//! `upload` bodies: the only thing that varies is
//! the provider's storage key and the dev-console URL.
//! Centralising both here keeps the per-provider files focused on the
//! one platform-specific thing each really owns: the URL template.

use std::time::{SystemTime, UNIX_EPOCH};

use super::ai_disclosure::provider_log_line;
use super::errors::ProviderError;
use super::storage::{self};
use super::types::{ConnectionStatus, OAuthChallenge, UploadParams, UploadResult};

/// Placeholder marker the provider stubs embed in their authorize
/// URLs. When the user pastes a real `client_id` into their config,
/// this is the literal string they replace.
///
/// Documented as a constant so the frontend (W5.A5) can grep for it
/// and surface a "credentials not registered" hint.
pub const CLIENT_ID_PLACEHOLDER: &str = "YOUR_CLIENT_ID_HERE";

/// Local loopback used for the OAuth redirect. We keep the port
/// stable so the user only has to whitelist it once in the dev
/// console. Real implementation in W5.A2+ will spin up a one-shot
/// HTTP server bound to this address.
pub const REDIRECT_URI: &str = "http://127.0.0.1:8419/oauth/callback";

const KEYCHAIN_REQUIRED: &str =
    "publishing OAuth is disabled until keychain-backed credential storage is implemented";

/// Pick a fresh state nonce. Cryptographically uninteresting — this
/// is CSRF defense, not authentication — so a monotonically-unique
/// time value plus a process-counter is enough. Avoids a `rand`
/// dependency for the stub phase; the real provider impls (W5.A2+)
/// will swap in `rand::thread_rng().gen()` for a 128-bit token.
///
/// TODO(W5.A2): replace with a 128-bit secure random.
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

/// Url-encode one query value. Cheap inline implementation so we
/// don't pull in `urlencoding` for the stub phase — the chars that
/// matter for OAuth URLs are `:` `/` `?` `#` `&` ` `, and we keep
/// alphanumerics + `-_.~` per RFC 3986.
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
///
/// Drops `redirect_uri` and `state` into the pair list so callers
/// don't have to repeat them per provider.
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

// ---- Shared stub-phase implementations ----
//
// The W5.A1 contract is that every provider's `is_configured` /
// `status` / `upload` collapse to the same logic (read legacy state
// from publishing.json, refuse new secret writes, return `Unsupported`
// with a dev-console hint). The per-provider files just
// pass in their storage key, dev-console URL, and store path.
//
// Taking the store path as an argument (rather than reading from a
// global) keeps tests sandboxable without env-var racing — each test
// constructs its provider with a tempdir-backed path.

/// Cheap check used by `is_configured` — has a non-null slot with a
/// usable access token in the store? A slot that only carries BYO
/// client_credentials counts as "not configured" (the OAuth flow
/// hasn't completed yet).
pub async fn has_credentials(store_path: &std::path::Path, key: &str) -> bool {
    storage::load_from(store_path)
        .await
        .ok()
        .and_then(|s| s.get_authenticated(key).cloned())
        .is_some()
}

/// Read the slot and build a `ConnectionStatus` snapshot. Missing /
/// unreadable storage collapses to the default "not connected" status
/// — callers should never bubble I/O errors into the Settings UI.
pub async fn load_status(store_path: &std::path::Path, key: &str) -> ConnectionStatus {
    let creds = storage::load_from(store_path)
        .await
        .ok()
        .and_then(|s| s.get_authenticated(key).cloned());
    match creds {
        Some(c) => ConnectionStatus {
            connected: true,
            account_name: c.account_name,
            expires_at: c.expires_at,
        },
        None => ConnectionStatus::default(),
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
        .and_then(|c| c.client_credentials)
        .map(|cc| cc.client_id)
        .filter(|id| !id.is_empty())
        .unwrap_or_else(|| CLIENT_ID_PLACEHOLDER.to_string())
}

/// Persisting `(client_id, client_secret)` is disabled until secrets
/// move to an OS keychain. Legacy plaintext config can still be read
/// so existing local state does not crash the settings surface.
///
/// Empty strings for either field are rejected — the BYO contract is
/// all-or-nothing (a half-set state would silently fall back to the
/// placeholder `client_id` mid-OAuth, which is the worst failure mode).
pub async fn set_client_credentials(
    store_path: &std::path::Path,
    key: &str,
    client_id: String,
    client_secret: String,
) -> Result<(), super::errors::ProviderError> {
    if client_id.trim().is_empty() || client_secret.trim().is_empty() {
        return Err(super::errors::ProviderError::OAuthFailed(
            "client_id and client_secret must both be non-empty".into(),
        ));
    }
    let _ = (store_path, key);
    Err(super::errors::ProviderError::Unsupported(
        KEYCHAIN_REQUIRED.into(),
    ))
}

/// Read the *presence* of BYO client credentials — never returns the
/// secret itself, only `(client_id_set, client_secret_set)` flags. The
/// frontend uses this to show "✓ Configured" without ever round-tripping
/// the secret through IPC.
pub async fn get_client_credentials_state(
    store_path: &std::path::Path,
    key: &str,
) -> ClientCredentialsState {
    let cc = storage::load_from(store_path)
        .await
        .ok()
        .and_then(|s| s.get(key).cloned())
        .and_then(|c| c.client_credentials);
    match cc {
        Some(cc) => ClientCredentialsState {
            client_id_set: !cc.client_id.is_empty(),
            client_secret_set: !cc.client_secret.is_empty(),
        },
        None => ClientCredentialsState::default(),
    }
}

/// Wire shape for `get_client_credentials_state`. Booleans only — the
/// actual `client_secret` value never leaves the backend.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ClientCredentialsState {
    pub client_id_set: bool,
    pub client_secret_set: bool,
}

/// Shared `disconnect` — clears the OAuth-issued tokens but preserves
/// the user's BYO client credentials so they don't have to re-paste.
/// Returns `Ok(())` even when there were no tokens to clear (idempotent).
pub async fn disconnect_provider(
    store_path: &std::path::Path,
    key: &str,
) -> Result<(), super::errors::ProviderError> {
    let mut store = storage::load_from(store_path).await?;
    // If client_credentials are present, keep the slot around with
    // tokens cleared; otherwise drop the slot entirely.
    let keep_slot = store
        .get(key)
        .and_then(|c| c.client_credentials.clone())
        .is_some();
    if keep_slot {
        let slot = store.get_or_insert(key);
        slot.clear_tokens();
    } else {
        store.set(key, None);
    }
    storage::save_to(store_path, &store).await
}

/// Shared stub for `complete_oauth`. Non-empty codes are accepted as
/// valid input, but no access token is persisted until the publishing
/// layer has keychain-backed credential storage.
pub async fn stub_complete_oauth(
    store_path: &std::path::Path,
    key: &str,
    code: String,
) -> Result<(), ProviderError> {
    if code.trim().is_empty() {
        return Err(ProviderError::OAuthFailed(
            "empty authorization code".into(),
        ));
    }
    let _ = (store_path, key);
    Err(ProviderError::Unsupported(KEYCHAIN_REQUIRED.into()))
}

/// Shared stub for `upload`. Two-tier behaviour:
///
/// 1. No credentials → [`ProviderError::NotConfigured`] (frontend opens
///    the OAuth flow).
/// 2. Credentials present → [`ProviderError::Unsupported`] with a
///    pointer to the platform's dev console (real upload code ships
///    in W5.A2+).
///
/// `disclosure_flag_name` is the platform-specific AI flag the real
/// upload would set when synthetic content is present (YouTube
/// `alteredContent`, TikTok `aigc_label`, Instagram `ai_label`).
/// When `params.ai_disclosure.has_synthetic_content` is true the stub
/// folds a "would set <flag>=true" hint into the Unsupported message
/// + a `tracing::info!` line so the user (and tests) can confirm the
/// disclosure intent reached the platform layer.
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
    // Synthetic-content disclosure (W5.A4). The stub doesn't actually
    // hit the platform yet, so the log line + extended error message
    // are the user-visible evidence that the flag would have been set
    // on the real upload.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_state_is_unique() {
        // Two consecutive calls must not collide — the counter
        // guarantees this even if the clock has 1ns resolution.
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
    async fn set_client_credentials_is_disabled_until_keychain_storage() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("publishing.json");
        let err = set_client_credentials(
            &path,
            "youtube",
            "real-client-id.apps.googleusercontent.com".into(),
            "secret-shh".into(),
        )
        .await
        .unwrap_err();
        assert_eq!(err.kind(), "unsupported");
        let id = client_id_for(&path, "youtube").await;
        assert_eq!(id, CLIENT_ID_PLACEHOLDER);
        let state = get_client_credentials_state(&path, "youtube").await;
        assert!(!state.client_id_set);
        assert!(!state.client_secret_set);
        assert!(!path.exists());
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
    async fn disconnect_preserves_legacy_client_credentials() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("publishing.json");
        let mut store = storage::PublishingStore::default();
        store.set(
            "youtube",
            Some(storage::Credentials {
                access_token: "legacy-token".into(),
                client_credentials: Some(storage::ClientCredentials {
                    client_id: "cid".into(),
                    client_secret: "csec".into(),
                }),
                ..Default::default()
            }),
        );
        storage::save_to(&path, &store).await.unwrap();

        assert!(has_credentials(&path, "youtube").await);
        disconnect_provider(&path, "youtube").await.unwrap();
        assert!(!has_credentials(&path, "youtube").await);
        let state = get_client_credentials_state(&path, "youtube").await;
        assert!(
            state.client_id_set && state.client_secret_set,
            "legacy BYO creds must outlive disconnect",
        );
        let id = client_id_for(&path, "youtube").await;
        assert_eq!(id, "cid");
    }

    #[tokio::test]
    async fn disconnect_without_byo_drops_slot() {
        // No BYO creds → disconnect collapses the slot entirely so
        // we don't accumulate empty stubs.
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("publishing.json");
        let mut store = storage::PublishingStore::default();
        store.set(
            "youtube",
            Some(storage::Credentials {
                access_token: "legacy-token".into(),
                ..Default::default()
            }),
        );
        storage::save_to(&path, &store).await.unwrap();
        assert!(has_credentials(&path, "youtube").await);
        disconnect_provider(&path, "youtube").await.unwrap();
        assert!(!has_credentials(&path, "youtube").await);
        // Slot dropped entirely.
        let store = storage::load_from(&path).await.unwrap();
        assert!(store.get("youtube").is_none());
    }

    #[tokio::test]
    async fn stub_complete_oauth_is_disabled_until_keychain_storage() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("publishing.json");
        let err = stub_complete_oauth(&path, "youtube", "code".into())
            .await
            .unwrap_err();
        assert_eq!(err.kind(), "unsupported");
        assert!(!has_credentials(&path, "youtube").await);
        let state = get_client_credentials_state(&path, "youtube").await;
        assert!(!state.client_id_set);
        assert!(!state.client_secret_set);
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
        // redirect_uri is encoded.
        assert!(
            challenge
                .url
                .contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A8419%2Foauth%2Fcallback"),
            "got {}",
            challenge.url,
        );
    }
}
