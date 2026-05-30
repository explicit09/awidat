//! W6.A2 — real OAuth `authorization_code → token` exchange + refresh.
//!
//! The Wave 6.A1 OAuth listener captures the redirect's `code`. The
//! Wave 6.A4 keychain wrapper stores the resulting access / refresh
//! tokens. This module is the missing middle: a thin HTTPS client that
//! POSTs the code to the provider's `/token` endpoint and parses the
//! reply into a [`TokenResponse`].
//!
//! The same module also handles the *refresh* half of the lifecycle —
//! before every upload we call [`ensure_fresh_token`], which refreshes
//! the access token a minute before it expires so the in-flight HTTP
//! call never trips on a stale 401.
//!
//! # Why endpoint-parameterised
//!
//! `exchange_code_for_token` takes the token endpoint as an argument
//! instead of hardcoding YouTube's URL. This lets the same helper drive
//! TikTok and Instagram once their stubs catch up (the request body
//! shape differs slightly per provider, but the URL plumbing is
//! identical) and — more importantly today — lets the unit tests point
//! at a `wiremock` server without monkey-patching globals.
//!
//! # Token secrecy
//!
//! Per the W6.A4 contract: tokens never appear in `tracing::error!`
//! lines. The error mapping below uses `Provider error: invalid_grant`
//! on the wire and routes the actual token / response body through
//! `tracing::debug!` only.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Deserialize;

use super::errors::ProviderError;
use super::keychain::{Keychain, TokenKind};
use super::storage::{self, keychain_to_provider};

/// Margin we refresh ahead of `expires_at` to absorb clock skew and
/// network latency. Google's access tokens are valid for ~3600s so a
/// 60-second pre-expiry refresh is one ~1.7% of the window — small
/// enough that we don't burn refresh quota and large enough to cover a
/// slow round-trip.
///
/// `#[allow(dead_code)]` because the constant is only read from inside
/// `ensure_fresh_token` (which is itself currently called only by
/// tests — W6.A3 will wire it onto the upload hot path).
#[allow(dead_code)]
pub const REFRESH_LEAD_SECS: i64 = 60;

/// HTTP request timeout for token exchange / refresh / userinfo. Real
/// Google responses are <500ms; 30s leaves room for a slow corporate
/// proxy and is short enough that a hung connection doesn't strand the
/// upload queue.
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);

/// Parsed shape of the JSON Google (and most OAuth 2.0 providers)
/// return from `/token`.
///
/// Field names match the OAuth 2.0 spec (`snake_case`); the wrapper
/// converts `expires_in` → an `i64` for friction-free comparison
/// against `Credentials::expires_at`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenResponse {
    /// Bearer token to set on subsequent API requests.
    pub access_token: String,
    /// Long-lived token used to mint a fresh `access_token` without the
    /// user re-running the OAuth flow. Optional because refresh responses
    /// often omit it (the original refresh token stays valid).
    pub refresh_token: Option<String>,
    /// Lifetime of `access_token` in seconds — Google ships 3600.
    pub expires_in_secs: i64,
    /// `"Bearer"` for Google / TikTok / Meta. Kept on the struct so
    /// downstream callers can construct the `Authorization` header
    /// without re-hardcoding the scheme name.
    pub token_type: String,
    /// Space-separated scope string the provider actually granted.
    /// Stored so future debugging can confirm the user consented to the
    /// upload scope and not just `userinfo`.
    pub scope: String,
}

/// Wire-shape mirror of `TokenResponse` — kept private so the public
/// type can ship with friendlier field names (`expires_in_secs` rather
/// than `expires_in`).
#[derive(Debug, Deserialize)]
struct RawTokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<i64>,
    #[serde(default = "default_token_type")]
    token_type: String,
    #[serde(default)]
    scope: String,
}

fn default_token_type() -> String {
    "Bearer".to_string()
}

impl From<RawTokenResponse> for TokenResponse {
    fn from(raw: RawTokenResponse) -> Self {
        Self {
            access_token: raw.access_token,
            refresh_token: raw.refresh_token.filter(|s| !s.is_empty()),
            // Google always sends `expires_in`; fall back to one hour
            // when missing so we don't store a sentinel that makes
            // `ensure_fresh_token` think the token is already expired.
            expires_in_secs: raw.expires_in.unwrap_or(3600),
            token_type: raw.token_type,
            scope: raw.scope,
        }
    }
}

/// Error-response shape per RFC 6749 §5.2.
#[derive(Debug, Deserialize)]
struct OAuthErrorBody {
    error: String,
    #[serde(default)]
    error_description: Option<String>,
}

/// POST the `authorization_code` exchange. Body is
/// `application/x-www-form-urlencoded` per RFC 6749 §4.1.3.
///
/// Returns [`ProviderError::NetworkError`] on transport failure,
/// [`ProviderError::OAuthFailed`] when the provider replies with an
/// OAuth error body, [`ProviderError::Io`] when the JSON shape is
/// unparseable. The error string never includes the access / refresh
/// token values themselves — only the error code from the provider.
pub async fn exchange_code_for_token(
    token_endpoint: &str,
    code: String,
    client_id: String,
    client_secret: String,
    redirect_uri: String,
) -> Result<TokenResponse, ProviderError> {
    let form = [
        ("code", code.as_str()),
        ("client_id", client_id.as_str()),
        ("client_secret", client_secret.as_str()),
        ("redirect_uri", redirect_uri.as_str()),
        ("grant_type", "authorization_code"),
    ];
    post_token_form(token_endpoint, &form).await
}

/// POST the `refresh_token` exchange. Same wire shape as
/// [`exchange_code_for_token`] but a different `grant_type` and a
/// shorter form-field set (no `code`, no `redirect_uri`).
///
/// `#[allow(dead_code)]` because production callers route through
/// `ensure_fresh_token`; this lower-level entry point is the unit test
/// hook + the future W6.A3 upload integration point.
#[allow(dead_code)]
pub async fn refresh_token(
    token_endpoint: &str,
    refresh_token: String,
    client_id: String,
    client_secret: String,
) -> Result<TokenResponse, ProviderError> {
    let form = [
        ("refresh_token", refresh_token.as_str()),
        ("client_id", client_id.as_str()),
        ("client_secret", client_secret.as_str()),
        ("grant_type", "refresh_token"),
    ];
    post_token_form(token_endpoint, &form).await
}

/// Single shared POST → parse path. Centralised so the exchange and
/// refresh helpers share their error mapping (the OAuth error response
/// shape is the same for both grant types).
async fn post_token_form(
    token_endpoint: &str,
    form: &[(&str, &str)],
) -> Result<TokenResponse, ProviderError> {
    let client = build_client()?;
    let response = client
        .post(token_endpoint)
        .form(form)
        .send()
        .await
        .map_err(|e| {
            // Don't include the form body in this log line — the
            // `client_secret` rides in it.
            tracing::debug!(endpoint = token_endpoint, error = %e, "oauth token POST failed");
            ProviderError::NetworkError(format!("token endpoint POST failed: {e}"))
        })?;

    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|e| ProviderError::NetworkError(format!("read token endpoint response: {e}")))?;

    if !status.is_success() {
        // RFC 6749 §5.2: error responses are JSON with `error` + optional
        // `error_description`. Pull just the `error` code into the
        // surfaced message so we don't leak whatever Google decided to
        // echo back. The full body lands in `tracing::debug!` for ops.
        tracing::debug!(
            status = status.as_u16(),
            body = body.as_str(),
            "oauth token endpoint returned non-success",
        );
        let summary = match serde_json::from_str::<OAuthErrorBody>(&body) {
            Ok(parsed) => match parsed.error_description {
                Some(desc) => format!("{}: {desc}", parsed.error),
                None => parsed.error,
            },
            Err(_) => format!("HTTP {} from token endpoint", status.as_u16()),
        };
        return Err(ProviderError::OAuthFailed(summary));
    }

    let raw: RawTokenResponse = serde_json::from_str(&body).map_err(|e| {
        // The body could include the access token here — only debug-log
        // the parse error, not the body itself.
        tracing::debug!(error = %e, "oauth token response parse failed");
        ProviderError::Io(format!("parse token response: {e}"))
    })?;
    Ok(raw.into())
}

/// Build the shared HTTPS client. Kept as a function (rather than a
/// `OnceLock` singleton) because each `Client` holds an OS-level
/// connection pool and we don't want a stuck pool to outlive a single
/// OAuth attempt. Construction is microsecond-cheap so per-call is fine.
fn build_client() -> Result<reqwest::Client, ProviderError> {
    reqwest::Client::builder()
        .timeout(HTTP_TIMEOUT)
        // Google's token endpoint occasionally serves a redirect on a
        // misconfigured DNS lookup — leave reqwest's defaults intact.
        .build()
        .map_err(|e| ProviderError::Io(format!("build reqwest client: {e}")))
}

/// Read the `userinfo` endpoint (Google's OIDC profile call) to discover
/// the connected account's display name / email. Best-effort: a failure
/// here is cosmetic so we return `None` rather than propagating the
/// error up into `complete_oauth`.
pub async fn fetch_account_name(userinfo_endpoint: &str, access_token: &str) -> Option<String> {
    let client = build_client().ok()?;
    let response = client
        .get(userinfo_endpoint)
        .bearer_auth(access_token)
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        tracing::debug!(
            endpoint = userinfo_endpoint,
            status = response.status().as_u16(),
            "userinfo endpoint returned non-success",
        );
        return None;
    }
    let body = response.text().await.ok()?;
    // Google's userinfo: `{ "sub": "...", "name": "...", "email": "..." }`.
    // Email is the most user-recognisable; fall back to `name` then
    // `sub` so we always produce *something* if any field is present.
    let v: serde_json::Value = serde_json::from_str(&body).ok()?;
    v.get("email")
        .and_then(|x| x.as_str())
        .or_else(|| v.get("name").and_then(|x| x.as_str()))
        .or_else(|| v.get("sub").and_then(|x| x.as_str()))
        .map(str::to_string)
}

/// Persist a freshly-minted [`TokenResponse`] into the keychain +
/// `publishing.json`. Centralised so `complete_oauth` and
/// `ensure_fresh_token` share the write path (and any future
/// instrumentation hooks).
///
/// - access_token → keychain `{provider}-access_token`
/// - refresh_token (if present) → keychain `{provider}-refresh_token`
/// - `expires_at = now + expires_in_secs` → `publishing.json`
/// - `account_name` (if `Some`) → `publishing.json`
pub async fn persist_token_response(
    store_path: &std::path::Path,
    provider_key: &str,
    tokens: &TokenResponse,
    account_name: Option<String>,
) -> Result<(), ProviderError> {
    let kc = Keychain::new();
    kc.store_token(provider_key, TokenKind::AccessToken, &tokens.access_token)
        .map_err(keychain_to_provider)?;
    if let Some(ref refresh) = tokens.refresh_token {
        kc.store_token(provider_key, TokenKind::RefreshToken, refresh)
            .map_err(keychain_to_provider)?;
    }

    let expires_at = now_unix_seconds().saturating_add(tokens.expires_in_secs);

    let mut store = storage::load_from(store_path).await?;
    let slot = store.get_or_insert(provider_key);
    slot.expires_at = Some(expires_at);
    if let Some(name) = account_name {
        slot.account_name = Some(name);
    }
    slot.migrated_at.get_or_insert_with(now_unix_seconds);
    storage::save_to(store_path, &store).await
}

/// Return a valid access token for `provider_key`, refreshing it if it's
/// within [`REFRESH_LEAD_SECS`] of expiry.
///
/// Resolution rules:
/// 1. No access token at all → [`ProviderError::NotConfigured`] — the
///    user needs to run the OAuth flow.
/// 2. Access token present + `expires_at` in the future (with lead
///    margin) → return as-is, no network call.
/// 3. Expiring / expired + refresh token present → POST to
///    `token_endpoint` with `grant_type=refresh_token`, persist the new
///    pair, return the new access token.
/// 4. Expiring / expired + no refresh token → [`ProviderError::NotConfigured`]
///    with a hint to reconnect.
///
/// `client_id` / `client_secret` are passed in (rather than read here)
/// so the caller — usually `youtube.rs::upload` — can use whatever BYO
/// credential lookup matches its own naming conventions.
///
/// `#[allow(dead_code)]` because the production wiring lands in W6.A3
/// when `youtube.rs::upload` calls this before each `videos.insert`
/// request. Tests already exercise the full state machine.
#[allow(dead_code)]
pub async fn ensure_fresh_token(
    store_path: &std::path::Path,
    provider_key: &str,
    token_endpoint: &str,
    client_id: String,
    client_secret: String,
) -> Result<String, ProviderError> {
    let kc = Keychain::new();
    let access = kc
        .read_token(provider_key, TokenKind::AccessToken)
        .map_err(keychain_to_provider)?
        .filter(|s| !s.is_empty())
        .ok_or(ProviderError::NotConfigured)?;

    let expires_at = storage::load_from(store_path)
        .await
        .ok()
        .and_then(|s| s.get(provider_key).cloned())
        .and_then(|c| c.expires_at);

    let now = now_unix_seconds();
    let needs_refresh = match expires_at {
        // No stored expiry — treat as "good for one hour" rather than
        // forcing a refresh, since legacy slots from the stub path don't
        // carry a value. The first real exchange repopulates the field.
        None => false,
        Some(exp) => now >= exp.saturating_sub(REFRESH_LEAD_SECS),
    };

    if !needs_refresh {
        return Ok(access);
    }

    // Refresh path. Missing refresh token → tell the caller they need
    // to reconnect. We deliberately don't crash on a refresh failure
    // either — surface NotConfigured so the frontend pops the connect
    // sheet instead of a generic error.
    let refresh = kc
        .read_token(provider_key, TokenKind::RefreshToken)
        .map_err(keychain_to_provider)?
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            tracing::debug!(
                provider = provider_key,
                "access token expired and no refresh token on file — user must reconnect",
            );
            ProviderError::NotConfigured
        })?;

    let new_tokens = refresh_token(token_endpoint, refresh, client_id, client_secret)
        .await
        .map_err(|e| {
            tracing::debug!(
                provider = provider_key,
                error = %e,
                "refresh_token POST failed — treating as NotConfigured so the user reconnects",
            );
            ProviderError::NotConfigured
        })?;

    // Don't pull a fresh `account_name` on refresh — that requires a
    // separate userinfo call and isn't worth the latency / quota on
    // the hot path. Email rarely changes between refreshes anyway.
    persist_token_response(store_path, provider_key, &new_tokens, None).await?;
    Ok(new_tokens.access_token)
}

/// Unix epoch seconds — duplicates `storage::now_unix_seconds` rather
/// than re-exporting to keep this module's dep graph thin.
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
    use wiremock::matchers::{body_string_contains, header, method, path as match_path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// Generate a unique provider key per test so parallel runs using
    /// the shared in-process mock keychain don't observe each other.
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

    #[tokio::test]
    async fn exchange_code_for_token_happy_path() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(match_path("/token"))
            .and(header("content-type", "application/x-www-form-urlencoded"))
            .and(body_string_contains("grant_type=authorization_code"))
            .and(body_string_contains("code=auth-code-xyz"))
            .and(body_string_contains("client_id=cid"))
            .and(body_string_contains("client_secret=csec"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "ya29.access-abc",
                "refresh_token": "1//refresh-xyz",
                "expires_in": 3599,
                "token_type": "Bearer",
                "scope": "https://www.googleapis.com/auth/youtube.upload",
            })))
            .mount(&server)
            .await;

        let endpoint = format!("{}/token", server.uri());
        let tokens = exchange_code_for_token(
            &endpoint,
            "auth-code-xyz".into(),
            "cid".into(),
            "csec".into(),
            "http://127.0.0.1:8419/oauth/callback".into(),
        )
        .await
        .expect("exchange ok");

        assert_eq!(tokens.access_token, "ya29.access-abc");
        assert_eq!(tokens.refresh_token.as_deref(), Some("1//refresh-xyz"));
        assert_eq!(tokens.expires_in_secs, 3599);
        assert_eq!(tokens.token_type, "Bearer");
        assert!(tokens.scope.contains("youtube.upload"));
    }

    #[tokio::test]
    async fn exchange_code_for_token_invalid_grant_returns_oauth_failed() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(match_path("/token"))
            .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
                "error": "invalid_grant",
                "error_description": "Bad authorization code.",
            })))
            .mount(&server)
            .await;

        let endpoint = format!("{}/token", server.uri());
        let err = exchange_code_for_token(
            &endpoint,
            "bad-code".into(),
            "cid".into(),
            "csec".into(),
            "redir".into(),
        )
        .await
        .expect_err("must error on 400");
        assert_eq!(err.kind(), "oauth_failed");
        let msg = format!("{err}");
        assert!(msg.contains("invalid_grant"), "got {msg}");
        assert!(msg.contains("Bad authorization code"), "got {msg}");
    }

    #[tokio::test]
    async fn exchange_code_for_token_html_error_body_falls_back_to_status() {
        // Real Google sometimes returns an HTML maintenance page when
        // /token is brown-out. Make sure the JSON-parse failure doesn't
        // collapse into a confusing "io" error.
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(match_path("/token"))
            .respond_with(ResponseTemplate::new(503).set_body_string("<html>oops</html>"))
            .mount(&server)
            .await;
        let endpoint = format!("{}/token", server.uri());
        let err =
            exchange_code_for_token(&endpoint, "c".into(), "i".into(), "s".into(), "r".into())
                .await
                .expect_err("must error on 503");
        assert_eq!(err.kind(), "oauth_failed");
        assert!(format!("{err}").contains("503"), "got {err}");
    }

    #[tokio::test]
    async fn refresh_token_happy_path() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(match_path("/token"))
            .and(body_string_contains("grant_type=refresh_token"))
            .and(body_string_contains("refresh_token=1%2Frefresh-abc"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "ya29.new-access",
                "expires_in": 3600,
                "token_type": "Bearer",
                "scope": "https://www.googleapis.com/auth/youtube.upload",
            })))
            .mount(&server)
            .await;

        let endpoint = format!("{}/token", server.uri());
        let tokens = refresh_token(
            &endpoint,
            "1/refresh-abc".into(),
            "cid".into(),
            "csec".into(),
        )
        .await
        .expect("refresh ok");
        assert_eq!(tokens.access_token, "ya29.new-access");
        // Refresh responses commonly omit refresh_token — must surface as None.
        assert!(tokens.refresh_token.is_none());
        assert_eq!(tokens.expires_in_secs, 3600);
    }

    #[tokio::test]
    async fn ensure_fresh_token_returns_existing_when_not_expired() {
        install_mock_backend();
        let provider = unique_provider("fresh-ok");
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("publishing.json");

        // Seed: a stored access token whose `expires_at` is comfortably
        // in the future. ensure_fresh_token must NOT touch the network.
        Keychain::new()
            .store_token(&provider, TokenKind::AccessToken, "ya29.still-valid")
            .expect("store access");
        let mut store = storage::load_from(&path).await.expect("load");
        let slot = store.get_or_insert(&provider);
        slot.expires_at = Some(now_unix_seconds() + 3600);
        storage::save_to(&path, &store).await.expect("save");

        // Use a deliberately bogus endpoint to prove no network call happens.
        let token = ensure_fresh_token(
            &path,
            &provider,
            "http://127.0.0.1:1/should-not-be-called",
            "cid".into(),
            "csec".into(),
        )
        .await
        .expect("ensure ok");
        assert_eq!(token, "ya29.still-valid");
    }

    #[tokio::test]
    async fn ensure_fresh_token_refreshes_when_expired() {
        install_mock_backend();
        let provider = unique_provider("fresh-exp");
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("publishing.json");

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(match_path("/token"))
            .and(body_string_contains("grant_type=refresh_token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "ya29.refreshed",
                "expires_in": 3600,
                "token_type": "Bearer",
                "scope": "x",
            })))
            .mount(&server)
            .await;

        let kc = Keychain::new();
        kc.store_token(&provider, TokenKind::AccessToken, "ya29.old")
            .expect("store access");
        kc.store_token(&provider, TokenKind::RefreshToken, "1/refresh")
            .expect("store refresh");
        let mut store = storage::load_from(&path).await.expect("load");
        let slot = store.get_or_insert(&provider);
        slot.expires_at = Some(now_unix_seconds() - 1);
        storage::save_to(&path, &store).await.expect("save");

        let endpoint = format!("{}/token", server.uri());
        let token = ensure_fresh_token(&path, &provider, &endpoint, "cid".into(), "csec".into())
            .await
            .expect("ensure ok");
        assert_eq!(token, "ya29.refreshed");
        // Keychain reflects the new token + the *original* refresh token
        // (the response omitted refresh_token, which is the common case).
        assert_eq!(
            kc.read_token(&provider, TokenKind::AccessToken)
                .expect("read")
                .as_deref(),
            Some("ya29.refreshed"),
        );
        assert_eq!(
            kc.read_token(&provider, TokenKind::RefreshToken)
                .expect("read")
                .as_deref(),
            Some("1/refresh"),
        );
        // expires_at moved forward.
        let reloaded = storage::load_from(&path).await.expect("reload");
        let creds = reloaded.get(&provider).expect("slot");
        assert!(creds.expires_at.expect("ea") > now_unix_seconds() + 3500);
    }

    #[tokio::test]
    async fn ensure_fresh_token_no_refresh_token_returns_not_configured() {
        install_mock_backend();
        let provider = unique_provider("fresh-noref");
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("publishing.json");

        let kc = Keychain::new();
        kc.store_token(&provider, TokenKind::AccessToken, "ya29.old")
            .expect("store access");
        let mut store = storage::load_from(&path).await.expect("load");
        let slot = store.get_or_insert(&provider);
        slot.expires_at = Some(now_unix_seconds() - 1);
        storage::save_to(&path, &store).await.expect("save");

        let err = ensure_fresh_token(
            &path,
            &provider,
            "http://127.0.0.1:1/never",
            "cid".into(),
            "csec".into(),
        )
        .await
        .expect_err("must error without refresh token");
        assert_eq!(err.kind(), "not_configured");
    }

    #[tokio::test]
    async fn ensure_fresh_token_refresh_failure_returns_not_configured() {
        install_mock_backend();
        let provider = unique_provider("fresh-fail");
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("publishing.json");

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(match_path("/token"))
            .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
                "error": "invalid_grant",
            })))
            .mount(&server)
            .await;

        let kc = Keychain::new();
        kc.store_token(&provider, TokenKind::AccessToken, "ya29.old")
            .expect("store access");
        kc.store_token(&provider, TokenKind::RefreshToken, "1/refresh")
            .expect("store refresh");
        let mut store = storage::load_from(&path).await.expect("load");
        let slot = store.get_or_insert(&provider);
        slot.expires_at = Some(now_unix_seconds() - 1);
        storage::save_to(&path, &store).await.expect("save");

        let endpoint = format!("{}/token", server.uri());
        let err = ensure_fresh_token(&path, &provider, &endpoint, "cid".into(), "csec".into())
            .await
            .expect_err("must error on refresh failure");
        // Refresh failures collapse to NotConfigured so the UI prompts
        // the user to reconnect rather than retry a doomed request.
        assert_eq!(err.kind(), "not_configured");
    }

    #[tokio::test]
    async fn ensure_fresh_token_missing_access_returns_not_configured() {
        install_mock_backend();
        let provider = unique_provider("fresh-missing");
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("publishing.json");
        let err = ensure_fresh_token(
            &path,
            &provider,
            "http://127.0.0.1:1/never",
            "cid".into(),
            "csec".into(),
        )
        .await
        .expect_err("must error without any tokens");
        assert_eq!(err.kind(), "not_configured");
    }

    #[tokio::test]
    async fn fetch_account_name_prefers_email() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(match_path("/userinfo"))
            .and(header("authorization", "Bearer ya29.test"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "sub": "1234",
                "name": "Test User",
                "email": "user@example.com",
            })))
            .mount(&server)
            .await;
        let endpoint = format!("{}/userinfo", server.uri());
        let name = fetch_account_name(&endpoint, "ya29.test").await;
        assert_eq!(name.as_deref(), Some("user@example.com"));
    }

    #[tokio::test]
    async fn fetch_account_name_failure_returns_none() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(match_path("/userinfo"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;
        let endpoint = format!("{}/userinfo", server.uri());
        let name = fetch_account_name(&endpoint, "bad").await;
        assert!(name.is_none(), "401 must collapse to None");
    }

    #[tokio::test]
    async fn persist_token_response_writes_keychain_and_metadata() {
        install_mock_backend();
        let provider = unique_provider("persist");
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("publishing.json");

        let tokens = TokenResponse {
            access_token: "ya29.fresh".into(),
            refresh_token: Some("1//ref".into()),
            expires_in_secs: 3600,
            token_type: "Bearer".into(),
            scope: "s".into(),
        };
        persist_token_response(&path, &provider, &tokens, Some("you@gmail.com".into()))
            .await
            .expect("persist ok");

        let kc = Keychain::new();
        assert_eq!(
            kc.read_token(&provider, TokenKind::AccessToken)
                .expect("read access")
                .as_deref(),
            Some("ya29.fresh"),
        );
        assert_eq!(
            kc.read_token(&provider, TokenKind::RefreshToken)
                .expect("read refresh")
                .as_deref(),
            Some("1//ref"),
        );
        let store = storage::load_from(&path).await.expect("load");
        let creds = store.get(&provider).expect("slot");
        assert_eq!(creds.account_name.as_deref(), Some("you@gmail.com"));
        let now = now_unix_seconds();
        let ea = creds.expires_at.expect("ea");
        assert!(
            ea >= now + 3500 && ea <= now + 3700,
            "expires_at = {ea}, now = {now}"
        );
        // No token material leaks into the JSON file.
        let raw = tokio::fs::read_to_string(&path).await.expect("read");
        assert!(!raw.contains("ya29.fresh"));
        assert!(!raw.contains("1//ref"));
    }
}
