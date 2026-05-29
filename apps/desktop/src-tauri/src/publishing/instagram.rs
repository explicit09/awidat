//! Instagram Graph API stub provider.
//!
//! Instagram uploads route through the Graph API (not the consumer
//! API) and require a Business / Creator account linked to a Facebook
//! Page. Real upload (POST to `/{ig_user_id}/media` to create a media
//! container, poll for `status_code=FINISHED`, then POST to
//! `/{ig_user_id}/media_publish`) lands in W5.A4.
//!
//! # When this becomes real
//!
//! The user has to:
//! 1. Register a Meta App at <https://developers.facebook.com/apps>.
//! 2. Add the "Instagram Graph API" product.
//! 3. Submit the app for review with the `instagram_content_publish`
//!    permission (Meta manually reviews; takes ~2 weeks).
//! 4. Convert their Instagram account to a Business / Creator account
//!    and link it to a Facebook Page they admin.
//!
//! This is the heaviest scaffolding gate of the three providers — IG
//! is the most restrictive about programmatic uploads.

use std::path::PathBuf;

use async_trait::async_trait;

use super::errors::ProviderError;
use super::oauth::{
    build_authorize, fresh_state, has_credentials, load_status, stub_complete_oauth,
    stub_upload, CLIENT_ID_PLACEHOLDER,
};
use super::provider::PublishingProvider;
use super::types::{ConnectionStatus, OAuthChallenge, UploadParams, UploadResult};

/// Stable provider key.
pub const KEY: &str = "instagram";

/// Facebook's OAuth 2.0 authorize endpoint — Instagram Graph uses
/// Facebook Login under the hood.
const AUTHORIZE_URL: &str = "https://www.facebook.com/v19.0/dialog/oauth";

/// Scopes for Instagram content publishing. `pages_show_list` and
/// `pages_read_engagement` are required to discover the IG user id
/// from the linked Facebook Page.
const SCOPES: &str =
    "instagram_basic,instagram_content_publish,pages_show_list,pages_read_engagement";

const DEV_CONSOLE_URL: &str = "https://developers.facebook.com/apps";

/// Instagram provider. Owns its credential-store path for testability.
pub struct InstagramProvider {
    store_path: PathBuf,
}

impl InstagramProvider {
    /// Construct using an explicit credentials path. Tests pass a
    /// tempdir path; production wires it through
    /// [`super::ProviderRegistry::new`].
    pub fn with_store_path(store_path: PathBuf) -> Self {
        Self { store_path }
    }
}

#[async_trait]
impl PublishingProvider for InstagramProvider {
    fn key(&self) -> &'static str {
        KEY
    }

    fn display_name(&self) -> &'static str {
        "Instagram"
    }

    async fn is_configured(&self) -> bool {
        has_credentials(&self.store_path, KEY).await
    }

    async fn begin_oauth(&self) -> Result<OAuthChallenge, ProviderError> {
        // TODO(W5.A4): replace CLIENT_ID_PLACEHOLDER with the Meta
        // App ID stored in publishing.json once the user has
        // registered + been approved for instagram_content_publish.
        Ok(build_authorize(
            AUTHORIZE_URL,
            &[
                ("client_id", CLIENT_ID_PLACEHOLDER),
                ("response_type", "code"),
                ("scope", SCOPES),
            ],
            &fresh_state(),
        ))
    }

    async fn complete_oauth(&self, code: String) -> Result<(), ProviderError> {
        // TODO(W5.A4): GET
        // https://graph.facebook.com/v19.0/oauth/access_token to
        // exchange `code` for a short-lived user token, then exchange
        // *that* for a long-lived (60 day) token via a second hop.
        // IG's token lifecycle is the most complex of the three
        // providers — the real impl needs both the exchange + a
        // refresh task that runs on the 50-day mark.
        stub_complete_oauth(&self.store_path, KEY, code).await
    }

    async fn upload(&self, params: UploadParams) -> Result<UploadResult, ProviderError> {
        stub_upload(&self.store_path, KEY, DEV_CONSOLE_URL, params).await
    }

    async fn status(&self) -> ConnectionStatus {
        load_status(&self.store_path, KEY).await
    }
}
