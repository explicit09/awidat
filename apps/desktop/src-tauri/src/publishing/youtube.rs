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
use super::oauth::{
    build_authorize, fresh_state, has_credentials, load_status, stub_complete_oauth,
    stub_upload, CLIENT_ID_PLACEHOLDER,
};
use super::provider::PublishingProvider;
use super::types::{ConnectionStatus, OAuthChallenge, UploadParams, UploadResult};

/// Stable provider key — referenced by Tauri commands and storage.
pub const KEY: &str = "youtube";

/// YouTube's OAuth 2.0 authorize endpoint.
const AUTHORIZE_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";

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
        // TODO(W5.A2): replace CLIENT_ID_PLACEHOLDER with the value
        // read from publishing.json once the user has registered a
        // Cloud Console project.
        Ok(build_authorize(
            AUTHORIZE_URL,
            &[
                ("client_id", CLIENT_ID_PLACEHOLDER),
                ("response_type", "code"),
                ("scope", SCOPES),
                ("access_type", "offline"),
                ("prompt", "consent"),
            ],
            &fresh_state(),
        ))
    }

    async fn complete_oauth(&self, code: String) -> Result<(), ProviderError> {
        // TODO(W5.A2): POST `code` to
        // https://oauth2.googleapis.com/token with client_id +
        // client_secret to exchange for access_token + refresh_token.
        stub_complete_oauth(&self.store_path, KEY, code).await
    }

    async fn upload(&self, params: UploadParams) -> Result<UploadResult, ProviderError> {
        // TODO(W5.A2+): when synthetic content is present, also set
        // `containsSyntheticMedia=true` on the `videos.insert` call
        // so YouTube surfaces the "Altered or synthetic content"
        // disclosure to viewers. Today the disclosure intent rides
        // on `params.ai_disclosure`; `stub_upload` folds it into the
        // log line + Unsupported message until the real call lands.
        stub_upload(
            &self.store_path,
            KEY,
            DEV_CONSOLE_URL,
            AI_DISCLOSURE_FLAG,
            params,
        )
        .await
    }

    async fn status(&self) -> ConnectionStatus {
        load_status(&self.store_path, KEY).await
    }
}
