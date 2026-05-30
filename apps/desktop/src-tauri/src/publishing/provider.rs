//! The [`PublishingProvider`] trait — one impl per platform.
//!
//! The trait stays minimal on purpose: the *protocol* of a publishing
//! provider (advertise → OAuth → upload → status) is the same across
//! YouTube / TikTok / Instagram even though the request bodies aren't.
//! Provider-specific quirks (resumable-upload sessions, chunked
//! uploads, etc.) stay private inside each impl.
//!
//! All methods return [`Result<_, ProviderError>`](super::ProviderError).
//! The Tauri command layer stringifies the error at the IPC boundary.

use async_trait::async_trait;

use super::errors::ProviderError;
use super::oauth::ClientCredentialsState;
use super::types::{ConnectionStatus, OAuthChallenge, UploadParams, UploadResult};

/// One publishing platform.
#[async_trait]
pub trait PublishingProvider: Send + Sync {
    /// Stable identifier — `"youtube"`, `"tiktok"`, `"instagram"`.
    ///
    /// Used as the storage key in [`publishing.json`](super::storage)
    /// and as the discriminator in every Tauri command, so this is a
    /// permanent contract.
    fn key(&self) -> &'static str;

    /// Human-readable label (`"YouTube"`).
    fn display_name(&self) -> &'static str;

    /// `true` once credentials are present and currently valid.
    ///
    /// Lightweight check — must not hit the network. The frontend
    /// polls this at modal open time, so an HTTP round trip per
    /// provider would stall the UI.
    async fn is_configured(&self) -> bool;

    /// Build the platform's `/authorize` URL with `client_id`,
    /// redirect URI, scope, and a fresh `state` nonce. The frontend
    /// opens the URL in a browser; provider closes the loop in
    /// [`PublishingProvider::complete_oauth`].
    async fn begin_oauth(&self) -> Result<OAuthChallenge, ProviderError>;

    /// Exchange the redirect's `code` for an access (and refresh)
    /// token and persist to storage.
    async fn complete_oauth(&self, code: String) -> Result<(), ProviderError>;

    /// Push a finished render to the platform.
    ///
    /// Returns [`ProviderError::NotConfigured`] when no credentials
    /// are on file; that's the contract the frontend matches on to
    /// pop the "Connect account" sheet.
    async fn upload(&self, params: UploadParams) -> Result<UploadResult, ProviderError>;

    /// Account label / token expiry for the Settings panel.
    ///
    /// Defaults to a "not connected" snapshot when storage is missing
    /// or unreadable so callers never need to handle errors here.
    async fn status(&self) -> ConnectionStatus;

    /// W5.A5 — clear the OAuth-issued tokens for this provider while
    /// preserving the user's BYO `client_credentials`. Idempotent.
    async fn disconnect(&self) -> Result<(), ProviderError>;

    /// W5.A5 — persist BYO OAuth-app credentials for this provider.
    /// Used by Settings → Publishing → "Bring your own credentials" so
    /// the user can plug in their own dev-console-registered app and
    /// upgrade the placeholder `client_id` substitution to a real one.
    async fn set_client_credentials(
        &self,
        client_id: String,
        client_secret: String,
    ) -> Result<(), ProviderError>;

    /// W5.A5 — read the BYO credentials *presence* (booleans only — the
    /// actual `client_secret` never leaves the backend). Used to render
    /// the "✓ Configured" pip next to each provider's BYO row.
    async fn client_credentials_state(&self) -> ClientCredentialsState;
}
