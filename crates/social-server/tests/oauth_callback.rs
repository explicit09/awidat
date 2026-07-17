//! R16 — hermetic route tests for `GET /oauth/callback/{provider}`.
//!
//! Uses TikTok: both its token endpoint and profile endpoint derive from the
//! config-injected `tiktok_api_base`, so the whole exchange runs against a
//! local wiremock server. (Google's happy path is NOT hermetically testable —
//! see the skip note in `google_callback_not_testable_hermetically`.)

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::{TEST_AEAD_HEX, TEST_KEY_ID, now_secs, serve, state_with};
use montage_social::model::{OwnerRef, Provider};
use montage_social::oauth::{OAuthConnection, OAuthConnectionStatus};
use montage_social::store::SocialStore;
use montage_social_server::{ServerConfig, StoreHandle};
use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const ACCESS_TOKEN: &str = "tk-access-token-SECRET";
const REFRESH_TOKEN: &str = "tk-refresh-token-SECRET";

fn callback_config(mock_base: &str) -> ServerConfig {
    ServerConfig {
        token_key_id: TEST_KEY_ID.into(),
        token_key_hex: TEST_AEAD_HEX.into(),
        tiktok_client_key: "tt-client".into(),
        tiktok_client_secret: "tt-secret".into(),
        oauth_redirect_base: "https://social.example".into(),
        tiktok_api_base: mock_base.to_string(),
        ..ServerConfig::default()
    }
}

/// Stub the TikTok code exchange + profile lookup on `mock`.
async fn stub_tiktok_exchange(mock: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/v2/oauth/token/"))
        .and(body_string_contains("grant_type=authorization_code"))
        .and(body_string_contains("client_key=tt-client"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": ACCESS_TOKEN,
            "refresh_token": REFRESH_TOKEN,
            "expires_in": 86_400,
            "refresh_expires_in": 31_536_000,
            "open_id": "tt_user_1",
            "scope": "user.info.basic,video.publish"
        })))
        .mount(mock)
        .await;
    Mock::given(method("GET"))
        .and(path("/v2/user/info/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": {"user": {"open_id": "tt_user_1", "display_name": "Test Creator"}}
        })))
        .mount(mock)
        .await;
}

fn seed_started_connection(store: &StoreHandle, connection_id: &str, raw_state: &str, now: i64) {
    store
        .open()
        .save_oauth_connection(OAuthConnection::start(
            connection_id,
            OwnerRef::User("user_1".into()),
            Provider::TikTok,
            raw_state,
            vec!["video.publish".into()],
            "/return".into(),
            now,
            now + 900,
        ))
        .expect("seed oauth connection");
}

/// R16.6 / R26 — malformed or unknown `state` is rejected and nothing is
/// persisted. State is validated (shape, then hash + status/expiry against the
/// stored connection) BEFORE any provider round-trip, so the token endpoint is
/// stubbed here only to prove it is never actually hit by a rejected callback
/// — see `callback_rejects_invalid_state_before_any_provider_request` below
/// for the dedicated `.expect(0)` assertion.
#[tokio::test]
async fn callback_rejects_missing_or_invalid_state_and_persists_nothing() {
    let mock = MockServer::start().await;
    stub_tiktok_exchange(&mock).await;

    let store = StoreHandle::in_memory();
    let base = serve(state_with(callback_config(&mock.uri()), store.clone())).await;
    let client = reqwest::Client::new();

    // (a) state without the `<connection_id>~<random>` shape → malformed.
    let malformed = client
        .get(format!(
            "{base}/oauth/callback/tiktok?code=auth-code&state=no-tilde-in-here"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(malformed.status().as_u16(), 400);
    let body: serde_json::Value = malformed.json().await.unwrap();
    assert_eq!(body["error"], "malformed oauth state");

    // (b) well-shaped state pointing at a connection that was never started.
    let unknown = client
        .get(format!(
            "{base}/oauth/callback/tiktok?code=auth-code&state=ghost-conn~deadbeef"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(
        unknown.status().as_u16(),
        400,
        "unknown connection rejected"
    );

    // (c) known connection but a forged random suffix → state-hash mismatch.
    let now = now_secs();
    seed_started_connection(&store, "conn_real", "conn_real~genuine-entropy", now);
    let forged = client
        .get(format!(
            "{base}/oauth/callback/tiktok?code=auth-code&state=conn_real~forged-entropy"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(
        forged.status().as_u16(),
        400,
        "state hash mismatch rejected"
    );

    // Nothing persisted by any rejected callback.
    let opened = store.open();
    assert!(
        opened.connected_account("tiktok:tt_user_1").is_err(),
        "no account persisted"
    );
    assert!(
        opened.token_secret_for_account("tiktok:tt_user_1").is_err(),
        "no token secret persisted"
    );
    assert_eq!(
        opened.oauth_connection("conn_real").unwrap().status,
        OAuthConnectionStatus::Started,
        "seeded connection not consumed by a forged state"
    );
    // R26: state is validated before any provider round-trip, so none of the
    // three rejected callbacks above should have hit the token endpoint.
    assert!(
        mock.received_requests().await.unwrap().is_empty(),
        "state validation rejects before any provider request is made"
    );
}

/// R26 — dedicated proof that state validation happens strictly before the
/// provider token exchange: the token endpoint mock is configured to fail the
/// test (`.expect(0)`) if it receives ANY request, then a callback with a
/// state-hash mismatch is sent. The 400 response alone doesn't prove
/// ordering (a post-exchange rejection would also 400) — the zero-requests
/// assertion is what proves the reorder.
#[tokio::test]
async fn callback_rejects_invalid_state_before_any_provider_request() {
    let mock = MockServer::start().await;
    // No token/profile stubs mounted with a body — mount a bare 200 responder
    // but assert it is called exactly zero times. If state validation ever
    // regresses to run after the exchange, this mock still "succeeds" (200)
    // but `.expect(0)` fails the test on drop.
    Mock::given(method("POST"))
        .and(path("/v2/oauth/token/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": ACCESS_TOKEN,
            "refresh_token": REFRESH_TOKEN,
            "expires_in": 86_400,
            "refresh_expires_in": 31_536_000,
            "open_id": "tt_user_1",
            "scope": "user.info.basic,video.publish"
        })))
        .expect(0)
        .mount(&mock)
        .await;

    let now = now_secs();
    let store = StoreHandle::in_memory();
    seed_started_connection(&store, "conn_real", "conn_real~genuine-entropy", now);

    let base = serve(state_with(callback_config(&mock.uri()), store.clone())).await;
    let resp = reqwest::Client::new()
        .get(format!(
            "{base}/oauth/callback/tiktok?code=auth-code&state=conn_real~forged-entropy"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 400, "invalid state rejected");

    assert!(
        mock.received_requests().await.unwrap().is_empty(),
        "zero provider requests: state is validated before the token exchange"
    );
    // `mock` is dropped at end of scope, which also asserts `.expect(0)` held.
}

/// R16.7 — happy path: the code is exchanged at the wiremock token endpoint,
/// the account + encrypted token secret are persisted, the connection is
/// completed, and the HTML response leaks no token material.
#[tokio::test]
async fn callback_happy_path_persists_account_and_leaks_no_tokens() {
    let mock = MockServer::start().await;
    stub_tiktok_exchange(&mock).await;

    let now = now_secs();
    let store = StoreHandle::in_memory();
    let raw_state = "conn_1~genuine-entropy";
    seed_started_connection(&store, "conn_1", raw_state, now);

    let base = serve(state_with(callback_config(&mock.uri()), store.clone())).await;

    let resp = reqwest::Client::new()
        .get(format!(
            "{base}/oauth/callback/tiktok?code=auth-code&state={raw_state}"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let html = resp.text().await.unwrap();
    assert!(
        html.contains("tiktok:tt_user_1"),
        "success page names the connected account"
    );
    // The critical redaction assertion: no token material in the response body.
    assert!(
        !html.contains(ACCESS_TOKEN) && !html.contains("tk-access"),
        "access token must not appear in the response"
    );
    assert!(
        !html.contains(REFRESH_TOKEN) && !html.contains("tk-refresh"),
        "refresh token must not appear in the response"
    );

    // Persistence: account, encrypted secret, and completed connection.
    let opened = store.open();
    let account = opened.connected_account("tiktok:tt_user_1").unwrap();
    assert_eq!(account.provider, Provider::TikTok);
    assert_eq!(account.provider_account_id, "tt_user_1");
    assert_eq!(account.display_name, "Test Creator");
    assert_eq!(account.owner, OwnerRef::User("user_1".into()));
    assert!(
        account.scopes.contains(&"video.publish".to_string()),
        "granted scopes persisted from the token response"
    );

    let secret = opened.token_secret_for_account("tiktok:tt_user_1").unwrap();
    assert_eq!(
        secret
            .decrypt_access_token(&common::test_aead_key())
            .unwrap(),
        ACCESS_TOKEN,
        "secret decrypts with the configured AEAD key"
    );

    assert_eq!(
        opened.oauth_connection("conn_1").unwrap().status,
        OAuthConnectionStatus::Completed
    );

    // Replay defense: the same callback again must now be rejected.
    let replay = reqwest::Client::new()
        .get(format!(
            "{base}/oauth/callback/tiktok?code=auth-code&state={raw_state}"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(
        replay.status().as_u16(),
        400,
        "completed connection cannot be replayed"
    );
}
