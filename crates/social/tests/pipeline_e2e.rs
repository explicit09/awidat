//! End-to-end verification of the social publishing pipeline.
//!
//! These integration tests drive the *whole* lifecycle through the public
//! `SocialApi` facade and the queue-claim step, exactly as a future server
//! wrapper and worker would:
//!
//!   oauth_start -> oauth_complete -> bind_target -> validate_target
//!     -> schedule_target -> claim_due_jobs -> execute_claimed_upload_job
//!     -> poll_upload_status -> Published
//!
//! Every observable boundary (API responses and audit events) is asserted to
//! be free of provider token material. The harness is generic over
//! `SocialStore` so it exercises the same lifecycle production runs against
//! `PgSocialStore`, using `InMemorySocialStore` as the fast test double.

use montage_social::api::{
    ApiActor, ApiOwner, BindTargetRequest, ExecuteUploadRequest, OAuthCompleteRequest,
    OAuthStartRequest, ScheduleTargetRequest, SocialApi, ValidateTargetRequest,
};
use montage_social::model::{
    AccountEligibility, AccountKind, ConnectedAccount, ConnectedAccountStatus, OwnerRef, Provider,
    ProviderCapabilities, PublishJobStatus, ValidationState,
};
use montage_social::oauth_url::OAuthProviderConfig;
use montage_social::provider::ProviderRegistry;
use montage_social::publish_service::PublishService;
use montage_social::store::{InMemorySocialStore, SocialStore};
use montage_social::token::TestKeyProvider;
use montage_social::token_bundle::ProviderTokenBundle;
use montage_social::upload_adapter::{
    UploadAdapter, UploadAdapterError, UploadRequest, UploadResult,
};
use montage_social::upload_status::{
    UploadProcessingStatus, UploadStatusAdapter, UploadStatusAdapterError, UploadStatusRequest,
    UploadStatusResult,
};

const ACCESS_TOKEN: &str = "access-secret-e2e";
const REFRESH_TOKEN: &str = "refresh-secret-e2e";
const PROVIDER_POST_ID: &str = "yt_video_e2e";
const PROVIDER_POST_URL: &str = "https://www.youtube.com/watch?v=yt_video_e2e";

/// Upload adapter that reports the upload accepted but still processing, so the
/// status poller drives the job to Published. Mirrors the real YouTube path.
struct ProcessingUploadAdapter;

impl UploadAdapter for ProcessingUploadAdapter {
    fn provider(&self) -> Provider {
        Provider::YouTube
    }

    fn upload(&self, request: &UploadRequest) -> Result<UploadResult, UploadAdapterError> {
        // The worker must never hand the adapter raw token material.
        assert!(!request.access_token_ref.contains(ACCESS_TOKEN));
        assert!(!request.access_token_ref.contains(REFRESH_TOKEN));
        Ok(UploadResult {
            provider_post_id: PROVIDER_POST_ID.into(),
            provider_post_url: PROVIDER_POST_URL.into(),
            processing: true,
        })
    }
}

/// Status adapter that reports the provider finished processing.
struct PublishedStatusAdapter;

impl UploadStatusAdapter for PublishedStatusAdapter {
    fn provider(&self) -> Provider {
        Provider::YouTube
    }

    fn poll_status(
        &self,
        request: &UploadStatusRequest,
    ) -> Result<UploadStatusResult, UploadStatusAdapterError> {
        assert!(!request.access_token_ref.contains(ACCESS_TOKEN));
        Ok(UploadStatusResult {
            provider_post_id: PROVIDER_POST_ID.into(),
            provider_post_url: Some(PROVIDER_POST_URL.into()),
            status: UploadProcessingStatus::Published,
            normalized_error: None,
            raw_error_ref: None,
        })
    }
}

fn config() -> OAuthProviderConfig {
    OAuthProviderConfig {
        client_id: "client_e2e".into(),
        redirect_uri: "https://app.montage.test/social/oauth/callback".into(),
    }
}

fn connected_account(owner: OwnerRef) -> ConnectedAccount {
    ConnectedAccount {
        id: "acct_e2e".into(),
        owner,
        provider: Provider::YouTube,
        provider_account_id: "channel_e2e".into(),
        display_name: "Montage E2E Channel".into(),
        handle: Some("@montage".into()),
        avatar_url: None,
        account_kind: AccountKind::Channel,
        status: ConnectedAccountStatus::Connected,
        scopes: vec!["old.scope".into()],
        capabilities: ProviderCapabilities {
            native_scheduling: true,
            queue_scheduling: true,
            upload_video: true,
            upload_thumbnail: true,
            public_posting: true,
            requires_user_consent: false,
        },
        eligibility: AccountEligibility::eligible(),
        last_verified_at: None,
        created_at: 100,
        updated_at: 100,
    }
}

fn token_bundle() -> ProviderTokenBundle {
    ProviderTokenBundle {
        provider: Provider::YouTube,
        provider_account_id: "channel_e2e".into(),
        scopes: vec!["https://www.googleapis.com/auth/youtube.upload".into()],
        access_token_expires_at: 9_000,
        refresh_token_expires_at: Some(18_000),
    }
}

/// Asserts a serializable value carries no provider token material.
fn assert_token_safe<T: serde::Serialize>(label: &str, value: &T) {
    let json =
        serde_json::to_string(value).unwrap_or_else(|err| panic!("serialize {label}: {err}"));
    for needle in [
        ACCESS_TOKEN,
        REFRESH_TOKEN,
        "access_token",
        "refresh_token",
        "encrypted",
    ] {
        assert!(
            !json.contains(needle),
            "{label} leaked token material ({needle}): {json}"
        );
    }
}

/// Drives the full pipeline for a user-owned account against any store and
/// returns nothing; panics on any deviation.
fn run_full_pipeline<S: SocialStore>(store: &mut S, store_label: &str) {
    let owner = OwnerRef::User("user_e2e".into());
    let actor = ApiActor::new("user_e2e", Vec::new());
    let api_owner = ApiOwner {
        owner: owner.clone(),
    };
    let registry = ProviderRegistry::default_multi_platform();
    let key_provider = TestKeyProvider::new("key-e2e", "local-key");

    // 1. Providers list is available and token-safe.
    let providers = SocialApi::providers(&registry);
    assert_eq!(providers.len(), 4, "[{store_label}] expected 4 providers");
    assert_token_safe(&format!("[{store_label}] providers"), &providers);

    // 2. OAuth start persists a connection and returns an authorization URL.
    let start = SocialApi::oauth_start(
        store,
        &actor,
        OAuthStartRequest {
            oauth_connection_id: "oauth_e2e".into(),
            owner: owner.clone(),
            provider: Provider::YouTube,
            config: config(),
            raw_state: "state-e2e".into(),
            return_to: "/campaigns/campaign_e2e".into(),
            created_at: 100,
            expires_at: 10_000,
        },
    )
    .unwrap_or_else(|err| panic!("[{store_label}] oauth start: {err}"));
    assert!(
        start
            .authorization_url
            .starts_with("https://accounts.google.com/o/oauth2/v2/auth?")
    );

    // 3. OAuth complete persists account + encrypted token, returns no secrets.
    let complete = SocialApi::oauth_complete(
        store,
        &key_provider,
        &actor,
        OAuthCompleteRequest {
            oauth_connection_id: "oauth_e2e".into(),
            owner: owner.clone(),
            raw_state: "state-e2e".into(),
            connected_account: connected_account(owner),
            token_bundle: token_bundle(),
            access_token: ACCESS_TOKEN.into(),
            refresh_token: Some(REFRESH_TOKEN.into()),
            now: 1_000,
        },
    )
    .unwrap_or_else(|err| panic!("[{store_label}] oauth complete: {err}"));
    assert_eq!(complete.account.id, "acct_e2e");
    assert_token_safe(&format!("[{store_label}] oauth complete"), &complete);

    // The token was persisted server-side, encrypted (never plaintext).
    let secret = store
        .token_secret_for_account("acct_e2e")
        .unwrap_or_else(|err| panic!("[{store_label}] token secret: {err}"));
    assert_ne!(secret.encrypted_access_token, ACCESS_TOKEN);

    // 4. Accounts list returns the connected account, token-safe.
    let accounts = SocialApi::accounts(store, &actor, &api_owner)
        .unwrap_or_else(|err| panic!("[{store_label}] accounts: {err}"));
    assert_eq!(accounts.len(), 1);
    assert_eq!(accounts[0].scopes, token_bundle().scopes);
    assert_token_safe(&format!("[{store_label}] accounts"), &accounts);

    // 5. Bind a campaign variant target to the account.
    let target = SocialApi::bind_target(
        store,
        &actor,
        BindTargetRequest {
            target_id: "target_e2e".into(),
            campaign_id: "campaign_e2e".into(),
            variant_id: "variant_e2e".into(),
            connected_account_id: "acct_e2e".into(),
            platform_fields: serde_json::json!({"privacy": "private", "title": "Launch clip"}),
            scheduled_for: 5_000,
            now: 1_100,
        },
    )
    .unwrap_or_else(|err| panic!("[{store_label}] bind target: {err}"));
    assert_eq!(target.validation_state, ValidationState::Pending);

    // 6. Validate the target -> Valid.
    let validated = SocialApi::validate_target(
        store,
        &registry,
        &actor,
        ValidateTargetRequest {
            target_id: "target_e2e".into(),
            now: 1_200,
        },
    )
    .unwrap_or_else(|err| panic!("[{store_label}] validate target: {err}"));
    assert_eq!(validated.validation_state, ValidationState::Valid);

    // 7. Schedule -> a Scheduled publish job with an audit event.
    let scheduled = SocialApi::schedule_target(
        store,
        &registry,
        &actor,
        ScheduleTargetRequest {
            target_id: "target_e2e".into(),
            job_id: "job_e2e".into(),
            artifact_ref: "render://artifact_e2e".into(),
            created_by: "user_e2e".into(),
            now: 1_300,
        },
    )
    .unwrap_or_else(|err| panic!("[{store_label}] schedule target: {err}"));
    assert_eq!(scheduled.status, PublishJobStatus::Scheduled);
    assert_eq!(scheduled.events.len(), 1);
    assert_token_safe(&format!("[{store_label}] scheduled job"), &scheduled);

    // 8. Worker claims the due job through the durable queue (-> Uploading).
    let claimed = PublishService::claim_due_jobs(store, 5_000, 10)
        .unwrap_or_else(|err| panic!("[{store_label}] claim due jobs: {err}"));
    assert_eq!(claimed.len(), 1, "[{store_label}] expected one claimed job");
    assert_eq!(claimed[0].id, "job_e2e");
    assert_eq!(claimed[0].status, PublishJobStatus::Uploading);

    // 9. Worker executes the claimed upload (-> Processing).
    let uploaded = SocialApi::execute_claimed_upload_job(
        store,
        &ProcessingUploadAdapter,
        ExecuteUploadRequest {
            job_id: "job_e2e".into(),
            title: "Montage E2E launch".into(),
            description: Some("End-to-end pipeline test".into()),
            tags: vec!["montage".into(), "e2e".into()],
            thumbnail_ref: Some("render://thumb_e2e".into()),
            artifact_ref: None,
            privacy: None,
            tiktok_interactions: Default::default(),
            now: 5_100,
        },
    )
    .unwrap_or_else(|err| panic!("[{store_label}] execute upload: {err}"));
    assert_eq!(uploaded.status, PublishJobStatus::Processing);
    assert_eq!(uploaded.provider_post_id.as_deref(), Some(PROVIDER_POST_ID));
    assert_token_safe(&format!("[{store_label}] uploaded job"), &uploaded);

    // 10. Worker polls provider status until Published.
    let published = SocialApi::poll_upload_status(store, &PublishedStatusAdapter, "job_e2e", 5_200)
        .unwrap_or_else(|err| panic!("[{store_label}] poll status: {err}"));
    assert_eq!(published.status, PublishJobStatus::Published);
    assert_eq!(
        published.provider_post_url.as_deref(),
        Some(PROVIDER_POST_URL)
    );
    assert_token_safe(&format!("[{store_label}] published job"), &published);

    // 11. The final job read carries the full audit trail, still token-safe.
    let final_job = SocialApi::publish_job(store, &actor, &api_owner, "job_e2e")
        .unwrap_or_else(|err| panic!("[{store_label}] publish job lookup: {err}"));
    assert_eq!(final_job, published);
    assert!(
        final_job.events.len() >= 3,
        "[{store_label}] expected scheduled + uploaded + status events, got {}",
        final_job.events.len()
    );
    assert_token_safe(&format!("[{store_label}] final job"), &final_job);
}

#[test]
fn full_pipeline_in_memory_store() {
    let mut store = InMemorySocialStore::default();
    run_full_pipeline(&mut store, "in-memory");
}
