# Multi-Provider OAuth And Eligibility Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement Phase 2 of the server-backed social OAuth design: provider-specific OAuth, profile normalization, token refresh planning, and eligibility/capability results for YouTube, TikTok, and Instagram.

**Architecture:** Extend `montage-social` with provider-specific server contracts and mocked adapters. This phase does not add a web server, database migrations, or live provider HTTP calls; it creates the request/response and validation surfaces that a future server route can call. OAuth tokens still remain server-only values, represented by sanitized token bundle structs and the token envelope boundary from Phase 1.

**Tech Stack:** Rust 2024 workspace crate, `serde`, `serde_json`, `base64`, `sha2`, `thiserror`, deterministic unit tests, no live network calls in CI.

---

## Source Notes

- YouTube Data API uses OAuth 2.0 for user authorization, does not support service accounts for YouTube account access, and uses scopes such as `https://www.googleapis.com/auth/youtube.upload`.
- YouTube `videos.insert` accepts `youtube.upload` and related scopes and notes that unverified API projects created after July 28, 2020 may have uploaded videos restricted to private.
- TikTok Login Kit manages user access tokens with OAuth v2; user scopes require direct consent, and TikTok recommends storing/managing tokens server-side.
- TikTok Content Posting API creator info uses `POST /v2/post/publish/creator_info/query/` with `video.publish` scope and must be called when rendering the export page to show current creator options.
- Instagram content publishing requires the Instagram Platform/Graph publishing flow for professional accounts; Meta documentation may require developer login, so implementation must model app-review and professional-account eligibility explicitly.

## Scope

Implement Phase 2 from `docs/superpowers/specs/2026-06-02-server-backed-social-oauth-design.md`:

- YouTube OAuth, channel profile normalization, refresh bundle handling, and capability fetch shape.
- TikTok OAuth, profile normalization, creator info eligibility, and capability fetch shape.
- Instagram OAuth, professional-account resolution shape, eligibility, and capability fetch shape.
- Shared provider adapter trait methods for OAuth/profile/capabilities.

Do not implement live network calls, upload adapters, scheduled queue workers, HTTP routes, database storage, or desktop UI in this plan.

## File Structure

- Modify `crates/social/src/lib.rs`: expose new modules.
- Create `crates/social/src/oauth_url.rs`: provider OAuth URL builder and server-side state wiring.
- Create `crates/social/src/token_bundle.rs`: provider token exchange/refresh response normalization without exposing raw tokens in public account models.
- Create `crates/social/src/eligibility.rs`: normalized provider profile, account resolution, and eligibility helpers.
- Modify `crates/social/src/provider.rs`: add provider OAuth/capability adapter contracts and mocked default adapter implementations.
- Modify `crates/social/src/model.rs`: add account profile fields only if needed for normalized account creation.
- Modify `crates/social/Cargo.toml`: add dependencies only if used by the new modules.

## Task 1: Provider OAuth URL Builder

**Files:**
- Create: `crates/social/src/oauth_url.rs`
- Modify: `crates/social/src/lib.rs`

- [ ] **Step 1: Write failing OAuth URL tests**

Create `crates/social/src/oauth_url.rs`:

```rust
use crate::model::{OwnerRef, Provider};
use crate::oauth::OAuthConnection;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OAuthProviderConfig {
    pub client_id: String,
    pub redirect_uri: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OAuthAuthorizeRequest {
    pub connection: OAuthConnection,
    pub authorization_url: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> OAuthProviderConfig {
        OAuthProviderConfig {
            client_id: "client_123".into(),
            redirect_uri: "https://app.montage.test/social/oauth/callback".into(),
        }
    }

    #[test]
    fn youtube_authorize_url_uses_google_endpoint_and_upload_scope() {
        let request = begin_provider_oauth(
            "oauth_1",
            OwnerRef::User("user_1".into()),
            Provider::YouTube,
            &config(),
            "state-secret",
            "/campaigns/campaign_1".into(),
            100,
            200,
        );

        assert!(request.authorization_url.starts_with("https://accounts.google.com/o/oauth2/v2/auth?"));
        assert!(request.authorization_url.contains("client_id=client_123"));
        assert!(request.authorization_url.contains("scope=https%3A%2F%2Fwww.googleapis.com%2Fauth%2Fyoutube.upload"));
        assert!(request.authorization_url.contains("access_type=offline"));
        assert!(request.authorization_url.contains("prompt=consent"));
        assert_ne!(request.connection.state_hash, "state-secret");
    }

    #[test]
    fn tiktok_authorize_url_uses_tiktok_endpoint_and_publish_scopes() {
        let request = begin_provider_oauth(
            "oauth_1",
            OwnerRef::User("user_1".into()),
            Provider::TikTok,
            &config(),
            "state-secret",
            "/campaigns/campaign_1".into(),
            100,
            200,
        );

        assert!(request.authorization_url.starts_with("https://www.tiktok.com/v2/auth/authorize/?"));
        assert!(request.authorization_url.contains("scope=user.info.basic%2Cvideo.publish"));
        assert!(request.authorization_url.contains("response_type=code"));
    }

    #[test]
    fn instagram_authorize_url_uses_meta_endpoint_and_publish_scope() {
        let request = begin_provider_oauth(
            "oauth_1",
            OwnerRef::User("user_1".into()),
            Provider::Instagram,
            &config(),
            "state-secret",
            "/campaigns/campaign_1".into(),
            100,
            200,
        );

        assert!(request.authorization_url.starts_with("https://www.facebook.com/v24.0/dialog/oauth?"));
        assert!(request.authorization_url.contains("scope=instagram_basic%2Cinstagram_content_publish"));
        assert!(request.authorization_url.contains("redirect_uri=https%3A%2F%2Fapp.montage.test%2Fsocial%2Foauth%2Fcallback"));
    }
}
```

- [ ] **Step 2: Run the failing tests**

Run:

```bash
cargo test -p montage-social oauth_url::tests
```

Expected: FAIL with unresolved `begin_provider_oauth`.

- [ ] **Step 3: Implement URL building**

Replace `crates/social/src/oauth_url.rs` with:

```rust
use crate::model::{OwnerRef, Provider};
use crate::oauth::OAuthConnection;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OAuthProviderConfig {
    pub client_id: String,
    pub redirect_uri: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OAuthAuthorizeRequest {
    pub connection: OAuthConnection,
    pub authorization_url: String,
}

#[allow(clippy::too_many_arguments)]
pub fn begin_provider_oauth(
    id: impl Into<String>,
    owner: OwnerRef,
    provider: Provider,
    config: &OAuthProviderConfig,
    raw_state: &str,
    return_to: String,
    created_at: i64,
    expires_at: i64,
) -> OAuthAuthorizeRequest {
    let scopes = scopes_for(&provider);
    let connection = OAuthConnection::start(
        id,
        owner,
        provider.clone(),
        raw_state,
        scopes.iter().map(|scope| (*scope).to_string()).collect(),
        return_to,
        created_at,
        expires_at,
    );
    let authorization_url = authorization_url(&provider, config, raw_state, &scopes);
    OAuthAuthorizeRequest {
        connection,
        authorization_url,
    }
}

fn scopes_for(provider: &Provider) -> Vec<&'static str> {
    match provider {
        Provider::YouTube => vec!["https://www.googleapis.com/auth/youtube.upload"],
        Provider::TikTok => vec!["user.info.basic", "video.publish"],
        Provider::Instagram => vec!["instagram_basic", "instagram_content_publish"],
    }
}

fn authorization_url(
    provider: &Provider,
    config: &OAuthProviderConfig,
    raw_state: &str,
    scopes: &[&str],
) -> String {
    let scope = match provider {
        Provider::YouTube => scopes.join(" "),
        Provider::TikTok | Provider::Instagram => scopes.join(","),
    };
    let base = match provider {
        Provider::YouTube => "https://accounts.google.com/o/oauth2/v2/auth",
        Provider::TikTok => "https://www.tiktok.com/v2/auth/authorize/",
        Provider::Instagram => "https://www.facebook.com/v24.0/dialog/oauth",
    };
    let mut params = vec![
        ("client_id", config.client_id.as_str()),
        ("redirect_uri", config.redirect_uri.as_str()),
        ("response_type", "code"),
        ("scope", scope.as_str()),
        ("state", raw_state),
    ];
    if provider == &Provider::YouTube {
        params.push(("access_type", "offline"));
        params.push(("prompt", "consent"));
    }
    format!("{base}?{}", encode_query(&params))
}

fn encode_query(params: &[(&str, &str)]) -> String {
    params
        .iter()
        .map(|(key, value)| format!("{key}={}", percent_encode(value)))
        .collect::<Vec<_>>()
        .join("&")
}

fn percent_encode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(char::from(byte));
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> OAuthProviderConfig {
        OAuthProviderConfig {
            client_id: "client_123".into(),
            redirect_uri: "https://app.montage.test/social/oauth/callback".into(),
        }
    }

    #[test]
    fn youtube_authorize_url_uses_google_endpoint_and_upload_scope() {
        let request = begin_provider_oauth(
            "oauth_1",
            OwnerRef::User("user_1".into()),
            Provider::YouTube,
            &config(),
            "state-secret",
            "/campaigns/campaign_1".into(),
            100,
            200,
        );

        assert!(request.authorization_url.starts_with("https://accounts.google.com/o/oauth2/v2/auth?"));
        assert!(request.authorization_url.contains("client_id=client_123"));
        assert!(request.authorization_url.contains("scope=https%3A%2F%2Fwww.googleapis.com%2Fauth%2Fyoutube.upload"));
        assert!(request.authorization_url.contains("access_type=offline"));
        assert!(request.authorization_url.contains("prompt=consent"));
        assert_ne!(request.connection.state_hash, "state-secret");
    }

    #[test]
    fn tiktok_authorize_url_uses_tiktok_endpoint_and_publish_scopes() {
        let request = begin_provider_oauth(
            "oauth_1",
            OwnerRef::User("user_1".into()),
            Provider::TikTok,
            &config(),
            "state-secret",
            "/campaigns/campaign_1".into(),
            100,
            200,
        );

        assert!(request.authorization_url.starts_with("https://www.tiktok.com/v2/auth/authorize/?"));
        assert!(request.authorization_url.contains("scope=user.info.basic%2Cvideo.publish"));
        assert!(request.authorization_url.contains("response_type=code"));
    }

    #[test]
    fn instagram_authorize_url_uses_meta_endpoint_and_publish_scope() {
        let request = begin_provider_oauth(
            "oauth_1",
            OwnerRef::User("user_1".into()),
            Provider::Instagram,
            &config(),
            "state-secret",
            "/campaigns/campaign_1".into(),
            100,
            200,
        );

        assert!(request.authorization_url.starts_with("https://www.facebook.com/v24.0/dialog/oauth?"));
        assert!(request.authorization_url.contains("scope=instagram_basic%2Cinstagram_content_publish"));
        assert!(request.authorization_url.contains("redirect_uri=https%3A%2F%2Fapp.montage.test%2Fsocial%2Foauth%2Fcallback"));
    }
}
```

Modify `crates/social/src/lib.rs`:

```rust
pub mod job;
pub mod model;
pub mod oauth;
pub mod oauth_url;
pub mod provider;
pub mod token;
```

- [ ] **Step 4: Run tests**

Run:

```bash
cargo test -p montage-social oauth_url::tests
```

Expected: PASS, three OAuth URL tests pass.

- [ ] **Step 5: Commit**

Run:

```bash
git add crates/social/src/lib.rs crates/social/src/oauth_url.rs
git commit -m "feat(social): add provider oauth urls"
```

## Task 2: Provider Token Bundle Normalization

**Files:**
- Create: `crates/social/src/token_bundle.rs`
- Modify: `crates/social/src/lib.rs`

- [ ] **Step 1: Write failing token bundle tests**

Create `crates/social/src/token_bundle.rs`:

```rust
use crate::model::Provider;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderTokenBundle {
    pub provider: Provider,
    pub provider_account_id: String,
    pub scopes: Vec<String>,
    pub access_token_expires_at: i64,
    pub refresh_token_expires_at: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn youtube_token_response_normalizes_channel_identity_and_expiry() {
        let response = OAuthTokenResponse {
            provider_account_id: "channel_1".into(),
            scopes: "https://www.googleapis.com/auth/youtube.upload".into(),
            expires_in: 3_600,
            refresh_expires_in: None,
        };

        let bundle = ProviderTokenBundle::from_oauth_response(Provider::YouTube, response, 100);
        assert_eq!(bundle.provider_account_id, "channel_1");
        assert_eq!(bundle.scopes, vec!["https://www.googleapis.com/auth/youtube.upload"]);
        assert_eq!(bundle.access_token_expires_at, 3_700);
        assert_eq!(bundle.refresh_token_expires_at, None);
    }

    #[test]
    fn tiktok_token_response_splits_comma_scopes_and_refresh_expiry() {
        let response = OAuthTokenResponse {
            provider_account_id: "open_id_1".into(),
            scopes: "user.info.basic,video.publish".into(),
            expires_in: 86_400,
            refresh_expires_in: Some(31_536_000),
        };

        let bundle = ProviderTokenBundle::from_oauth_response(Provider::TikTok, response, 100);
        assert_eq!(bundle.provider_account_id, "open_id_1");
        assert_eq!(bundle.scopes, vec!["user.info.basic", "video.publish"]);
        assert_eq!(bundle.access_token_expires_at, 86_500);
        assert_eq!(bundle.refresh_token_expires_at, Some(31_536_100));
    }
}
```

- [ ] **Step 2: Run the failing tests**

Run:

```bash
cargo test -p montage-social token_bundle::tests
```

Expected: FAIL with unresolved `OAuthTokenResponse` and `from_oauth_response`.

- [ ] **Step 3: Implement token bundle normalization**

Replace `crates/social/src/token_bundle.rs` with:

```rust
use crate::model::Provider;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OAuthTokenResponse {
    pub provider_account_id: String,
    pub scopes: String,
    pub expires_in: i64,
    pub refresh_expires_in: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderTokenBundle {
    pub provider: Provider,
    pub provider_account_id: String,
    pub scopes: Vec<String>,
    pub access_token_expires_at: i64,
    pub refresh_token_expires_at: Option<i64>,
}

impl ProviderTokenBundle {
    pub fn from_oauth_response(
        provider: Provider,
        response: OAuthTokenResponse,
        now: i64,
    ) -> Self {
        let scopes = split_scopes(&provider, &response.scopes);
        Self {
            provider,
            provider_account_id: response.provider_account_id,
            scopes,
            access_token_expires_at: now + response.expires_in,
            refresh_token_expires_at: response.refresh_expires_in.map(|ttl| now + ttl),
        }
    }
}

fn split_scopes(provider: &Provider, raw: &str) -> Vec<String> {
    let delimiter = match provider {
        Provider::YouTube => ' ',
        Provider::TikTok | Provider::Instagram => ',',
    };
    raw.split(delimiter)
        .map(str::trim)
        .filter(|scope| !scope.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn youtube_token_response_normalizes_channel_identity_and_expiry() {
        let response = OAuthTokenResponse {
            provider_account_id: "channel_1".into(),
            scopes: "https://www.googleapis.com/auth/youtube.upload".into(),
            expires_in: 3_600,
            refresh_expires_in: None,
        };

        let bundle = ProviderTokenBundle::from_oauth_response(Provider::YouTube, response, 100);
        assert_eq!(bundle.provider_account_id, "channel_1");
        assert_eq!(bundle.scopes, vec!["https://www.googleapis.com/auth/youtube.upload"]);
        assert_eq!(bundle.access_token_expires_at, 3_700);
        assert_eq!(bundle.refresh_token_expires_at, None);
    }

    #[test]
    fn tiktok_token_response_splits_comma_scopes_and_refresh_expiry() {
        let response = OAuthTokenResponse {
            provider_account_id: "open_id_1".into(),
            scopes: "user.info.basic,video.publish".into(),
            expires_in: 86_400,
            refresh_expires_in: Some(31_536_000),
        };

        let bundle = ProviderTokenBundle::from_oauth_response(Provider::TikTok, response, 100);
        assert_eq!(bundle.provider_account_id, "open_id_1");
        assert_eq!(bundle.scopes, vec!["user.info.basic", "video.publish"]);
        assert_eq!(bundle.access_token_expires_at, 86_500);
        assert_eq!(bundle.refresh_token_expires_at, Some(31_536_100));
    }
}
```

Modify `crates/social/src/lib.rs`:

```rust
pub mod job;
pub mod model;
pub mod oauth;
pub mod oauth_url;
pub mod provider;
pub mod token;
pub mod token_bundle;
```

- [ ] **Step 4: Run tests**

Run:

```bash
cargo test -p montage-social token_bundle::tests
```

Expected: PASS, two token bundle tests pass.

- [ ] **Step 5: Commit**

Run:

```bash
git add crates/social/src/lib.rs crates/social/src/token_bundle.rs
git commit -m "feat(social): normalize provider token bundles"
```

## Task 3: Profile And Eligibility Normalization

**Files:**
- Create: `crates/social/src/eligibility.rs`
- Modify: `crates/social/src/lib.rs`

- [ ] **Step 1: Write failing eligibility tests**

Create `crates/social/src/eligibility.rs`:

```rust
use crate::model::{AccountEligibility, AccountKind, Provider, ProviderCapabilities};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderAccountProfile {
    pub provider: Provider,
    pub provider_account_id: String,
    pub display_name: String,
    pub handle: Option<String>,
    pub avatar_url: Option<String>,
    pub account_kind: AccountKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderEligibilityReport {
    pub profile: ProviderAccountProfile,
    pub capabilities: ProviderCapabilities,
    pub eligibility: AccountEligibility,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn youtube_channel_profile_is_upload_eligible() {
        let report = youtube_eligibility("channel_1", "Montage", Some("@montage"));
        assert!(report.eligibility.eligible);
        assert!(report.capabilities.upload_video);
        assert!(report.capabilities.native_scheduling);
        assert_eq!(report.profile.account_kind, AccountKind::Channel);
    }

    #[test]
    fn tiktok_missing_direct_post_scope_is_requires_action() {
        let report = tiktok_eligibility("open_id_1", "Creator", &["user.info.basic"]);
        assert!(!report.eligibility.eligible);
        assert_eq!(report.eligibility.reasons, vec!["missing_video_publish_scope"]);
        assert!(report.capabilities.requires_user_consent);
    }

    #[test]
    fn instagram_non_professional_account_is_ineligible() {
        let report = instagram_eligibility("ig_1", "Creator", false, true);
        assert!(!report.eligibility.eligible);
        assert_eq!(report.eligibility.reasons, vec!["instagram_professional_account_required"]);
    }

    #[test]
    fn instagram_missing_publish_scope_is_ineligible() {
        let report = instagram_eligibility("ig_1", "Creator", true, false);
        assert!(!report.eligibility.eligible);
        assert_eq!(report.eligibility.reasons, vec!["missing_instagram_content_publish_scope"]);
    }
}
```

- [ ] **Step 2: Run the failing tests**

Run:

```bash
cargo test -p montage-social eligibility::tests
```

Expected: FAIL with unresolved eligibility helper functions.

- [ ] **Step 3: Implement eligibility helpers**

Replace `crates/social/src/eligibility.rs` with:

```rust
use crate::model::{AccountEligibility, AccountKind, Provider, ProviderCapabilities};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderAccountProfile {
    pub provider: Provider,
    pub provider_account_id: String,
    pub display_name: String,
    pub handle: Option<String>,
    pub avatar_url: Option<String>,
    pub account_kind: AccountKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderEligibilityReport {
    pub profile: ProviderAccountProfile,
    pub capabilities: ProviderCapabilities,
    pub eligibility: AccountEligibility,
}

pub fn youtube_eligibility(
    provider_account_id: impl Into<String>,
    display_name: impl Into<String>,
    handle: Option<&str>,
) -> ProviderEligibilityReport {
    ProviderEligibilityReport {
        profile: ProviderAccountProfile {
            provider: Provider::YouTube,
            provider_account_id: provider_account_id.into(),
            display_name: display_name.into(),
            handle: handle.map(ToOwned::to_owned),
            avatar_url: None,
            account_kind: AccountKind::Channel,
        },
        capabilities: ProviderCapabilities {
            native_scheduling: true,
            queue_scheduling: true,
            upload_video: true,
            upload_thumbnail: true,
            public_posting: true,
            requires_user_consent: false,
        },
        eligibility: AccountEligibility::eligible(),
    }
}

pub fn tiktok_eligibility(
    provider_account_id: impl Into<String>,
    display_name: impl Into<String>,
    scopes: &[&str],
) -> ProviderEligibilityReport {
    let has_publish = scopes.iter().any(|scope| *scope == "video.publish");
    ProviderEligibilityReport {
        profile: ProviderAccountProfile {
            provider: Provider::TikTok,
            provider_account_id: provider_account_id.into(),
            display_name: display_name.into(),
            handle: None,
            avatar_url: None,
            account_kind: AccountKind::Creator,
        },
        capabilities: ProviderCapabilities {
            native_scheduling: false,
            queue_scheduling: true,
            upload_video: has_publish,
            upload_thumbnail: false,
            public_posting: has_publish,
            requires_user_consent: true,
        },
        eligibility: if has_publish {
            AccountEligibility::eligible()
        } else {
            AccountEligibility::blocked("missing_video_publish_scope")
        },
    }
}

pub fn instagram_eligibility(
    provider_account_id: impl Into<String>,
    display_name: impl Into<String>,
    is_professional: bool,
    has_content_publish_scope: bool,
) -> ProviderEligibilityReport {
    let eligibility = match (is_professional, has_content_publish_scope) {
        (false, _) => AccountEligibility::blocked("instagram_professional_account_required"),
        (true, false) => AccountEligibility::blocked("missing_instagram_content_publish_scope"),
        (true, true) => AccountEligibility::eligible(),
    };
    ProviderEligibilityReport {
        profile: ProviderAccountProfile {
            provider: Provider::Instagram,
            provider_account_id: provider_account_id.into(),
            display_name: display_name.into(),
            handle: None,
            avatar_url: None,
            account_kind: if is_professional {
                AccountKind::Professional
            } else {
                AccountKind::Unknown
            },
        },
        capabilities: ProviderCapabilities {
            native_scheduling: false,
            queue_scheduling: true,
            upload_video: is_professional && has_content_publish_scope,
            upload_thumbnail: false,
            public_posting: is_professional && has_content_publish_scope,
            requires_user_consent: false,
        },
        eligibility,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn youtube_channel_profile_is_upload_eligible() {
        let report = youtube_eligibility("channel_1", "Montage", Some("@montage"));
        assert!(report.eligibility.eligible);
        assert!(report.capabilities.upload_video);
        assert!(report.capabilities.native_scheduling);
        assert_eq!(report.profile.account_kind, AccountKind::Channel);
    }

    #[test]
    fn tiktok_missing_direct_post_scope_is_requires_action() {
        let report = tiktok_eligibility("open_id_1", "Creator", &["user.info.basic"]);
        assert!(!report.eligibility.eligible);
        assert_eq!(report.eligibility.reasons, vec!["missing_video_publish_scope"]);
        assert!(report.capabilities.requires_user_consent);
    }

    #[test]
    fn instagram_non_professional_account_is_ineligible() {
        let report = instagram_eligibility("ig_1", "Creator", false, true);
        assert!(!report.eligibility.eligible);
        assert_eq!(report.eligibility.reasons, vec!["instagram_professional_account_required"]);
    }

    #[test]
    fn instagram_missing_publish_scope_is_ineligible() {
        let report = instagram_eligibility("ig_1", "Creator", true, false);
        assert!(!report.eligibility.eligible);
        assert_eq!(report.eligibility.reasons, vec!["missing_instagram_content_publish_scope"]);
    }
}
```

Modify `crates/social/src/lib.rs`:

```rust
pub mod eligibility;
pub mod job;
pub mod model;
pub mod oauth;
pub mod oauth_url;
pub mod provider;
pub mod token;
pub mod token_bundle;
```

- [ ] **Step 4: Run tests**

Run:

```bash
cargo test -p montage-social eligibility::tests
```

Expected: PASS, four eligibility tests pass.

- [ ] **Step 5: Commit**

Run:

```bash
git add crates/social/src/lib.rs crates/social/src/eligibility.rs
git commit -m "feat(social): normalize provider eligibility"
```

## Task 4: Provider Adapter Contract

**Files:**
- Modify: `crates/social/src/provider.rs`

- [ ] **Step 1: Write failing provider adapter tests**

Append to `crates/social/src/provider.rs` tests:

```rust
#[test]
fn youtube_adapter_reports_profile_and_capabilities() {
    let adapter = MockProviderAdapter::youtube("channel_1", "Montage");
    let report = adapter.fetch_capabilities(&["https://www.googleapis.com/auth/youtube.upload"]);
    assert!(report.eligibility.eligible);
    assert_eq!(report.profile.provider_account_id, "channel_1");
    assert!(report.capabilities.upload_video);
}

#[test]
fn tiktok_adapter_reports_missing_publish_scope() {
    let adapter = MockProviderAdapter::tiktok("open_id_1", "Creator");
    let report = adapter.fetch_capabilities(&["user.info.basic"]);
    assert_eq!(report.eligibility.reasons, vec!["missing_video_publish_scope"]);
}

#[test]
fn instagram_adapter_reports_professional_requirement() {
    let adapter = MockProviderAdapter::instagram("ig_1", "Creator", false);
    let report = adapter.fetch_capabilities(&["instagram_basic", "instagram_content_publish"]);
    assert_eq!(
        report.eligibility.reasons,
        vec!["instagram_professional_account_required"]
    );
}
```

- [ ] **Step 2: Run the failing tests**

Run:

```bash
cargo test -p montage-social provider::tests
```

Expected: FAIL with unresolved `MockProviderAdapter`.

- [ ] **Step 3: Implement mock adapter contract**

Add near the top of `crates/social/src/provider.rs`:

```rust
use crate::eligibility::{
    ProviderEligibilityReport, instagram_eligibility, tiktok_eligibility, youtube_eligibility,
};
```

Add above the tests:

```rust
pub trait SocialProviderAdapter {
    fn provider(&self) -> Provider;
    fn fetch_capabilities(&self, scopes: &[&str]) -> ProviderEligibilityReport;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MockProviderAdapter {
    provider: Provider,
    provider_account_id: String,
    display_name: String,
    instagram_professional: bool,
}

impl MockProviderAdapter {
    pub fn youtube(
        provider_account_id: impl Into<String>,
        display_name: impl Into<String>,
    ) -> Self {
        Self {
            provider: Provider::YouTube,
            provider_account_id: provider_account_id.into(),
            display_name: display_name.into(),
            instagram_professional: false,
        }
    }

    pub fn tiktok(
        provider_account_id: impl Into<String>,
        display_name: impl Into<String>,
    ) -> Self {
        Self {
            provider: Provider::TikTok,
            provider_account_id: provider_account_id.into(),
            display_name: display_name.into(),
            instagram_professional: false,
        }
    }

    pub fn instagram(
        provider_account_id: impl Into<String>,
        display_name: impl Into<String>,
        instagram_professional: bool,
    ) -> Self {
        Self {
            provider: Provider::Instagram,
            provider_account_id: provider_account_id.into(),
            display_name: display_name.into(),
            instagram_professional,
        }
    }
}

impl SocialProviderAdapter for MockProviderAdapter {
    fn provider(&self) -> Provider {
        self.provider.clone()
    }

    fn fetch_capabilities(&self, scopes: &[&str]) -> ProviderEligibilityReport {
        match self.provider {
            Provider::YouTube => {
                youtube_eligibility(&self.provider_account_id, &self.display_name, None)
            }
            Provider::TikTok => tiktok_eligibility(
                &self.provider_account_id,
                &self.display_name,
                scopes,
            ),
            Provider::Instagram => instagram_eligibility(
                &self.provider_account_id,
                &self.display_name,
                self.instagram_professional,
                scopes.iter().any(|scope| *scope == "instagram_content_publish"),
            ),
        }
    }
}
```

- [ ] **Step 4: Run provider tests**

Run:

```bash
cargo test -p montage-social provider::tests
```

Expected: PASS, existing and new provider tests pass.

- [ ] **Step 5: Commit**

Run:

```bash
git add crates/social/src/provider.rs
git commit -m "feat(social): add provider eligibility adapter"
```

## Task 5: Phase 2 Verification

**Files:**
- Test only

- [ ] **Step 1: Run full crate tests**

Run:

```bash
cargo test -p montage-social
```

Expected: PASS, all `montage-social` tests pass.

- [ ] **Step 2: Run clippy**

Run:

```bash
cargo clippy -p montage-social --all-targets -- -D warnings
```

Expected: PASS with no warnings.

- [ ] **Step 3: Run formatting check**

Run:

```bash
cargo fmt --all -- --check
```

Expected: exit 0. Existing stable-rustfmt warnings about `imports_granularity` are acceptable if there are no formatting diffs.

- [ ] **Step 4: Check diff hygiene**

Run:

```bash
git diff --check
git status --short --branch
```

Expected: `git diff --check` prints nothing. `git status` shows a clean branch after commits.

## Self-Review

Spec coverage:

- YouTube OAuth URL, upload scope, profile/capability normalization: Tasks 1 and 3.
- TikTok OAuth URL, `user.info.basic` and `video.publish`, token refresh shape, creator eligibility: Tasks 1, 2, and 3.
- Instagram OAuth URL, publish scope, professional-account and scope eligibility: Tasks 1 and 3.
- Shared provider adapter capability contract: Task 4.
- Verification: Task 5.

Intentional gaps for the next phase:

- No live provider HTTP calls; the adapter contract is mocked and test-only.
- No server route handlers; the repository still has no chosen web server crate.
- No database-backed connected-account persistence.
- No upload/schedule adapter calls; those belong to later publish-job phases.
- No desktop/web UI account selector; UI should consume this crate once the server/API layer exists.
