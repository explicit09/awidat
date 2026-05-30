//! TikTok Content Posting API stub provider.
//!
//! Real upload (init upload via
//! `https://open.tiktokapis.com/v2/post/publish/video/init/`, then
//! PUT chunks to the returned `upload_url`) lands in W5.A3.
//!
//! # When this becomes real
//!
//! The user has to register a TikTok app at
//! <https://developers.tiktok.com/> with the Content Posting API
//! scope, get the app approved (TikTok manually reviews apps that
//! request video.upload), and paste `client_key` + `client_secret`
//! into `<config_dir>/awidat/publishing.json`.

use std::path::PathBuf;

use async_trait::async_trait;

use super::errors::ProviderError;
use super::oauth::{
    build_authorize, client_id_for, disconnect_provider, fresh_state,
    get_client_credentials_state, has_credentials, load_status, set_client_credentials,
    stub_complete_oauth, stub_upload, ClientCredentialsState,
};
use super::provider::PublishingProvider;
use super::types::{ConnectionStatus, OAuthChallenge, UploadParams, UploadResult};

/// Stable provider key.
pub const KEY: &str = "tiktok";

/// TikTok's OAuth 2.0 authorize endpoint.
const AUTHORIZE_URL: &str = "https://www.tiktok.com/v2/auth/authorize/";

/// Scopes for video upload + reading the account display name.
/// `video.upload` requires manual review on TikTok's side.
const SCOPES: &str = "user.info.basic,video.upload";

const DEV_CONSOLE_URL: &str = "https://developers.tiktok.com/";

/// TikTok's AIGC (AI-generated content) label flag. When the W5.A4
/// disclosure flags synthetic content the real upload will set
/// `aigc_label_type` in the post info — surfacing the "AI-generated"
/// badge on the video. The stub folds the name into its log line.
const AI_DISCLOSURE_FLAG: &str = "aigc_label";

/// TikTok provider. Owns its credential-store path for testability.
pub struct TiktokProvider {
    store_path: PathBuf,
}

impl TiktokProvider {
    /// Construct using an explicit credentials path. Tests pass a
    /// tempdir path; production wires it through
    /// [`super::ProviderRegistry::new`].
    pub fn with_store_path(store_path: PathBuf) -> Self {
        Self { store_path }
    }
}

#[async_trait]
impl PublishingProvider for TiktokProvider {
    fn key(&self) -> &'static str {
        KEY
    }

    fn display_name(&self) -> &'static str {
        "TikTok"
    }

    async fn is_configured(&self) -> bool {
        has_credentials(&self.store_path, KEY).await
    }

    async fn begin_oauth(&self) -> Result<OAuthChallenge, ProviderError> {
        // W5.A5: substitute the user's BYO client_key when set. TikTok
        // calls the field `client_key` rather than `client_id` but
        // semantically it's the same — we store under the canonical
        // `client_id` name and rewrite at URL-build time.
        let client_key = client_id_for(&self.store_path, KEY).await;
        Ok(build_authorize(
            AUTHORIZE_URL,
            &[
                ("client_key", client_key.as_str()),
                ("response_type", "code"),
                ("scope", SCOPES),
            ],
            &fresh_state(),
        ))
    }

    async fn complete_oauth(&self, code: String) -> Result<(), ProviderError> {
        // TODO(W5.A3): POST `code` to
        // https://open.tiktokapis.com/v2/oauth/token/ to exchange
        // for an access_token + refresh_token. TikTok's tokens are
        // short-lived (1 day for access, 365 days for refresh) so
        // we'll need a background refresh task once the real impl
        // lands.
        stub_complete_oauth(&self.store_path, KEY, code).await
    }

    async fn upload(&self, params: UploadParams) -> Result<UploadResult, ProviderError> {
        // TODO(W5.A3+): when synthetic content is present, also set
        // `aigc_label_type` in the publish init body so TikTok shows
        // the AIGC badge. Today the disclosure intent rides on
        // `params.ai_disclosure`; `stub_upload` folds it into the log
        // line + Unsupported message until the real call lands.
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
