//! Error type returned by every [`PublishingProvider`](super::PublishingProvider)
//! method.
//!
//! The variants here are the *user-actionable* failure modes — each
//! one maps to a distinct frontend behaviour (open settings, retry,
//! show an upgrade prompt, …). Provider-internal errors (HTTP 500
//! from the platform, JSON parse failure, …) collapse into
//! [`ProviderError::NetworkError`] with a message string so we don't
//! leak transport details up into match arms.

use thiserror::Error;

/// Failure modes shared across every publishing provider.
///
/// `NetworkError` and `RateLimited` are unused in the W5.A1 stub
/// phase — they get constructed once real HTTP code lands in W5.A2+.
/// The variants stay in the enum now so the wire shape (and the
/// frontend's match arms) don't need to change when that happens.
#[allow(dead_code)]
#[derive(Debug, Error)]
pub enum ProviderError {
    /// No credentials on file (or stored credentials failed validation).
    /// The frontend should kick the user into the OAuth flow.
    #[error("provider not configured — connect an account first")]
    NotConfigured,

    /// OAuth handshake failed at the begin / complete step (bad redirect
    /// payload, state mismatch, exchange returned an error, …).
    #[error("oauth failed: {0}")]
    OAuthFailed(String),

    /// HTTP / DNS / TLS error reaching the provider.
    #[error("network error: {0}")]
    NetworkError(String),

    /// Provider replied with a rate-limit response. Caller should
    /// surface the retry-after window if it has one.
    #[error("rate limited by provider — retry later")]
    RateLimited,

    /// Feature the provider doesn't (yet) support — used by the stubs
    /// to flag "real upload requires OAuth credentials".
    #[error("unsupported: {0}")]
    Unsupported(String),

    /// Local I/O failure (reading the file to upload, writing the
    /// credential store, …).
    #[error("io: {0}")]
    Io(String),
}

impl ProviderError {
    /// Tag this error with a stable identifier so the frontend can
    /// match without parsing the message string. Kept as a free
    /// function rather than `Serialize` so callers stay in control of
    /// the wire shape.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::NotConfigured => "not_configured",
            Self::OAuthFailed(_) => "oauth_failed",
            Self::NetworkError(_) => "network_error",
            Self::RateLimited => "rate_limited",
            Self::Unsupported(_) => "unsupported",
            Self::Io(_) => "io",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_strings_are_stable() {
        // These map onto frontend behaviour — renaming them is a
        // breaking change for the UI.
        assert_eq!(ProviderError::NotConfigured.kind(), "not_configured");
        assert_eq!(
            ProviderError::OAuthFailed("bad code".into()).kind(),
            "oauth_failed",
        );
        assert_eq!(
            ProviderError::NetworkError("dns".into()).kind(),
            "network_error",
        );
        assert_eq!(ProviderError::RateLimited.kind(), "rate_limited");
        assert_eq!(
            ProviderError::Unsupported("oauth required".into()).kind(),
            "unsupported",
        );
        assert_eq!(ProviderError::Io("disk".into()).kind(), "io");
    }
}
