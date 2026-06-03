# Social Upload Adapters Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add Phase 4 server-backed upload adapter contracts and a YouTube-first execution path that can publish claimed jobs without exposing provider tokens.

**Architecture:** Keep upload behavior inside `awidat-social` as framework-neutral Rust services. Provider-specific upload code implements a shared adapter trait; the worker service loads a claimed job, account, and token secret from `SocialStore`, calls the adapter, persists the job state, and appends audit events. Live HTTP remains behind a small trait so unit tests use mocked clients and no real provider credentials.

**Tech Stack:** Rust, `serde`, `serde_json`, existing `SocialStore`, existing encrypted `TokenSecret`, existing provider/account/job models.

---

## Scope

This is Phase 4A for upload adapters.

Included:
- provider-agnostic upload adapter request/result/error contracts
- YouTube upload request validation and mocked HTTP execution boundary
- upload execution service that moves jobs from `Uploading` to `Processing`, `Published`, `Failed`, or `RequiresAction`
- append-only provider/worker audit events
- TikTok and Instagram explicit unsupported/permission-blocked adapter slots

Excluded:
- live Google/TikTok/Meta network integration with real credentials
- web server routes
- UI
- background scheduler daemon
- production KMS/envelope encryption

## File Structure

- Create `crates/social/src/upload_adapter.rs`
  - Shared upload adapter traits and request/result/error types.
  - Mock adapter for service tests.
  - TikTok/Instagram blocked adapter slots.
- Create `crates/social/src/youtube_upload.rs`
  - YouTube-specific upload payload validation.
  - Mockable YouTube upload HTTP trait.
  - YouTube adapter implementation that returns provider IDs/URLs from mocked responses.
- Create `crates/social/src/upload_service.rs`
  - Upload worker execution service.
  - Loads job/account/token, checks owner/account/provider/token boundaries, calls adapters, saves job/events.
- Modify `crates/social/src/job.rs`
  - Add job transition helpers for `processing`, `published`, and provider-blocked `requires_action`.
- Modify `crates/social/src/lib.rs`
  - Export `upload_adapter`, `youtube_upload`, and `upload_service`.

## Task 1: Upload Adapter Domain Contracts

**Files:**
- Create: `crates/social/src/upload_adapter.rs`
- Modify: `crates/social/src/lib.rs`

- [ ] **Step 1: Write failing adapter contract tests**

Create `crates/social/src/upload_adapter.rs` with this test module first:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Provider;

    #[test]
    fn mock_adapter_returns_published_post_without_token_material() {
        let adapter = MockUploadAdapter::published(
            Provider::YouTube,
            "video_123",
            "https://youtube.com/watch?v=video_123",
        );
        let request = UploadRequest {
            job_id: "job_1".into(),
            provider: Provider::YouTube,
            connected_account_id: "acct_1".into(),
            artifact_ref: "file:///tmp/render.mp4".into(),
            title: "Launch clip".into(),
            description: Some("Description".into()),
            tags: vec!["awidat".into()],
            thumbnail_ref: Some("file:///tmp/thumb.jpg".into()),
            privacy: UploadPrivacy::Private,
            scheduled_for: Some(2_000),
            access_token_ref: "token-secret-ref".into(),
        };

        let result = adapter.upload(&request).unwrap_or_else(|err| {
            panic!("upload through mock adapter: {err:?}");
        });

        assert_eq!(result.provider_post_id, "video_123");
        assert_eq!(
            result.provider_post_url,
            "https://youtube.com/watch?v=video_123"
        );
        let json = serde_json::to_string(&request)
            .unwrap_or_else(|err| panic!("serialize upload request: {err}"));
        assert!(json.contains("token-secret-ref"));
        assert!(!json.contains("access_token"));
        assert!(!json.contains("refresh_token"));
    }

    #[test]
    fn blocked_adapter_maps_to_requires_action() {
        let adapter = BlockedUploadAdapter::new(
            Provider::TikTok,
            "tiktok_direct_post_permission_required",
        );
        let request = UploadRequest {
            job_id: "job_1".into(),
            provider: Provider::TikTok,
            connected_account_id: "acct_1".into(),
            artifact_ref: "file:///tmp/render.mp4".into(),
            title: "Launch clip".into(),
            description: None,
            tags: Vec::new(),
            thumbnail_ref: None,
            privacy: UploadPrivacy::Private,
            scheduled_for: None,
            access_token_ref: "token-secret-ref".into(),
        };

        assert_eq!(
            adapter.upload(&request),
            Err(UploadAdapterError::RequiresAction {
                reason: "tiktok_direct_post_permission_required".into(),
            })
        );
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run:

```bash
cargo test -p awidat-social upload_adapter::tests
```

Expected: FAIL because `upload_adapter` module is not exported and types do not exist.

- [ ] **Step 3: Implement adapter contracts**

Replace the file content with:

```rust
use crate::model::Provider;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UploadPrivacy {
    Private,
    Unlisted,
    Public,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UploadRequest {
    pub job_id: String,
    pub provider: Provider,
    pub connected_account_id: String,
    pub artifact_ref: String,
    pub title: String,
    pub description: Option<String>,
    pub tags: Vec<String>,
    pub thumbnail_ref: Option<String>,
    pub privacy: UploadPrivacy,
    pub scheduled_for: Option<i64>,
    pub access_token_ref: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UploadResult {
    pub provider_post_id: String,
    pub provider_post_url: String,
    pub processing: bool,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum UploadAdapterError {
    #[error("provider mismatch")]
    ProviderMismatch,
    #[error("missing upload token")]
    MissingUploadToken,
    #[error("media constraint failed: {reason}")]
    MediaConstraintFailed { reason: String },
    #[error("requires action: {reason}")]
    RequiresAction { reason: String },
    #[error("network or server error: {message}")]
    NetworkOrServer { message: String },
}

pub trait UploadAdapter {
    fn provider(&self) -> Provider;
    fn upload(&self, request: &UploadRequest) -> Result<UploadResult, UploadAdapterError>;
}

#[derive(Clone, Debug)]
pub struct MockUploadAdapter {
    provider: Provider,
    result: Result<UploadResult, UploadAdapterError>,
}

impl MockUploadAdapter {
    pub fn published(
        provider: Provider,
        provider_post_id: impl Into<String>,
        provider_post_url: impl Into<String>,
    ) -> Self {
        Self {
            provider,
            result: Ok(UploadResult {
                provider_post_id: provider_post_id.into(),
                provider_post_url: provider_post_url.into(),
                processing: false,
            }),
        }
    }

    pub fn failing(provider: Provider, error: UploadAdapterError) -> Self {
        Self {
            provider,
            result: Err(error),
        }
    }
}

impl UploadAdapter for MockUploadAdapter {
    fn provider(&self) -> Provider {
        self.provider.clone()
    }

    fn upload(&self, request: &UploadRequest) -> Result<UploadResult, UploadAdapterError> {
        if request.provider != self.provider {
            return Err(UploadAdapterError::ProviderMismatch);
        }
        self.result.clone()
    }
}

#[derive(Clone, Debug)]
pub struct BlockedUploadAdapter {
    provider: Provider,
    reason: String,
}

impl BlockedUploadAdapter {
    pub fn new(provider: Provider, reason: impl Into<String>) -> Self {
        Self {
            provider,
            reason: reason.into(),
        }
    }
}

impl UploadAdapter for BlockedUploadAdapter {
    fn provider(&self) -> Provider {
        self.provider.clone()
    }

    fn upload(&self, request: &UploadRequest) -> Result<UploadResult, UploadAdapterError> {
        if request.provider != self.provider {
            return Err(UploadAdapterError::ProviderMismatch);
        }
        Err(UploadAdapterError::RequiresAction {
            reason: self.reason.clone(),
        })
    }
}
```

Update `crates/social/src/lib.rs`:

```rust
pub mod upload_adapter;
```

- [ ] **Step 4: Run test to verify it passes**

Run:

```bash
cargo test -p awidat-social upload_adapter::tests
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/social/src/upload_adapter.rs crates/social/src/lib.rs
git commit -m "feat(social): add upload adapter contract"
```

## Task 2: Job Upload State Transitions

**Files:**
- Modify: `crates/social/src/job.rs`

- [ ] **Step 1: Write failing job transition tests**

Add these tests to `crates/social/src/job.rs` inside the existing test module:

```rust
#[test]
fn publish_job_can_move_to_processing_and_published() {
    let job = PublishJob::new(
        "job_1",
        "campaign_1",
        "variant_1",
        "acct_1",
        Provider::YouTube,
        "render://artifact_1",
        1_800,
        "user_1",
    )
    .claim_for_upload(2_000)
    .processing("yt_processing_1", 2_100);

    assert_eq!(job.status, PublishJobStatus::Processing);
    assert_eq!(job.provider_post_id.as_deref(), Some("yt_processing_1"));
    assert_eq!(job.updated_at, 2_100);

    let published = job.publish(
        "yt_video_1",
        "https://youtube.com/watch?v=yt_video_1",
        2_300,
    );
    assert_eq!(published.status, PublishJobStatus::Published);
    assert_eq!(published.provider_post_id.as_deref(), Some("yt_video_1"));
    assert_eq!(
        published.provider_post_url.as_deref(),
        Some("https://youtube.com/watch?v=yt_video_1")
    );
    assert_eq!(published.updated_at, 2_300);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run:

```bash
cargo test -p awidat-social job::tests::publish_job_can_move_to_processing_and_published
```

Expected: FAIL with missing `processing` and `publish` methods.

- [ ] **Step 3: Implement job transitions**

Add these methods to the existing `impl PublishJob` block:

```rust
pub fn processing(mut self, provider_post_id: impl Into<String>, now: i64) -> Self {
    self.status = PublishJobStatus::Processing;
    self.provider_post_id = Some(provider_post_id.into());
    self.updated_at = now;
    self
}

pub fn publish(
    mut self,
    provider_post_id: impl Into<String>,
    provider_post_url: impl Into<String>,
    now: i64,
) -> Self {
    self.status = PublishJobStatus::Published;
    self.provider_post_id = Some(provider_post_id.into());
    self.provider_post_url = Some(provider_post_url.into());
    self.normalized_error = None;
    self.raw_error_ref = None;
    self.requires_action_reason = None;
    self.updated_at = now;
    self
}
```

- [ ] **Step 4: Run test to verify it passes**

Run:

```bash
cargo test -p awidat-social job::tests::publish_job_can_move_to_processing_and_published
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/social/src/job.rs
git commit -m "feat(social): add publish job upload transitions"
```

## Task 3: YouTube Upload Adapter Boundary

**Files:**
- Create: `crates/social/src/youtube_upload.rs`
- Modify: `crates/social/src/lib.rs`

- [ ] **Step 1: Write failing YouTube adapter tests**

Create `crates/social/src/youtube_upload.rs` with these tests first:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Provider;
    use crate::upload_adapter::{UploadAdapter, UploadPrivacy, UploadRequest};

    #[derive(Clone, Debug, Default)]
    struct RecordingYouTubeClient {
        response: Option<YouTubeUploadResponse>,
    }

    impl YouTubeUploadClient for RecordingYouTubeClient {
        fn upload_video(
            &self,
            request: &YouTubeUploadRequest,
        ) -> Result<YouTubeUploadResponse, YouTubeUploadClientError> {
            assert_eq!(request.access_token_ref, "token-secret-ref");
            assert_eq!(request.title, "Launch clip");
            assert_eq!(request.privacy, "private");
            Ok(self.response.clone().unwrap_or_else(|| YouTubeUploadResponse {
                video_id: "yt_video_1".into(),
                processing: false,
            }))
        }
    }

    #[test]
    fn youtube_adapter_maps_upload_response_to_provider_post() {
        let adapter = YouTubeUploadAdapter::new(RecordingYouTubeClient::default());
        let result = adapter
            .upload(&UploadRequest {
                job_id: "job_1".into(),
                provider: Provider::YouTube,
                connected_account_id: "acct_1".into(),
                artifact_ref: "file:///tmp/render.mp4".into(),
                title: "Launch clip".into(),
                description: Some("Description".into()),
                tags: vec!["awidat".into()],
                thumbnail_ref: Some("file:///tmp/thumb.jpg".into()),
                privacy: UploadPrivacy::Private,
                scheduled_for: Some(2_000),
                access_token_ref: "token-secret-ref".into(),
            })
            .unwrap_or_else(|err| panic!("youtube upload: {err:?}"));

        assert_eq!(result.provider_post_id, "yt_video_1");
        assert_eq!(
            result.provider_post_url,
            "https://www.youtube.com/watch?v=yt_video_1"
        );
        assert!(!result.processing);
    }

    #[test]
    fn youtube_adapter_rejects_missing_title_and_wrong_provider() {
        let adapter = YouTubeUploadAdapter::new(RecordingYouTubeClient::default());
        let mut request = UploadRequest {
            job_id: "job_1".into(),
            provider: Provider::TikTok,
            connected_account_id: "acct_1".into(),
            artifact_ref: "file:///tmp/render.mp4".into(),
            title: "Launch clip".into(),
            description: None,
            tags: Vec::new(),
            thumbnail_ref: None,
            privacy: UploadPrivacy::Private,
            scheduled_for: None,
            access_token_ref: "token-secret-ref".into(),
        };
        assert_eq!(adapter.upload(&request), Err(crate::upload_adapter::UploadAdapterError::ProviderMismatch));

        request.provider = Provider::YouTube;
        request.title = "   ".into();
        assert_eq!(
            adapter.upload(&request),
            Err(crate::upload_adapter::UploadAdapterError::MediaConstraintFailed {
                reason: "youtube_title_required".into(),
            })
        );
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run:

```bash
cargo test -p awidat-social youtube_upload::tests
```

Expected: FAIL because module/types do not exist.

- [ ] **Step 3: Implement YouTube adapter boundary**

Replace `crates/social/src/youtube_upload.rs` with:

```rust
use crate::model::Provider;
use crate::upload_adapter::{
    UploadAdapter, UploadAdapterError, UploadPrivacy, UploadRequest, UploadResult,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct YouTubeUploadRequest {
    pub artifact_ref: String,
    pub thumbnail_ref: Option<String>,
    pub title: String,
    pub description: Option<String>,
    pub tags: Vec<String>,
    pub privacy: String,
    pub scheduled_for: Option<i64>,
    pub access_token_ref: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct YouTubeUploadResponse {
    pub video_id: String,
    pub processing: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum YouTubeUploadClientError {
    MissingScope,
    AccountNotEligible,
    NetworkOrServer(String),
}

pub trait YouTubeUploadClient {
    fn upload_video(
        &self,
        request: &YouTubeUploadRequest,
    ) -> Result<YouTubeUploadResponse, YouTubeUploadClientError>;
}

#[derive(Clone, Debug)]
pub struct YouTubeUploadAdapter<C> {
    client: C,
}

impl<C> YouTubeUploadAdapter<C> {
    pub fn new(client: C) -> Self {
        Self { client }
    }
}

impl<C: YouTubeUploadClient> UploadAdapter for YouTubeUploadAdapter<C> {
    fn provider(&self) -> Provider {
        Provider::YouTube
    }

    fn upload(&self, request: &UploadRequest) -> Result<UploadResult, UploadAdapterError> {
        if request.provider != Provider::YouTube {
            return Err(UploadAdapterError::ProviderMismatch);
        }
        if request.title.trim().is_empty() {
            return Err(UploadAdapterError::MediaConstraintFailed {
                reason: "youtube_title_required".into(),
            });
        }
        if request.access_token_ref.trim().is_empty() {
            return Err(UploadAdapterError::MissingUploadToken);
        }

        let youtube_request = YouTubeUploadRequest {
            artifact_ref: request.artifact_ref.clone(),
            thumbnail_ref: request.thumbnail_ref.clone(),
            title: request.title.trim().to_string(),
            description: request.description.clone(),
            tags: request.tags.clone(),
            privacy: youtube_privacy(&request.privacy).to_string(),
            scheduled_for: request.scheduled_for,
            access_token_ref: request.access_token_ref.clone(),
        };
        let response = self
            .client
            .upload_video(&youtube_request)
            .map_err(youtube_client_error)?;
        Ok(UploadResult {
            provider_post_url: format!("https://www.youtube.com/watch?v={}", response.video_id),
            provider_post_id: response.video_id,
            processing: response.processing,
        })
    }
}

fn youtube_privacy(privacy: &UploadPrivacy) -> &'static str {
    match privacy {
        UploadPrivacy::Private => "private",
        UploadPrivacy::Unlisted => "unlisted",
        UploadPrivacy::Public => "public",
    }
}

fn youtube_client_error(error: YouTubeUploadClientError) -> UploadAdapterError {
    match error {
        YouTubeUploadClientError::MissingScope => UploadAdapterError::RequiresAction {
            reason: "missing_scope".into(),
        },
        YouTubeUploadClientError::AccountNotEligible => UploadAdapterError::RequiresAction {
            reason: "account_not_eligible".into(),
        },
        YouTubeUploadClientError::NetworkOrServer(message) => {
            UploadAdapterError::NetworkOrServer { message }
        }
    }
}
```

Update `crates/social/src/lib.rs`:

```rust
pub mod youtube_upload;
```

- [ ] **Step 4: Run test to verify it passes**

Run:

```bash
cargo test -p awidat-social youtube_upload::tests
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/social/src/youtube_upload.rs crates/social/src/lib.rs
git commit -m "feat(social): add youtube upload adapter"
```

## Task 4: Upload Execution Service

**Files:**
- Create: `crates/social/src/upload_service.rs`
- Modify: `crates/social/src/lib.rs`

- [ ] **Step 1: Write failing service tests**

Create `crates/social/src/upload_service.rs` with tests first. Use the existing in-memory store and test token provider:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        AccountEligibility, AccountKind, ConnectedAccount, ConnectedAccountStatus, OwnerRef,
        Provider, ProviderCapabilities, PublishJob, PublishJobActorType, PublishJobEventType,
        PublishJobStatus,
    };
    use crate::store::{InMemorySocialStore, SocialStore};
    use crate::token::{TestKeyProvider, TokenSecret};
    use crate::upload_adapter::{MockUploadAdapter, UploadAdapterError};

    #[test]
    fn execute_upload_publishes_job_and_appends_provider_event() {
        let mut store = seeded_store(PublishJob::new(
            "job_1",
            "campaign_1",
            "variant_1",
            "acct_1",
            Provider::YouTube,
            "render://artifact_1",
            2_000,
            "user_1",
        )
        .claim_for_upload(2_100));
        let adapter = MockUploadAdapter::published(
            Provider::YouTube,
            "yt_video_1",
            "https://youtube.com/watch?v=yt_video_1",
        );

        let job = UploadService::execute_claimed_job(
            &mut store,
            &adapter,
            ExecuteUploadInput {
                job_id: "job_1".into(),
                title: "Launch clip".into(),
                description: Some("Description".into()),
                tags: vec!["awidat".into()],
                thumbnail_ref: Some("render://thumb_1".into()),
                now: 2_200,
            },
        )
        .unwrap_or_else(|err| panic!("execute upload: {err}"));

        assert_eq!(job.status, PublishJobStatus::Published);
        assert_eq!(job.provider_post_id.as_deref(), Some("yt_video_1"));
        assert_eq!(
            job.provider_post_url.as_deref(),
            Some("https://youtube.com/watch?v=yt_video_1")
        );
        let events = store
            .publish_job_events("job_1")
            .unwrap_or_else(|err| panic!("events: {err}"));
        assert!(events.iter().any(|event| {
            event.event_type == PublishJobEventType::Scheduled
        }));
        assert!(events.iter().any(|event| {
            event.event_type == PublishJobEventType::Claimed
                && event.actor_type == PublishJobActorType::Worker
        }));
    }

    #[test]
    fn execute_upload_requires_uploading_status_and_maps_requires_action() {
        let mut store = seeded_store(PublishJob::new(
            "job_1",
            "campaign_1",
            "variant_1",
            "acct_1",
            Provider::TikTok,
            "render://artifact_1",
            2_000,
            "user_1",
        )
        .claim_for_upload(2_100));
        let adapter = MockUploadAdapter::failing(
            Provider::TikTok,
            UploadAdapterError::RequiresAction {
                reason: "tiktok_direct_post_permission_required".into(),
            },
        );

        let job = UploadService::execute_claimed_job(
            &mut store,
            &adapter,
            ExecuteUploadInput {
                job_id: "job_1".into(),
                title: "Launch clip".into(),
                description: None,
                tags: Vec::new(),
                thumbnail_ref: None,
                now: 2_200,
            },
        )
        .unwrap_or_else(|err| panic!("execute upload: {err}"));

        assert_eq!(job.status, PublishJobStatus::RequiresAction);
        assert_eq!(
            job.requires_action_reason.as_deref(),
            Some("tiktok_direct_post_permission_required")
        );
    }

    #[test]
    fn execute_upload_rejects_wrong_state_before_provider_call() {
        let mut store = seeded_store(PublishJob::new(
            "job_1",
            "campaign_1",
            "variant_1",
            "acct_1",
            Provider::YouTube,
            "render://artifact_1",
            2_000,
            "user_1",
        )
        .schedule(2_000));
        let adapter = MockUploadAdapter::published(
            Provider::YouTube,
            "yt_video_1",
            "https://youtube.com/watch?v=yt_video_1",
        );

        assert_eq!(
            UploadService::execute_claimed_job(
                &mut store,
                &adapter,
                ExecuteUploadInput {
                    job_id: "job_1".into(),
                    title: "Launch clip".into(),
                    description: None,
                    tags: Vec::new(),
                    thumbnail_ref: None,
                    now: 2_200,
                },
            ),
            Err(UploadServiceError::JobNotUploading)
        );
    }

    fn seeded_store(job: PublishJob) -> InMemorySocialStore {
        let mut store = InMemorySocialStore::default();
        store
            .save_connected_account(connected_account(&job.connected_account_id, job.provider.clone()))
            .unwrap_or_else(|err| panic!("save account: {err}"));
        store
            .save_token_secret(token_secret(&job.connected_account_id))
            .unwrap_or_else(|err| panic!("save token secret: {err}"));
        store
            .save_publish_job(job.clone())
            .unwrap_or_else(|err| panic!("save publish job: {err}"));
        store
            .append_publish_job_event(crate::model::PublishJobEvent::new(
                "event_job_1_scheduled",
                job.id,
                PublishJobEventType::Scheduled,
                PublishJobActorType::User,
                "publish job scheduled",
                serde_json::json!({}),
                2_000,
            ))
            .unwrap_or_else(|err| panic!("append scheduled event: {err}"));
        store
    }

    fn connected_account(id: &str, provider: Provider) -> ConnectedAccount {
        ConnectedAccount {
            id: id.into(),
            owner: OwnerRef::User("user_1".into()),
            provider,
            provider_account_id: "channel_1".into(),
            display_name: "Awidat Channel".into(),
            handle: Some("@awidat".into()),
            avatar_url: None,
            account_kind: AccountKind::Channel,
            status: ConnectedAccountStatus::Connected,
            scopes: vec!["https://www.googleapis.com/auth/youtube.upload".into()],
            capabilities: ProviderCapabilities {
                native_scheduling: true,
                queue_scheduling: true,
                upload_video: true,
                upload_thumbnail: true,
                public_posting: true,
                requires_user_consent: false,
            },
            eligibility: AccountEligibility::eligible(),
            last_verified_at: Some(1_900),
            created_at: 1_900,
            updated_at: 1_900,
        }
    }

    fn token_secret(account_id: &str) -> TokenSecret {
        TokenSecret::encrypt(
            account_id,
            "access_token",
            Some("refresh_token"),
            9_999,
            None,
            &TestKeyProvider::new("phase4-key"),
        )
        .unwrap_or_else(|err| panic!("encrypt token: {err}"))
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run:

```bash
cargo test -p awidat-social upload_service::tests
```

Expected: FAIL because service types do not exist.

- [ ] **Step 3: Implement upload execution service**

Replace the non-test content in `crates/social/src/upload_service.rs` with:

```rust
use crate::model::{
    PublishJob, PublishJobActorType, PublishJobEvent, PublishJobEventType, PublishJobStatus,
};
use crate::store::{SocialStore, SocialStoreError};
use crate::upload_adapter::{
    UploadAdapter, UploadAdapterError, UploadPrivacy, UploadRequest, UploadResult,
};
use thiserror::Error;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecuteUploadInput {
    pub job_id: String,
    pub title: String,
    pub description: Option<String>,
    pub tags: Vec<String>,
    pub thumbnail_ref: Option<String>,
    pub now: i64,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum UploadServiceError {
    #[error(transparent)]
    Store(#[from] SocialStoreError),
    #[error("publish job is not uploading")]
    JobNotUploading,
    #[error("adapter provider does not match job provider")]
    ProviderMismatch,
}

pub struct UploadService;

impl UploadService {
    pub fn execute_claimed_job(
        store: &mut impl SocialStore,
        adapter: &impl UploadAdapter,
        input: ExecuteUploadInput,
    ) -> Result<PublishJob, UploadServiceError> {
        let job = store.publish_job(&input.job_id)?;
        if job.status != PublishJobStatus::Uploading {
            return Err(UploadServiceError::JobNotUploading);
        }
        if adapter.provider() != job.provider {
            return Err(UploadServiceError::ProviderMismatch);
        }
        let account = store.connected_account(&job.connected_account_id)?;
        let token = store.token_secret_for_account(&account.id)?;

        let request = UploadRequest {
            job_id: job.id.clone(),
            provider: job.provider.clone(),
            connected_account_id: account.id,
            artifact_ref: job.artifact_ref.clone(),
            title: input.title,
            description: input.description,
            tags: input.tags,
            thumbnail_ref: input.thumbnail_ref,
            privacy: UploadPrivacy::Private,
            scheduled_for: Some(job.scheduled_for),
            access_token_ref: format!("token_secret:{}", token.connected_account_id),
        };
        store.append_publish_job_event(PublishJobEvent::new(
            format!("event_{}_claimed_{}", job.id, input.now),
            job.id.clone(),
            PublishJobEventType::Claimed,
            PublishJobActorType::Worker,
            "publish job claimed for upload",
            serde_json::json!({}),
            input.now,
        ))?;

        match adapter.upload(&request) {
            Ok(result) => Self::complete_success(store, job, result, input.now),
            Err(error) => Self::complete_error(store, job, error, input.now),
        }
    }

    fn complete_success(
        store: &mut impl SocialStore,
        job: PublishJob,
        result: UploadResult,
        now: i64,
    ) -> Result<PublishJob, UploadServiceError> {
        let updated = if result.processing {
            job.processing(result.provider_post_id, now)
        } else {
            job.publish(result.provider_post_id, result.provider_post_url, now)
        };
        store.save_publish_job(updated.clone())?;
        store.append_publish_job_event(PublishJobEvent::new(
            format!("event_{}_uploaded_{}", updated.id, now),
            updated.id.clone(),
            PublishJobEventType::Validated,
            PublishJobActorType::Provider,
            "provider upload accepted",
            serde_json::json!({
                "provider_post_id": updated.provider_post_id,
                "provider_post_url": updated.provider_post_url,
            }),
            now,
        ))?;
        Ok(updated)
    }

    fn complete_error(
        store: &mut impl SocialStore,
        job: PublishJob,
        error: UploadAdapterError,
        now: i64,
    ) -> Result<PublishJob, UploadServiceError> {
        let updated = match error {
            UploadAdapterError::RequiresAction { reason } => job.requires_action(reason, now),
            UploadAdapterError::ProviderMismatch => return Err(UploadServiceError::ProviderMismatch),
            UploadAdapterError::MissingUploadToken => {
                job.requires_action("missing_upload_token", now)
            }
            UploadAdapterError::MediaConstraintFailed { reason } => {
                job.fail(reason, "provider_error_ref_unavailable", now)
            }
            UploadAdapterError::NetworkOrServer { message } => {
                job.fail("network_or_server_error", message, now)
            }
        };
        store.save_publish_job(updated.clone())?;
        store.append_publish_job_event(PublishJobEvent::new(
            format!("event_{}_upload_blocked_{}", updated.id, now),
            updated.id.clone(),
            if updated.status == PublishJobStatus::RequiresAction {
                PublishJobEventType::RequiresAction
            } else {
                PublishJobEventType::Failed
            },
            PublishJobActorType::Provider,
            "provider upload did not complete",
            serde_json::json!({
                "status": format!("{:?}", updated.status),
                "normalized_error": updated.normalized_error,
                "requires_action_reason": updated.requires_action_reason,
            }),
            now,
        ))?;
        Ok(updated)
    }
}
```

Update `crates/social/src/lib.rs`:

```rust
pub mod upload_service;
```

- [ ] **Step 4: Run test to verify it passes**

Run:

```bash
cargo test -p awidat-social upload_service::tests
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/social/src/upload_service.rs crates/social/src/lib.rs
git commit -m "feat(social): execute claimed upload jobs"
```

## Task 5: Phase 4 Verification

**Files:**
- No planned edits.

- [ ] **Step 1: Run focused Phase 4 tests**

Run:

```bash
cargo test -p awidat-social upload_adapter::tests youtube_upload::tests upload_service::tests
```

Expected: Cargo accepts only one filter. Run the three filters separately:

```bash
cargo test -p awidat-social upload_adapter::tests
cargo test -p awidat-social youtube_upload::tests
cargo test -p awidat-social upload_service::tests
```

Expected: PASS.

- [ ] **Step 2: Run full social crate verification**

Run:

```bash
cargo test -p awidat-social
cargo clippy -p awidat-social --all-targets -- -D warnings
cargo fmt --all -- --check
git diff --check
```

Expected: PASS. `cargo fmt` may emit existing stable-toolchain warnings about `imports_granularity`; the command must still exit 0.

- [ ] **Step 3: Final review**

Dispatch a final reviewer with this scope:

```text
Review Phase 4 upload adapters only:
- upload adapter contracts
- YouTube adapter boundary
- upload execution service
- job upload transitions
- no live HTTP/server/UI required

Check correctness, token exposure, audit event behavior, state transitions,
multi-provider boundaries, and test coverage.
```

- [ ] **Step 4: Commit review fixes if any**

If review rejects, fix the concrete finding, rerun:

```bash
cargo test -p awidat-social
cargo clippy -p awidat-social --all-targets -- -D warnings
cargo fmt --all -- --check
git diff --check
```

Commit:

```bash
git add crates/social/src
git commit -m "fix(social): validate upload adapter execution"
```

- [ ] **Step 5: Completion status**

Report:

```text
Phase 4A complete:
- adapter contracts
- YouTube mocked adapter boundary
- upload execution service
- tests and final review
```

