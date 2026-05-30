//! YouTube Data API v3 stub provider.
//!
//! Real upload (resumable PUT to
//! `https://www.googleapis.com/upload/youtube/v3/videos`) lands in
//! W5.A2; this file just wires the trait so the rest of the system
//! (Tauri commands, registry, Settings UI) compiles end-to-end.
//!
//! # When this becomes real
//!
//! The user has to register an OAuth 2.0 Client ID at
//! <https://console.cloud.google.com/apis/credentials>, enable the
//! YouTube Data API v3, and paste the `client_id` + `client_secret`
//! into `<config_dir>/awidat/publishing.json` (or, post W5.A4, the
//! Settings UI). Until then `begin_oauth` returns a URL with a literal
//! `YOUR_CLIENT_ID_HERE` placeholder.

use std::path::PathBuf;

use async_trait::async_trait;

use super::errors::ProviderError;
use super::keychain::{Keychain, TokenKind};
use super::oauth::{
    build_authorize, client_id_for, disconnect_provider, fresh_state,
    get_client_credentials_state, has_credentials, load_status, set_client_credentials,
    ClientCredentialsState, REDIRECT_URI,
};
use super::oauth_exchange::{
    ensure_fresh_token, exchange_code_for_token, fetch_account_name, persist_token_response,
};
use super::provider::PublishingProvider;
use super::storage::keychain_to_provider;
use super::types::{ConnectionStatus, OAuthChallenge, UploadParams, UploadResult};
use super::youtube_upload::upload_video;

/// Stable provider key — referenced by Tauri commands and storage.
pub const KEY: &str = "youtube";

/// YouTube's OAuth 2.0 authorize endpoint.
const AUTHORIZE_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";

/// Google's OAuth 2.0 token-exchange endpoint. Same for every Google
/// API (YouTube Data, Drive, Calendar, …) — they all share OIDC.
const TOKEN_ENDPOINT: &str = "https://oauth2.googleapis.com/token";

/// Google's OIDC userinfo endpoint. We call it once at OAuth completion
/// time to pick up the account's email for the Settings display.
const USERINFO_ENDPOINT: &str = "https://www.googleapis.com/oauth2/v3/userinfo";

/// Minimum scope for uploading via the Data API. `youtube.upload` is
/// the narrowest scope that lets us POST a video; `youtube.readonly`
/// gives us the channel name to display in Settings.
const SCOPES: &str =
    "https://www.googleapis.com/auth/youtube.upload https://www.googleapis.com/auth/youtube.readonly";

/// Developer-console URL we point the user at when credentials are
/// missing. Surfaced in error messages so the user can click through.
const DEV_CONSOLE_URL: &str = "https://console.cloud.google.com/apis/credentials";

/// YouTube's `videos.insert` AI-disclosure field name. When the W5.A4
/// disclosure flags synthetic content the real upload will set this
/// to `true` (videoStatus.containsSyntheticMedia in the Data API).
/// The stub folds the name into its log line so the user can see
/// what would have been claimed.
const AI_DISCLOSURE_FLAG: &str = "alteredContent";

/// YouTube provider. Owns its credential-store path so tests can
/// sandbox it via [`YoutubeProvider::with_store_path`] without env-
/// var racing.
pub struct YoutubeProvider {
    store_path: PathBuf,
}

impl YoutubeProvider {
    /// Construct using an explicit credentials path. Tests pass a
    /// tempdir path; production wires it through
    /// [`super::ProviderRegistry::new`] which resolves the default
    /// `<config_dir>/awidat/publishing.json` location.
    pub fn with_store_path(store_path: PathBuf) -> Self {
        Self { store_path }
    }
}

#[async_trait]
impl PublishingProvider for YoutubeProvider {
    fn key(&self) -> &'static str {
        KEY
    }

    fn display_name(&self) -> &'static str {
        "YouTube"
    }

    async fn is_configured(&self) -> bool {
        has_credentials(&self.store_path, KEY).await
    }

    async fn begin_oauth(&self) -> Result<OAuthChallenge, ProviderError> {
        // W5.A5: substitute the user's BYO client_id when set, fall
        // back to the placeholder so the URL still looks correct in
        // dev mode (the platform will reject the placeholder — that's
        // the expected error path until the user pastes real creds).
        let client_id = client_id_for(&self.store_path, KEY).await;
        Ok(build_authorize(
            AUTHORIZE_URL,
            &[
                ("client_id", client_id.as_str()),
                ("response_type", "code"),
                ("scope", SCOPES),
                ("access_type", "offline"),
                ("prompt", "consent"),
            ],
            &fresh_state(),
        ))
    }

    async fn complete_oauth(&self, code: String) -> Result<(), ProviderError> {
        // W6.A2: real token exchange. Read BYO client credentials from
        // storage + keychain, POST to Google's /token endpoint, persist
        // the returned access / refresh tokens, then best-effort look
        // up the user's email for the Settings display label.
        let code = code.trim().to_string();
        if code.is_empty() {
            return Err(ProviderError::OAuthFailed(
                "empty authorization code".into(),
            ));
        }

        let client_id = read_byo_client_id(&self.store_path).await?;
        let client_secret = read_byo_client_secret()?;

        let tokens = exchange_code_for_token(
            TOKEN_ENDPOINT,
            code,
            client_id,
            client_secret,
            REDIRECT_URI.to_string(),
        )
        .await?;

        // Cosmetic-only label lookup. A failure here leaves the
        // Settings UI showing the bare "Connected" pill rather than
        // blocking the whole flow on a userinfo hiccup.
        let account_name = fetch_account_name(USERINFO_ENDPOINT, &tokens.access_token).await;

        persist_token_response(&self.store_path, KEY, &tokens, account_name).await
    }

    async fn upload(&self, params: UploadParams) -> Result<UploadResult, ProviderError> {
        // W6.A3 — real resumable upload. The contract is:
        //
        //   1. Refresh the OAuth token if it's near expiry (auto-flow
        //      from W6.A2). NotConfigured here pops the user back into
        //      the connect sheet rather than letting a 401 land in the
        //      middle of a chunk upload.
        //   2. Hand the file off to `youtube_upload::upload_video`,
        //      bridging the params' `progress_tx` (set by the upload
        //      dispatcher) into the `on_progress` closure.
        //   3. The `containsSyntheticMedia` flag is set inside
        //      `build_metadata_body` based on `params.ai_disclosure` —
        //      see `youtube_upload.rs` for the wire-level wiring.
        let client_id = read_byo_client_id(&self.store_path).await?;
        let client_secret = read_byo_client_secret()?;
        let access_token =
            ensure_fresh_token(&self.store_path, KEY, TOKEN_ENDPOINT, client_id, client_secret)
                .await?;

        // The disclosure flag name only matters to the stub log-line
        // path; the real upload writes `containsSyntheticMedia`
        // directly into the metadata body. Keep the constant referenced
        // so a future rename has a single grep target.
        let _ = AI_DISCLOSURE_FLAG;
        let _ = DEV_CONSOLE_URL;

        let progress_tx = params.progress_tx.clone();
        upload_video(access_token, params, move |fraction| {
            if let Some(tx) = progress_tx.as_ref() {
                // `send` only fails when the receiver has been dropped,
                // which means the dispatcher is no longer interested —
                // safe to swallow: it doesn't affect the upload itself.
                let _ = tx.send(fraction);
            }
        })
        .await
    }

    async fn status(&self) -> ConnectionStatus {
        load_status(&self.store_path, KEY).await
    }

    async fn disconnect(&self) -> Result<(), ProviderError> {
        disconnect_provider(&self.store_path, KEY).await
    }

    async fn set_client_credentials(
        &self,
        client_id: String,
        client_secret: String,
    ) -> Result<(), ProviderError> {
        set_client_credentials(&self.store_path, KEY, client_id, client_secret).await
    }

    async fn client_credentials_state(&self) -> ClientCredentialsState {
        get_client_credentials_state(&self.store_path, KEY).await
    }
}

/// Read the user's BYO `client_id` from `publishing.json`. Empty /
/// missing → `NotConfigured` with a pointer to Settings.
///
/// We deliberately reject the placeholder substitution here even though
/// `client_id_for` would happily return it — POSTing
/// `YOUR_CLIENT_ID_HERE` to Google's `/token` endpoint just yields a
/// cryptic `invalid_client` error from the platform. Better to short
/// the user into the Settings flow with a clear message.
async fn read_byo_client_id(store_path: &std::path::Path) -> Result<String, ProviderError> {
    let id = client_id_for(store_path, KEY).await;
    if id.is_empty() || id == super::oauth::CLIENT_ID_PLACEHOLDER {
        return Err(ProviderError::NotConfigured);
    }
    Ok(id)
}

/// Read the user's BYO `client_secret` from the OS keychain. Empty /
/// missing → `NotConfigured` with the same "set credentials in
/// Settings" guidance.
fn read_byo_client_secret() -> Result<String, ProviderError> {
    let secret = Keychain::new()
        .read_token(KEY, TokenKind::ClientSecret)
        .map_err(keychain_to_provider)?
        .filter(|s| !s.is_empty())
        .ok_or(ProviderError::NotConfigured)?;
    Ok(secret)
}

/// Test-only entry point that does the same work as the trait's
/// `complete_oauth` but takes overrideable token / userinfo endpoints
/// so a wiremock server can stand in for Google. Production code uses
/// the trait method.
#[cfg(test)]
pub(crate) async fn complete_oauth_with_endpoints(
    store_path: &std::path::Path,
    code: String,
    token_endpoint: &str,
    userinfo_endpoint: &str,
) -> Result<(), ProviderError> {
    let code = code.trim().to_string();
    if code.is_empty() {
        return Err(ProviderError::OAuthFailed(
            "empty authorization code".into(),
        ));
    }
    let client_id = read_byo_client_id(store_path).await?;
    let client_secret = read_byo_client_secret()?;
    let tokens = exchange_code_for_token(
        token_endpoint,
        code,
        client_id,
        client_secret,
        REDIRECT_URI.to_string(),
    )
    .await?;
    let account_name = fetch_account_name(userinfo_endpoint, &tokens.access_token).await;
    persist_token_response(store_path, KEY, &tokens, account_name).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::publishing::keychain::install_mock_backend;
    use crate::publishing::oauth::set_client_credentials;
    use crate::publishing::storage;
    use wiremock::matchers::{body_string_contains, method, path as match_path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn unique_suffix(prefix: &str) -> String {
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
    async fn complete_oauth_missing_byo_returns_not_configured() {
        install_mock_backend();
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join(unique_suffix("youtube") + ".json");
        // Trait call hits the real Google endpoints, but it should
        // short-circuit on the BYO check before ever opening a socket.
        let provider = YoutubeProvider::with_store_path(path);
        let err = provider
            .complete_oauth("any-code".into())
            .await
            .expect_err("must reject when BYO creds missing");
        assert_eq!(err.kind(), "not_configured");
    }

    #[tokio::test]
    async fn complete_oauth_real_flow_writes_to_keychain() {
        // Full end-to-end mock: configure BYO creds, stand up a wiremock
        // server for /token and /userinfo, drive the test-only
        // `complete_oauth_with_endpoints`, and verify the keychain +
        // metadata land where the trait method would land in production.
        install_mock_backend();
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join(unique_suffix("youtube") + ".json");

        // Seed BYO creds. We have to use a per-test storage path AND
        // override the global KEY for the keychain lookups. Since
        // read_byo_client_secret hardcodes KEY = "youtube", we have to
        // be tolerant of parallel tests fighting over that slot. The
        // wiremock server doesn't care what client_secret we send;
        // we just need *something* non-empty in the keychain.
        set_client_credentials(
            &path,
            KEY,
            "real-client-id.apps.googleusercontent.com".into(),
            "test-client-secret".into(),
        )
        .await
        .expect("seed BYO");

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(match_path("/token"))
            .and(body_string_contains("code=authcode-123"))
            .and(body_string_contains("grant_type=authorization_code"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "ya29.real-access",
                "refresh_token": "1//real-refresh",
                "expires_in": 3600,
                "token_type": "Bearer",
                "scope": "https://www.googleapis.com/auth/youtube.upload",
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(match_path("/userinfo"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "email": "creator@example.com",
            })))
            .mount(&server)
            .await;
        let token_endpoint = format!("{}/token", server.uri());
        let userinfo_endpoint = format!("{}/userinfo", server.uri());

        complete_oauth_with_endpoints(
            &path,
            "authcode-123".into(),
            &token_endpoint,
            &userinfo_endpoint,
        )
        .await
        .expect("complete ok");

        // Keychain landed both tokens.
        let kc = Keychain::new();
        assert_eq!(
            kc.read_token(KEY, TokenKind::AccessToken)
                .expect("read access")
                .as_deref(),
            Some("ya29.real-access"),
        );
        assert_eq!(
            kc.read_token(KEY, TokenKind::RefreshToken)
                .expect("read refresh")
                .as_deref(),
            Some("1//real-refresh"),
        );
        // Metadata picked up `account_name` from the userinfo response.
        let store = storage::load_from(&path).await.expect("load");
        let creds = store.get(KEY).expect("slot");
        assert_eq!(creds.account_name.as_deref(), Some("creator@example.com"));
        assert!(creds.expires_at.is_some());
        // JSON file never carries the secret material.
        let raw = tokio::fs::read_to_string(&path).await.expect("read");
        assert!(!raw.contains("ya29.real-access"));
        assert!(!raw.contains("1//real-refresh"));
        assert!(!raw.contains("test-client-secret"));
    }

    #[tokio::test]
    async fn complete_oauth_rejects_empty_code() {
        install_mock_backend();
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join(unique_suffix("youtube") + ".json");
        let provider = YoutubeProvider::with_store_path(path);
        let err = provider
            .complete_oauth("   ".into())
            .await
            .expect_err("must reject empty code");
        assert_eq!(err.kind(), "oauth_failed");
    }
}
