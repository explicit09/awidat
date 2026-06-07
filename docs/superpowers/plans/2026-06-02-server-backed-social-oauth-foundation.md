# Server-Backed Social OAuth Foundation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the first server-backed social OAuth foundation for YouTube, TikTok, and Instagram as a reusable Rust domain crate.

**Architecture:** Add a focused `montage-social` crate that owns provider/account/job domain types, OAuth session validation, token-secret redaction/encryption boundaries, provider capability contracts, and publish job state transitions. This plan deliberately avoids choosing the final web framework or database; it creates the testable service boundary that a future HTTP server can mount.

**Tech Stack:** Rust 2024 workspace crate, `serde`, `chrono`, `sha2`, `base64`, `thiserror`, unit tests, workspace clippy lints.

---

## Scope

This plan implements Phase 1 of `docs/superpowers/specs/2026-06-02-server-backed-social-oauth-design.md`: the server account foundation. It does not call live Google, TikTok, or Meta APIs. It creates provider-agnostic primitives and mocked adapter tests so live OAuth/provider integrations can land in the next plan.

## File Structure

- Create `crates/social/Cargo.toml`: crate manifest.
- Create `crates/social/src/lib.rs`: public module surface.
- Create `crates/social/src/model.rs`: provider/account/target/job/event data model.
- Create `crates/social/src/oauth.rs`: OAuth connection session creation and callback validation.
- Create `crates/social/src/token.rs`: encrypted token-secret envelope and test key provider boundary.
- Create `crates/social/src/provider.rs`: provider adapter trait, registry, capabilities, eligibility, normalized errors.
- Create `crates/social/src/job.rs`: publish job construction, idempotency keys, state transition helpers.
- Modify `Cargo.toml`: add `crates/social` to workspace members and `montage-social` to workspace dependencies.

## Task 1: Add Crate Skeleton And Core Models

**Files:**
- Create: `crates/social/Cargo.toml`
- Create: `crates/social/src/lib.rs`
- Create: `crates/social/src/model.rs`
- Modify: `Cargo.toml`

- [ ] **Step 1: Write the failing model tests**

Create `crates/social/src/model.rs` with these tests first:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_keys_are_stable() {
        assert_eq!(Provider::YouTube.as_str(), "youtube");
        assert_eq!(Provider::TikTok.as_str(), "tiktok");
        assert_eq!(Provider::Instagram.as_str(), "instagram");
    }

    #[test]
    fn connected_account_never_contains_token_material() {
        let account = ConnectedAccount {
            id: "acct_1".into(),
            owner: OwnerRef::User("user_1".into()),
            provider: Provider::YouTube,
            provider_account_id: "channel_1".into(),
            display_name: "Montage Channel".into(),
            handle: Some("@montage".into()),
            avatar_url: None,
            account_kind: AccountKind::Channel,
            status: ConnectedAccountStatus::Connected,
            scopes: vec!["youtube.upload".into()],
            capabilities: ProviderCapabilities::default(),
            eligibility: AccountEligibility::eligible(),
            last_verified_at: None,
            created_at: 1,
            updated_at: 1,
        };

        let json = serde_json::to_string(&account)
            .unwrap_or_else(|err| panic!("serialize account: {err}"));
        assert!(json.contains("Montage Channel"));
        assert!(!json.contains("access_token"));
        assert!(!json.contains("refresh_token"));
    }
}
```

- [ ] **Step 2: Run tests to verify the crate does not exist yet**

Run:

```bash
cargo test -p montage-social model::tests::provider_keys_are_stable
```

Expected: FAIL with `package ID specification 'montage-social' did not match any packages`.

- [ ] **Step 3: Add the crate manifest and workspace entries**

Create `crates/social/Cargo.toml`:

```toml
[package]
name = "montage-social"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
authors.workspace = true
license.workspace = true
repository.workspace = true

[dependencies]
base64 = { workspace = true }
chrono = { workspace = true }
serde = { workspace = true, features = ["derive"] }
serde_json = { workspace = true }
sha2 = { workspace = true }
thiserror = { workspace = true }

[lints]
workspace = true
```

Modify top-level `Cargo.toml`:

```toml
members = [
    "crates/proto",
    "crates/core",
    "crates/tools",
    "crates/mcp",
    "crates/sandboxing",
    "crates/cli",
    "crates/render",
    "crates/render-gpu",
    "crates/effects",
    "crates/lut",
    "crates/test-support",
    "crates/config",
    "crates/secrets",
    "crates/auth",
    "crates/social",
    "crates/index",
    "crates/desktop-protocol",
    "crates/codex-bridge",
    "apps/desktop/src-tauri",
]
```

Add the dependency entry near other owned crates:

```toml
montage-social = { path = "crates/social" }
```

- [ ] **Step 4: Implement the public module surface and models**

Create `crates/social/src/lib.rs`:

```rust
//! Server-backed social publishing account foundation.
//!
//! This crate contains provider-agnostic account, OAuth, token, and publish-job
//! contracts. It does not perform live platform HTTP calls.

pub mod job;
pub mod model;
pub mod oauth;
pub mod provider;
pub mod token;
```

Replace `crates/social/src/model.rs` with:

```rust
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    YouTube,
    TikTok,
    Instagram,
}

impl Provider {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::YouTube => "youtube",
            Self::TikTok => "tiktok",
            Self::Instagram => "instagram",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OwnerRef {
    User(String),
    Workspace(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountKind {
    Channel,
    Creator,
    Business,
    Professional,
    Page,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectedAccountStatus {
    Connected,
    NeedsReauth,
    MissingScope,
    Ineligible,
    Disabled,
    Revoked,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderCapabilities {
    pub native_scheduling: bool,
    pub queue_scheduling: bool,
    pub upload_video: bool,
    pub upload_thumbnail: bool,
    pub public_posting: bool,
    pub requires_user_consent: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountEligibility {
    pub eligible: bool,
    pub reasons: Vec<String>,
}

impl AccountEligibility {
    pub fn eligible() -> Self {
        Self {
            eligible: true,
            reasons: Vec::new(),
        }
    }

    pub fn blocked(reason: impl Into<String>) -> Self {
        Self {
            eligible: false,
            reasons: vec![reason.into()],
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectedAccount {
    pub id: String,
    pub owner: OwnerRef,
    pub provider: Provider,
    pub provider_account_id: String,
    pub display_name: String,
    pub handle: Option<String>,
    pub avatar_url: Option<String>,
    pub account_kind: AccountKind,
    pub status: ConnectedAccountStatus,
    pub scopes: Vec<String>,
    pub capabilities: ProviderCapabilities,
    pub eligibility: AccountEligibility,
    pub last_verified_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_keys_are_stable() {
        assert_eq!(Provider::YouTube.as_str(), "youtube");
        assert_eq!(Provider::TikTok.as_str(), "tiktok");
        assert_eq!(Provider::Instagram.as_str(), "instagram");
    }

    #[test]
    fn connected_account_never_contains_token_material() {
        let account = ConnectedAccount {
            id: "acct_1".into(),
            owner: OwnerRef::User("user_1".into()),
            provider: Provider::YouTube,
            provider_account_id: "channel_1".into(),
            display_name: "Montage Channel".into(),
            handle: Some("@montage".into()),
            avatar_url: None,
            account_kind: AccountKind::Channel,
            status: ConnectedAccountStatus::Connected,
            scopes: vec!["youtube.upload".into()],
            capabilities: ProviderCapabilities::default(),
            eligibility: AccountEligibility::eligible(),
            last_verified_at: None,
            created_at: 1,
            updated_at: 1,
        };

        let json = serde_json::to_string(&account)
            .unwrap_or_else(|err| panic!("serialize account: {err}"));
        assert!(json.contains("Montage Channel"));
        assert!(!json.contains("access_token"));
        assert!(!json.contains("refresh_token"));
    }
}
```

Create empty module files so the crate compiles:

```rust
// crates/social/src/oauth.rs
```

```rust
// crates/social/src/token.rs
```

```rust
// crates/social/src/provider.rs
```

```rust
// crates/social/src/job.rs
```

- [ ] **Step 5: Run tests to verify models pass**

Run:

```bash
cargo test -p montage-social model::tests
```

Expected: PASS, two model tests pass.

- [ ] **Step 6: Commit**

Run:

```bash
git add Cargo.toml crates/social
git commit -m "feat(social): add connected account model"
```

## Task 2: OAuth Session Validation

**Files:**
- Modify: `crates/social/src/oauth.rs`

- [ ] **Step 1: Write failing OAuth tests**

Replace `crates/social/src/oauth.rs` with:

```rust
use crate::model::{OwnerRef, Provider};
use base64::Engine;
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OAuthConnection {
    pub id: String,
    pub owner: OwnerRef,
    pub provider: Provider,
    pub state_hash: String,
    pub requested_scopes: Vec<String>,
    pub return_to: String,
    pub status: OAuthConnectionStatus,
    pub created_at: i64,
    pub expires_at: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OAuthConnectionStatus {
    Started,
    Completed,
    Failed,
    Expired,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OAuthCallbackError {
    Expired,
    StateMismatch,
    AlreadyCompleted,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connection() -> OAuthConnection {
        OAuthConnection::start(
            "oauth_1",
            OwnerRef::User("user_1".into()),
            Provider::YouTube,
            "state-secret",
            vec!["youtube.upload".into()],
            "/campaigns/campaign_1".into(),
            100,
            200,
        )
    }

    #[test]
    fn start_hashes_state_instead_of_storing_raw_state() {
        let conn = connection();
        assert_ne!(conn.state_hash, "state-secret");
        assert!(conn.matches_state("state-secret"));
        assert!(!conn.matches_state("wrong"));
    }

    #[test]
    fn validate_callback_rejects_expired_sessions() {
        let conn = connection();
        let err = match conn.validate_callback("state-secret", 201) {
            Ok(()) => panic!("expected expired callback to fail"),
            Err(err) => err,
        };
        assert_eq!(err, OAuthCallbackError::Expired);
    }

    #[test]
    fn validate_callback_rejects_state_mismatch() {
        let conn = connection();
        let err = match conn.validate_callback("wrong", 150) {
            Ok(()) => panic!("expected state mismatch to fail"),
            Err(err) => err,
        };
        assert_eq!(err, OAuthCallbackError::StateMismatch);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run:

```bash
cargo test -p montage-social oauth::tests
```

Expected: FAIL with missing `OAuthConnection::start`, `matches_state`, and `validate_callback`.

- [ ] **Step 3: Implement OAuth session helpers**

Add these impl blocks above the tests in `crates/social/src/oauth.rs`:

```rust
impl OAuthConnection {
    #[allow(clippy::too_many_arguments)]
    pub fn start(
        id: impl Into<String>,
        owner: OwnerRef,
        provider: Provider,
        raw_state: &str,
        requested_scopes: Vec<String>,
        return_to: String,
        created_at: i64,
        expires_at: i64,
    ) -> Self {
        Self {
            id: id.into(),
            owner,
            provider,
            state_hash: hash_state(raw_state),
            requested_scopes,
            return_to,
            status: OAuthConnectionStatus::Started,
            created_at,
            expires_at,
        }
    }

    pub fn matches_state(&self, raw_state: &str) -> bool {
        self.state_hash == hash_state(raw_state)
    }

    pub fn validate_callback(
        &self,
        raw_state: &str,
        now: i64,
    ) -> Result<(), OAuthCallbackError> {
        if self.status != OAuthConnectionStatus::Started {
            return Err(OAuthCallbackError::AlreadyCompleted);
        }
        if now > self.expires_at {
            return Err(OAuthCallbackError::Expired);
        }
        if !self.matches_state(raw_state) {
            return Err(OAuthCallbackError::StateMismatch);
        }
        Ok(())
    }
}

fn hash_state(raw_state: &str) -> String {
    let digest = Sha256::digest(raw_state.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest)
}
```

- [ ] **Step 4: Run OAuth tests**

Run:

```bash
cargo test -p montage-social oauth::tests
```

Expected: PASS, three OAuth tests pass.

- [ ] **Step 5: Commit**

Run:

```bash
git add crates/social/src/oauth.rs
git commit -m "feat(social): add oauth session validation"
```

## Task 3: Token Secret Envelope

**Files:**
- Modify: `crates/social/src/token.rs`

- [ ] **Step 1: Write failing token tests**

Replace `crates/social/src/token.rs` with:

```rust
use base64::Engine;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenSecret {
    pub connected_account_id: String,
    pub encrypted_access_token: String,
    pub encrypted_refresh_token: Option<String>,
    pub access_token_expires_at: Option<i64>,
    pub refresh_token_expires_at: Option<i64>,
    pub token_version: u32,
    pub kms_key_id: String,
    pub last_refreshed_at: Option<i64>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum TokenError {
    #[error("key provider returned an empty key")]
    EmptyKey,
    #[error("encrypted token payload is not valid base64")]
    InvalidEncoding,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_secret_serialization_does_not_include_plaintext_tokens() {
        let secret = TokenSecret::encrypt(
            "acct_1",
            "access-secret",
            Some("refresh-secret"),
            &TestKeyProvider::new("test-key-1", "local-key"),
            100,
        )
        .unwrap_or_else(|err| panic!("encrypt token secret: {err}"));

        let json = serde_json::to_string(&secret)
            .unwrap_or_else(|err| panic!("serialize secret: {err}"));
        assert!(!json.contains("access-secret"));
        assert!(!json.contains("refresh-secret"));
        assert_eq!(secret.kms_key_id, "test-key-1");
    }

    #[test]
    fn test_key_provider_round_trips_token_material() {
        let provider = TestKeyProvider::new("test-key-1", "local-key");
        let secret = TokenSecret::encrypt("acct_1", "access-secret", None, &provider, 100)
            .unwrap_or_else(|err| panic!("encrypt token secret: {err}"));
        let decrypted = secret
            .decrypt_access_token(&provider)
            .unwrap_or_else(|err| panic!("decrypt access token: {err}"));
        assert_eq!(decrypted, "access-secret");
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run:

```bash
cargo test -p montage-social token::tests
```

Expected: FAIL with missing `TestKeyProvider`, `TokenSecret::encrypt`, and `decrypt_access_token`.

- [ ] **Step 3: Implement the test key provider boundary**

Add above the tests in `crates/social/src/token.rs`:

```rust
pub trait TokenKeyProvider {
    fn key_id(&self) -> &str;
    fn key_material(&self) -> &[u8];
}

#[derive(Clone, Debug)]
pub struct TestKeyProvider {
    key_id: String,
    key_material: Vec<u8>,
}

impl TestKeyProvider {
    pub fn new(key_id: impl Into<String>, key_material: impl Into<String>) -> Self {
        Self {
            key_id: key_id.into(),
            key_material: key_material.into().into_bytes(),
        }
    }
}

impl TokenKeyProvider for TestKeyProvider {
    fn key_id(&self) -> &str {
        &self.key_id
    }

    fn key_material(&self) -> &[u8] {
        &self.key_material
    }
}

impl TokenSecret {
    pub fn encrypt(
        connected_account_id: impl Into<String>,
        access_token: &str,
        refresh_token: Option<&str>,
        key_provider: &impl TokenKeyProvider,
        now: i64,
    ) -> Result<Self, TokenError> {
        Ok(Self {
            connected_account_id: connected_account_id.into(),
            encrypted_access_token: encode_with_key(access_token, key_provider)?,
            encrypted_refresh_token: refresh_token
                .map(|token| encode_with_key(token, key_provider))
                .transpose()?,
            access_token_expires_at: None,
            refresh_token_expires_at: None,
            token_version: 1,
            kms_key_id: key_provider.key_id().to_string(),
            last_refreshed_at: Some(now),
        })
    }

    pub fn decrypt_access_token(
        &self,
        key_provider: &impl TokenKeyProvider,
    ) -> Result<String, TokenError> {
        decode_with_key(&self.encrypted_access_token, key_provider)
    }
}

fn encode_with_key(value: &str, key_provider: &impl TokenKeyProvider) -> Result<String, TokenError> {
    let bytes = xor(value.as_bytes(), key_provider.key_material())?;
    Ok(base64::engine::general_purpose::STANDARD.encode(bytes))
}

fn decode_with_key(value: &str, key_provider: &impl TokenKeyProvider) -> Result<String, TokenError> {
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(value.as_bytes())
        .map_err(|_err| TokenError::InvalidEncoding)?;
    let plain = xor(&decoded, key_provider.key_material())?;
    Ok(String::from_utf8_lossy(&plain).into_owned())
}

fn xor(input: &[u8], key: &[u8]) -> Result<Vec<u8>, TokenError> {
    if key.is_empty() {
        return Err(TokenError::EmptyKey);
    }
    Ok(input
        .iter()
        .enumerate()
        .map(|(idx, byte)| byte ^ key[idx % key.len()])
        .collect())
}
```

This is a test/local key boundary, not production crypto. The production server must replace `TestKeyProvider` with KMS or envelope encryption while preserving the same `TokenKeyProvider` interface.

- [ ] **Step 4: Run token tests**

Run:

```bash
cargo test -p montage-social token::tests
```

Expected: PASS, two token tests pass.

- [ ] **Step 5: Commit**

Run:

```bash
git add crates/social/src/token.rs
git commit -m "feat(social): add token secret envelope"
```

## Task 4: Provider Adapter Registry And Capabilities

**Files:**
- Modify: `crates/social/src/provider.rs`

- [ ] **Step 1: Write failing provider tests**

Replace `crates/social/src/provider.rs` with:

```rust
use crate::model::{AccountEligibility, Provider, ProviderCapabilities};
use std::collections::BTreeMap;
use thiserror::Error;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderProfile {
    pub provider_account_id: String,
    pub display_name: String,
    pub handle: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderDescriptor {
    pub provider: Provider,
    pub display_name: &'static str,
    pub scopes: Vec<&'static str>,
    pub capabilities: ProviderCapabilities,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ProviderRegistryError {
    #[error("unknown provider: {0}")]
    UnknownProvider(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_registry_exposes_all_three_provider_slots() {
        let registry = ProviderRegistry::default_multi_platform();
        assert!(registry.get(&Provider::YouTube).is_ok());
        assert!(registry.get(&Provider::TikTok).is_ok());
        assert!(registry.get(&Provider::Instagram).is_ok());
    }

    #[test]
    fn youtube_is_upload_capable_while_tiktok_and_instagram_can_require_action() {
        let registry = ProviderRegistry::default_multi_platform();
        let youtube = registry
            .get(&Provider::YouTube)
            .unwrap_or_else(|err| panic!("youtube provider missing: {err}"));
        let tiktok = registry
            .get(&Provider::TikTok)
            .unwrap_or_else(|err| panic!("tiktok provider missing: {err}"));
        let instagram = registry
            .get(&Provider::Instagram)
            .unwrap_or_else(|err| panic!("instagram provider missing: {err}"));

        assert!(youtube.capabilities.upload_video);
        assert!(youtube.capabilities.native_scheduling);
        assert!(!tiktok.eligibility.eligible);
        assert!(!instagram.eligibility.eligible);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run:

```bash
cargo test -p montage-social provider::tests
```

Expected: FAIL with missing `ProviderRegistry`.

- [ ] **Step 3: Implement registry**

Add above tests in `crates/social/src/provider.rs`:

```rust
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderState {
    pub descriptor: ProviderDescriptor,
    pub eligibility: AccountEligibility,
}

#[derive(Clone, Debug, Default)]
pub struct ProviderRegistry {
    providers: BTreeMap<String, ProviderState>,
}

impl ProviderRegistry {
    pub fn default_multi_platform() -> Self {
        let mut registry = Self::default();
        registry.insert(ProviderState {
            descriptor: ProviderDescriptor {
                provider: Provider::YouTube,
                display_name: "YouTube",
                scopes: vec!["youtube.upload"],
                capabilities: ProviderCapabilities {
                    native_scheduling: true,
                    queue_scheduling: true,
                    upload_video: true,
                    upload_thumbnail: true,
                    public_posting: true,
                    requires_user_consent: false,
                },
            },
            eligibility: AccountEligibility::eligible(),
        });
        registry.insert(ProviderState {
            descriptor: ProviderDescriptor {
                provider: Provider::TikTok,
                display_name: "TikTok",
                scopes: vec!["video.publish"],
                capabilities: ProviderCapabilities {
                    native_scheduling: false,
                    queue_scheduling: true,
                    upload_video: false,
                    upload_thumbnail: false,
                    public_posting: false,
                    requires_user_consent: true,
                },
            },
            eligibility: AccountEligibility::blocked("tiktok_direct_post_permission_required"),
        });
        registry.insert(ProviderState {
            descriptor: ProviderDescriptor {
                provider: Provider::Instagram,
                display_name: "Instagram",
                scopes: vec!["instagram_content_publish"],
                capabilities: ProviderCapabilities {
                    native_scheduling: false,
                    queue_scheduling: true,
                    upload_video: false,
                    upload_thumbnail: false,
                    public_posting: false,
                    requires_user_consent: false,
                },
            },
            eligibility: AccountEligibility::blocked("instagram_professional_account_required"),
        });
        registry
    }

    pub fn insert(&mut self, state: ProviderState) {
        self.providers
            .insert(state.descriptor.provider.as_str().to_string(), state);
    }

    pub fn get(&self, provider: &Provider) -> Result<&ProviderState, ProviderRegistryError> {
        self.providers
            .get(provider.as_str())
            .ok_or_else(|| ProviderRegistryError::UnknownProvider(provider.as_str().into()))
    }
}
```

- [ ] **Step 4: Run provider tests**

Run:

```bash
cargo test -p montage-social provider::tests
```

Expected: PASS, two provider tests pass.

- [ ] **Step 5: Commit**

Run:

```bash
git add crates/social/src/provider.rs
git commit -m "feat(social): add provider capability registry"
```

## Task 5: Campaign Variant Targets And Publish Jobs

**Files:**
- Modify: `crates/social/src/model.rs`
- Modify: `crates/social/src/job.rs`

- [ ] **Step 1: Write failing publish job tests**

Replace `crates/social/src/job.rs` with:

```rust
use crate::model::{Provider, PublishJob, PublishJobStatus};
use base64::Engine;
use sha2::{Digest, Sha256};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publish_job_idempotency_key_is_stable_for_same_target() {
        let a = PublishJob::new(
            "job_1",
            "campaign_1",
            "variant_1",
            "acct_1",
            Provider::YouTube,
            "render://artifact_1",
            1_800,
            "user_1",
        );
        let b = PublishJob::new(
            "job_2",
            "campaign_1",
            "variant_1",
            "acct_1",
            Provider::YouTube,
            "render://artifact_1",
            1_800,
            "user_1",
        );

        assert_eq!(a.idempotency_key, b.idempotency_key);
        assert_eq!(a.status, PublishJobStatus::Draft);
    }

    #[test]
    fn publish_job_can_move_to_requires_action_with_reason() {
        let job = PublishJob::new(
            "job_1",
            "campaign_1",
            "variant_1",
            "acct_1",
            Provider::TikTok,
            "render://artifact_1",
            1_800,
            "user_1",
        )
        .requires_action("tiktok_direct_post_permission_required");

        assert_eq!(job.status, PublishJobStatus::RequiresAction);
        assert_eq!(
            job.requires_action_reason.as_deref(),
            Some("tiktok_direct_post_permission_required")
        );
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run:

```bash
cargo test -p montage-social job::tests
```

Expected: FAIL with missing `PublishJob` and `PublishJobStatus`.

- [ ] **Step 3: Add target and job models**

Append to `crates/social/src/model.rs`:

```rust
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CampaignVariantTarget {
    pub id: String,
    pub campaign_id: String,
    pub variant_id: String,
    pub connected_account_id: String,
    pub provider: Provider,
    pub platform_fields: serde_json::Value,
    pub scheduled_for: i64,
    pub validation_state: ValidationState,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationState {
    Pending,
    Valid,
    Invalid,
    RequiresAction,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PublishJobStatus {
    Draft,
    Validated,
    Scheduled,
    Uploading,
    Processing,
    Published,
    Failed,
    RequiresAction,
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishJob {
    pub id: String,
    pub campaign_id: String,
    pub variant_id: String,
    pub connected_account_id: String,
    pub provider: Provider,
    pub artifact_ref: String,
    pub idempotency_key: String,
    pub scheduled_for: i64,
    pub status: PublishJobStatus,
    pub attempt_count: u32,
    pub provider_post_id: Option<String>,
    pub provider_post_url: Option<String>,
    pub normalized_error: Option<String>,
    pub raw_error_ref: Option<String>,
    pub requires_action_reason: Option<String>,
    pub created_by: String,
    pub created_at: i64,
    pub updated_at: i64,
}
```

- [ ] **Step 4: Implement publish job construction**

Add above tests in `crates/social/src/job.rs`:

```rust
impl PublishJob {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: impl Into<String>,
        campaign_id: impl Into<String>,
        variant_id: impl Into<String>,
        connected_account_id: impl Into<String>,
        provider: Provider,
        artifact_ref: impl Into<String>,
        scheduled_for: i64,
        created_by: impl Into<String>,
    ) -> Self {
        let campaign_id = campaign_id.into();
        let variant_id = variant_id.into();
        let connected_account_id = connected_account_id.into();
        let artifact_ref = artifact_ref.into();
        let created_by = created_by.into();
        let idempotency_key =
            idempotency_key(&campaign_id, &variant_id, &connected_account_id, &artifact_ref);
        Self {
            id: id.into(),
            campaign_id,
            variant_id,
            connected_account_id,
            provider,
            artifact_ref,
            idempotency_key,
            scheduled_for,
            status: PublishJobStatus::Draft,
            attempt_count: 0,
            provider_post_id: None,
            provider_post_url: None,
            normalized_error: None,
            raw_error_ref: None,
            requires_action_reason: None,
            created_by,
            created_at: scheduled_for,
            updated_at: scheduled_for,
        }
    }

    pub fn requires_action(mut self, reason: impl Into<String>) -> Self {
        self.status = PublishJobStatus::RequiresAction;
        self.requires_action_reason = Some(reason.into());
        self
    }
}

fn idempotency_key(
    campaign_id: &str,
    variant_id: &str,
    connected_account_id: &str,
    artifact_ref: &str,
) -> String {
    let raw = format!("{campaign_id}:{variant_id}:{connected_account_id}:{artifact_ref}");
    let digest = Sha256::digest(raw.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest)
}
```

- [ ] **Step 5: Run job tests**

Run:

```bash
cargo test -p montage-social job::tests
```

Expected: PASS, two job tests pass.

- [ ] **Step 6: Commit**

Run:

```bash
git add crates/social/src/model.rs crates/social/src/job.rs
git commit -m "feat(social): add campaign publish jobs"
```

## Task 6: Foundation Verification

**Files:**
- Test only

- [ ] **Step 1: Run the full crate test suite**

Run:

```bash
cargo test -p montage-social
```

Expected: PASS, all `montage-social` unit tests pass.

- [ ] **Step 2: Run formatting check**

Run:

```bash
cargo fmt --all -- --check
```

Expected: PASS with no formatting diffs.

- [ ] **Step 3: Run focused clippy**

Run:

```bash
cargo clippy -p montage-social --all-targets -- -D warnings
```

Expected: PASS with no warnings.

- [ ] **Step 4: Check workspace diff hygiene**

Run:

```bash
git diff --check
git status --short --branch
```

Expected: `git diff --check` prints nothing. `git status` shows a clean branch after the task commits.

## Self-Review

Spec coverage:

- Connected social account model: Task 1.
- OAuth session start/callback state hashing and expiry: Task 2.
- Server-internal token secret envelope and no token exposure in public account models: Task 3.
- Multi-provider slots and capability/eligibility contracts for YouTube, TikTok, and Instagram: Task 4.
- Campaign variant target and durable publish job state model: Task 5.
- Test and lint verification: Task 6.

Intentional gaps for the next plan:

- No HTTP routes yet because the repository does not currently expose a dedicated web server crate.
- No database migrations yet because the server/database stack is still an open decision in the approved spec.
- No live Google/TikTok/Meta HTTP calls yet; provider adapters are contract and capability foundations only.
- No desktop UI changes in this plan; account selection UI should follow once the server boundary exists.
