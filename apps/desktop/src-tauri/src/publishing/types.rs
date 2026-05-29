//! Shared data shapes for the publishing subsystem.
//!
//! These mirror what each provider's REST API ultimately needs (title,
//! description, visibility, scheduled publish time, etc.) but stay
//! provider-agnostic so the trait surface and Tauri commands don't
//! drift when a provider's specific request body changes. The fields
//! here are the *intersection* of YouTube / TikTok / Instagram upload
//! shapes — provider impls extend privately when they need more.

use serde::{Deserialize, Serialize};

/// Pre-OAuth handshake the frontend opens in a browser.
///
/// `url` is a fully-formed authorisation URL with `client_id`,
/// redirect URI, scope, and `state` already encoded. The `state` is
/// returned separately so the frontend can pin it against the redirect
/// payload (provider returns the same `state` so CSRF-style replays
/// are detectable).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OAuthChallenge {
    /// Provider's `/authorize` URL with all query params populated.
    pub url: String,
    /// CSRF-style nonce; provider echoes it back in the redirect.
    pub state: String,
}

/// Visibility hint applied to the uploaded asset.
///
/// Each provider maps this to its own enum (`public` / `unlisted` /
/// `private` on YouTube, `SELF_ONLY` / `PUBLIC_TO_EVERYONE` on TikTok,
/// `BUSINESS` / `PERSONAL` on Instagram), but the user-facing concept
/// stays small.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Visibility {
    /// Discoverable to anyone.
    Public,
    /// Anyone with the URL can view; not surfaced in search.
    Unlisted,
    /// Only the uploader (and explicitly shared accounts) can view.
    Private,
}

impl Default for Visibility {
    fn default() -> Self {
        // Default to the safest option — a user who forgets to set
        // visibility shouldn't accidentally publish to the world.
        Self::Private
    }
}

/// Per-upload request parameters. Filled by the frontend from the
/// render dialog (title, description, etc.).
///
/// `scheduled_at` is unix epoch seconds. When `None` the upload goes
/// live as soon as the platform finishes encoding.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UploadParams {
    /// Absolute path to the rendered file on disk.
    pub file_path: String,
    /// Human-facing title.
    pub title: String,
    /// Long-form description / caption.
    #[serde(default)]
    pub description: String,
    /// Hashtags / keywords. Provider impls map these to whatever the
    /// platform calls "tags" (`#hashtag` for TikTok / IG, keyword
    /// strings for YouTube).
    #[serde(default)]
    pub tags: Vec<String>,
    /// Visibility on publish.
    #[serde(default)]
    pub visibility: Visibility,
    /// Unix epoch seconds; `None` = publish immediately.
    #[serde(default)]
    pub scheduled_at: Option<i64>,
    /// Optional thumbnail / cover image path on disk.
    #[serde(default)]
    pub thumbnail_path: Option<String>,
}

/// What we hand back to the frontend after a successful upload.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UploadResult {
    /// Public URL the user can share (e.g. `https://youtu.be/abc123`).
    pub remote_url: String,
    /// Provider-specific id (so we can re-link later for analytics).
    pub remote_id: String,
    /// Unix epoch seconds the upload completed.
    pub uploaded_at: i64,
}

/// Lightweight status snapshot for the Settings UI.
///
/// `connected` is the authoritative flag; the optional fields are
/// just human-facing chrome (an account label to show, a "renews in
/// 14 days" expiry).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ConnectionStatus {
    /// `true` once the provider has stored credentials with a non-expired token.
    pub connected: bool,
    /// e.g. `"you@gmail.com"` or `"@handle"`.
    #[serde(default)]
    pub account_name: Option<String>,
    /// Unix epoch seconds; `None` when unknown or non-expiring.
    #[serde(default)]
    pub expires_at: Option<i64>,
}

/// What `list_providers` returns to the frontend — one entry per
/// installed provider.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderInfo {
    /// Stable key (`"youtube"`, `"tiktok"`, `"instagram"`).
    pub key: String,
    /// Human-readable label for the UI.
    pub display_name: String,
    /// Whether credentials are present and valid.
    pub configured: bool,
}
