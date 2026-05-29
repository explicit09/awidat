//! OAuth + shared stub helpers used by every provider.
//!
//! Each platform's authorize URL differs in `client_id`, redirect URI,
//! and scope set, but the shape of the work — pick a `state` nonce,
//! url-encode params, hand back an [`OAuthChallenge`] — is identical.
//! Same goes for the stub-phase `is_configured` / `status` /
//! `complete_oauth` / `upload` bodies: the only thing that varies is
//! the provider's storage key and the dev-console URL.
//! Centralising both here keeps the per-provider files focused on the
//! one platform-specific thing each really owns: the URL template.

use std::time::{SystemTime, UNIX_EPOCH};

use super::errors::ProviderError;
use super::storage::{self, Credentials};
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
        let safe = byte.is_ascii_alphanumeric()
            || matches!(byte, b'-' | b'_' | b'.' | b'~');
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
pub fn build_authorize(
    base: &str,
    extra_pairs: &[(&str, &str)],
    state: &str,
) -> OAuthChallenge {
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
// `status` / `complete_oauth` / `upload` collapse to the same logic
// (read from publishing.json, write a token-shaped placeholder, return
// `Unsupported` with a dev-console hint). The per-provider files just
// pass in their storage key, dev-console URL, and store path.
//
// Taking the store path as an argument (rather than reading from a
// global) keeps tests sandboxable without env-var racing — each test
// constructs its provider with a tempdir-backed path.

/// Cheap check used by `is_configured` — has a non-null slot in the store?
pub async fn has_credentials(store_path: &std::path::Path, key: &str) -> bool {
    storage::load_from(store_path)
        .await
        .ok()
        .and_then(|s| s.get(key).cloned())
        .is_some()
}

/// Read the slot and build a `ConnectionStatus` snapshot. Missing /
/// unreadable storage collapses to the default "not connected" status
/// — callers should never bubble I/O errors into the Settings UI.
pub async fn load_status(store_path: &std::path::Path, key: &str) -> ConnectionStatus {
    let creds = storage::load_from(store_path)
        .await
        .ok()
        .and_then(|s| s.get(key).cloned());
    match creds {
        Some(c) => ConnectionStatus {
            connected: true,
            account_name: c.account_name,
            expires_at: c.expires_at,
        },
        None => ConnectionStatus::default(),
    }
}

/// Shared stub for `complete_oauth` — accepts any non-empty `code`
/// and writes a placeholder token to storage so the rest of the flow
/// (status → upload) can be exercised end-to-end.
///
/// Real provider impls (W5.A2+) replace this with a POST to the
/// platform's token endpoint.
pub async fn stub_complete_oauth(
    store_path: &std::path::Path,
    key: &str,
    code: String,
) -> Result<(), ProviderError> {
    if code.trim().is_empty() {
        return Err(ProviderError::OAuthFailed("empty authorization code".into()));
    }
    let mut store = storage::load_from(store_path).await?;
    store.set(
        key,
        Some(Credentials {
            access_token: format!("stub-token-from-code-{code}"),
            refresh_token: None,
            account_name: None,
            expires_at: None,
        }),
    );
    storage::save_to(store_path, &store).await?;
    Ok(())
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
    _params: UploadParams,
) -> Result<UploadResult, ProviderError> {
    if !has_credentials(store_path, key).await {
        return Err(ProviderError::NotConfigured);
    }
    Err(ProviderError::Unsupported(format!(
        "Real upload requires OAuth credentials — register your app at {dev_console_url}",
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

    #[test]
    fn build_authorize_appends_state_and_redirect() {
        let challenge = build_authorize(
            "https://example.com/o/authorize",
            &[("client_id", CLIENT_ID_PLACEHOLDER), ("scope", "upload")],
            "fixed-state",
        );
        assert_eq!(challenge.state, "fixed-state");
        assert!(challenge.url.starts_with("https://example.com/o/authorize?"));
        assert!(challenge.url.contains("client_id=YOUR_CLIENT_ID_HERE"));
        assert!(challenge.url.contains("scope=upload"));
        assert!(challenge.url.contains("state=fixed-state"));
        // redirect_uri is encoded.
        assert!(
            challenge.url.contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A8419%2Foauth%2Fcallback"),
            "got {}",
            challenge.url,
        );
    }
}
