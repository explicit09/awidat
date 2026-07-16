//! Shared fixtures for the hermetic route-level test suite.
//!
//! Everything here is config-injected — no env vars, no Postgres, no network
//! beyond local wiremock servers.

#![allow(dead_code)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use montage_social::model::{
    AccountEligibility, AccountKind, ConnectedAccount, ConnectedAccountStatus, OwnerRef, Provider,
    ProviderCapabilities, PublishJob,
};
use montage_social::provider::ProviderRegistry;
use montage_social::store::SocialStore;
use montage_social::token::{Aead256Key, TokenSecret};
use montage_social_server::{AppState, ServerConfig, StoreHandle, build_router};

/// 32 bytes of 0x11, hex-encoded — a valid AEAD key both the server config and
/// the test's own `TokenSecret::encrypt` can share.
pub const TEST_AEAD_HEX: &str = "1111111111111111111111111111111111111111111111111111111111111111";
pub const TEST_KEY_ID: &str = "k1";

pub fn test_aead_key() -> Aead256Key {
    Aead256Key::from_hex(TEST_KEY_ID, TEST_AEAD_HEX).expect("test AEAD key")
}

pub fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_secs() as i64
}

/// Spawn the real production router over `state` on an ephemeral port.
pub async fn serve(state: Arc<AppState>) -> String {
    let app = build_router(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral port");
    let addr = listener.local_addr().expect("local addr");
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });
    format!("http://{addr}")
}

pub fn state_with(config: ServerConfig, store: StoreHandle) -> Arc<AppState> {
    Arc::new(AppState {
        store,
        registry: ProviderRegistry::default_multi_platform(),
        config,
    })
}

/// A connected, eligible, upload-capable account for `provider`.
pub fn publishable_account(id: &str, provider: Provider, now: i64) -> ConnectedAccount {
    ConnectedAccount {
        id: id.into(),
        owner: OwnerRef::User("user_1".into()),
        provider,
        provider_account_id: format!("provider-{id}"),
        display_name: "Test Account".into(),
        handle: None,
        avatar_url: None,
        account_kind: AccountKind::Channel,
        status: ConnectedAccountStatus::Connected,
        scopes: vec!["upload".into()],
        capabilities: ProviderCapabilities {
            native_scheduling: false,
            queue_scheduling: true,
            upload_video: true,
            upload_thumbnail: false,
            public_posting: false,
            requires_user_consent: false,
        },
        eligibility: AccountEligibility::eligible(),
        last_verified_at: Some(now),
        created_at: now,
        updated_at: now,
    }
}

/// A scheduled (due or future) publish job for `account_id`.
pub fn scheduled_job(
    job_id: &str,
    account_id: &str,
    provider: Provider,
    artifact_ref: &str,
    scheduled_for: i64,
    now: i64,
) -> PublishJob {
    PublishJob::new(
        job_id,
        "campaign_1",
        "variant_1",
        account_id,
        provider,
        artifact_ref,
        scheduled_for,
        "user_1",
    )
    .schedule(now)
}

/// Seed the shared in-memory store with everything the tick handler needs to
/// fire a due YouTube job: connected account, non-expiring encrypted token,
/// and the scheduled job pointing at `artifact_ref`.
pub fn seed_due_youtube_job(store: &StoreHandle, job_id: &str, artifact_ref: &str, now: i64) {
    let mut opened = store.open();
    opened
        .save_connected_account(publishable_account("acct_1", Provider::YouTube, now))
        .expect("seed account");
    let mut secret = TokenSecret::encrypt(
        "acct_1",
        "live-bearer-token",
        Some("live-refresh-token"),
        &test_aead_key(),
        now,
    )
    .expect("encrypt token secret");
    // Far-future expiry so the fire-time refresher is never driven.
    secret.access_token_expires_at = Some(now + 1_000_000);
    opened.save_token_secret(secret).expect("seed token");
    opened
        .save_publish_job(scheduled_job(
            job_id,
            "acct_1",
            Provider::YouTube,
            artifact_ref,
            now - 30,
            now - 60,
        ))
        .expect("seed job");
}

/// Config for firing-enabled tick tests: bearer + AEAD key + Google OAuth creds
/// (required for the YouTube refresher gate) + mock provider bases.
pub fn tick_config(mock_base: &str, firing_enabled: bool, artifact_base_dir: &str) -> ServerConfig {
    ServerConfig {
        service_shared_secret: "tick-secret".into(),
        social_firing_enabled: firing_enabled,
        token_key_id: TEST_KEY_ID.into(),
        token_key_hex: TEST_AEAD_HEX.into(),
        google_client_id: "google-client".into(),
        google_client_secret: "google-secret".into(),
        artifact_base_dir: artifact_base_dir.into(),
        youtube_upload_base: format!("{mock_base}/upload/youtube/v3/videos"),
        youtube_videos_base: format!("{mock_base}/youtube/v3/videos"),
        ..ServerConfig::default()
    }
}
