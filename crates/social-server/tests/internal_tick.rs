//! R8 — hermetic route tests for `POST /internal/tick`, the highest-blast-radius
//! handler (claims due jobs and fires live provider uploads).
//!
//! All state is config-injected (no env vars), the store is in-memory, and the
//! YouTube API is a wiremock server, so the suite is fully parallel-safe.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::{now_secs, seed_due_youtube_job, serve, state_with, test_aead_key, tick_config};
use montage_social::model::{Provider, PublishJobEventType, PublishJobStatus};
use montage_social::store::SocialStore;
use montage_social::token::TokenSecret;
use montage_social_server::StoreHandle;
use wiremock::matchers::{header_exists, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// R8.1 — wrong/missing bearer → 401 and the store is untouched: the due job
/// stays `Scheduled` with zero attempts and no provider request is made.
#[tokio::test]
async fn tick_rejects_bad_bearer_and_leaves_store_untouched() {
    let mock = MockServer::start().await;
    let now = now_secs();
    let store = StoreHandle::in_memory();
    seed_due_youtube_job(&store, "job_due", "file:///tmp/none.mp4", now);

    let config = tick_config(&mock.uri(), true, "/tmp");
    let base = serve(state_with(config, store.clone())).await;
    let client = reqwest::Client::new();

    let missing = client
        .post(format!("{base}/internal/tick"))
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status().as_u16(), 401, "missing bearer");

    let wrong = client
        .post(format!("{base}/internal/tick"))
        .bearer_auth("not-the-secret")
        .send()
        .await
        .unwrap();
    assert_eq!(wrong.status().as_u16(), 401, "wrong bearer");

    let job = store.open().publish_job("job_due").unwrap();
    assert_eq!(job.status, PublishJobStatus::Scheduled, "job not claimed");
    assert_eq!(job.attempt_count, 0, "no claim attempt recorded");
    assert!(
        mock.received_requests().await.unwrap().is_empty(),
        "no provider traffic on auth failure"
    );
}

/// R8.2 — firing disabled (config seam, not env) → authed tick is a noop and
/// nothing is claimed.
#[tokio::test]
async fn tick_with_firing_disabled_is_noop_and_claims_nothing() {
    let mock = MockServer::start().await;
    let now = now_secs();
    let store = StoreHandle::in_memory();
    seed_due_youtube_job(&store, "job_due", "file:///tmp/none.mp4", now);

    let config = tick_config(&mock.uri(), false, "/tmp");
    let base = serve(state_with(config, store.clone())).await;

    let resp = reqwest::Client::new()
        .post(format!("{base}/internal/tick"))
        .bearer_auth("tick-secret")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "noop");
    assert_eq!(body["reason"], "firing disabled");

    let job = store.open().publish_job("job_due").unwrap();
    assert_eq!(job.status, PublishJobStatus::Scheduled);
    assert_eq!(job.attempt_count, 0);
    assert!(mock.received_requests().await.unwrap().is_empty());
}

/// R8.3 — happy path: a due YouTube job is claimed, the resumable upload hits
/// the wiremock-stubbed YouTube base (initiate + chunk PUT), and the job lands
/// in the status the code actually writes for uploadStatus="uploaded":
/// `Processing`, with the provider post id recorded, an `Uploaded` event
/// appended, and the daily quota incremented.
#[tokio::test]
async fn tick_happy_path_fires_due_youtube_job_through_mock_provider() {
    let mock = MockServer::start().await;
    let now = now_secs();

    // Real artifact bytes under the confined artifact base dir.
    let artifacts = tempfile::tempdir().expect("artifact dir");
    let artifact_path = artifacts.path().join("render.mp4");
    std::fs::write(&artifact_path, b"fake-mp4-bytes").expect("write artifact");
    let artifact_ref = format!("file://{}", artifact_path.to_string_lossy());

    let store = StoreHandle::in_memory();
    seed_due_youtube_job(&store, "job_due", &artifact_ref, now);

    // Resumable upload protocol: initiate POST returns the session Location;
    // the chunk PUT completes with the video resource.
    Mock::given(method("POST"))
        .and(path("/upload/youtube/v3/videos"))
        .and(header_exists("authorization"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Location", format!("{}/upload-session/1", mock.uri())),
        )
        .expect(1)
        .mount(&mock)
        .await;
    Mock::given(method("PUT"))
        .and(path("/upload-session/1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "yt_vid_1",
            "status": {"uploadStatus": "uploaded"}
        })))
        .expect(1)
        .mount(&mock)
        .await;

    let config = tick_config(&mock.uri(), true, &artifacts.path().to_string_lossy());
    let base = serve(state_with(config, store.clone())).await;

    let resp = reqwest::Client::new()
        .post(format!("{base}/internal/tick"))
        .bearer_auth("tick-secret")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "ok");
    assert_eq!(body["claimed"], 1);

    let opened = store.open();
    let job = opened.publish_job("job_due").unwrap();
    assert_eq!(
        job.status,
        PublishJobStatus::Processing,
        "uploadStatus=uploaded maps to Processing (transcode pending)"
    );
    assert_eq!(job.provider_post_id.as_deref(), Some("yt_vid_1"));
    assert_eq!(job.attempt_count, 1);

    let events = opened.publish_job_events("job_due").unwrap();
    assert!(
        events
            .iter()
            .any(|e| e.event_type == PublishJobEventType::Uploaded),
        "Uploaded event recorded, got: {:?}",
        events.iter().map(|e| &e.event_type).collect::<Vec<_>>()
    );
    assert_eq!(
        opened.youtube_upload_quota_today(now_secs()).unwrap(),
        1,
        "successful fire consumes one quota unit"
    );
    // wiremock `.expect(1)` on both mocks verifies the exact call pattern on drop.
}

/// R8.4 — provider 5xx: the ACTUAL contract is bounded-backoff requeue, not
/// terminal failure. The job returns to `Scheduled` with `scheduled_for`
/// pushed ~60s out (base backoff, attempt 1 of 5), attempt_count stays at 1,
/// and a `RetryQueued` event is appended. NOTE (actual behavior): the handler
/// treats a requeued failure as a non-error and still increments the YouTube
/// quota — asserted here as-is.
#[tokio::test]
async fn tick_requeues_job_with_backoff_on_provider_5xx() {
    let mock = MockServer::start().await;
    let now = now_secs();

    let artifacts = tempfile::tempdir().expect("artifact dir");
    let artifact_path = artifacts.path().join("render.mp4");
    std::fs::write(&artifact_path, b"fake-mp4-bytes").expect("write artifact");
    let artifact_ref = format!("file://{}", artifact_path.to_string_lossy());

    let store = StoreHandle::in_memory();
    seed_due_youtube_job(&store, "job_due", &artifact_ref, now);

    Mock::given(method("POST"))
        .and(path("/upload/youtube/v3/videos"))
        .respond_with(ResponseTemplate::new(500).set_body_string("backend exploded"))
        .expect(1)
        .mount(&mock)
        .await;

    let config = tick_config(&mock.uri(), true, &artifacts.path().to_string_lossy());
    let base = serve(state_with(config, store.clone())).await;

    let resp = reqwest::Client::new()
        .post(format!("{base}/internal/tick"))
        .bearer_auth("tick-secret")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);

    let opened = store.open();
    let job = opened.publish_job("job_due").unwrap();
    assert_ne!(
        job.status,
        PublishJobStatus::Published,
        "5xx must never mark success"
    );
    assert_eq!(
        job.status,
        PublishJobStatus::Scheduled,
        "retryable failure requeues (attempt 1 of 5)"
    );
    assert_eq!(job.attempt_count, 1, "claim's attempt bump is preserved");
    assert!(
        job.scheduled_for >= now + 50 && job.scheduled_for <= now + 90,
        "rescheduled ~60s out (base backoff), got {} vs now {}",
        job.scheduled_for,
        now
    );

    let events = opened.publish_job_events("job_due").unwrap();
    assert!(
        events
            .iter()
            .any(|e| e.event_type == PublishJobEventType::RetryQueued),
        "RetryQueued event recorded"
    );
    // Documented actual behavior: the requeue path returns Ok to the handler,
    // which counts it as a fire and burns a quota unit.
    assert_eq!(opened.youtube_upload_quota_today(now_secs()).unwrap(), 1);
}

/// R8.5 — YouTube daily quota exhausted: the due job is claimed by the sweep
/// but immediately restored to `Scheduled` with its attempt refunded, and the
/// provider is never contacted.
#[tokio::test]
async fn tick_restores_youtube_job_when_daily_quota_exhausted() {
    let mock = MockServer::start().await;
    let now = now_secs();
    let store = StoreHandle::in_memory();
    seed_due_youtube_job(&store, "job_due", "file:///tmp/unused.mp4", now);
    {
        let mut opened = store.open();
        for _ in 0..100 {
            opened.increment_youtube_quota(now).expect("seed quota");
        }
    }

    let config = tick_config(&mock.uri(), true, "/tmp");
    let base = serve(state_with(config, store.clone())).await;

    let resp = reqwest::Client::new()
        .post(format!("{base}/internal/tick"))
        .bearer_auth("tick-secret")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    // The sweep claims before the per-provider quota gate; the count reflects
    // the claim, the job itself is restored below.
    assert_eq!(body["claimed"], 1);

    let opened = store.open();
    let job = opened.publish_job("job_due").unwrap();
    assert_eq!(
        job.status,
        PublishJobStatus::Scheduled,
        "quota-blocked job is restored, not fired"
    );
    assert_eq!(job.attempt_count, 0, "claim attempt refunded");
    assert_eq!(
        opened.youtube_upload_quota_today(now_secs()).unwrap(),
        100,
        "no extra quota consumed"
    );
    assert!(
        mock.received_requests().await.unwrap().is_empty(),
        "provider never contacted past the quota gate"
    );
    // Guard the fixture: the job really is a YouTube job.
    assert_eq!(job.provider, Provider::YouTube);
}

/// Fixture sanity: the seeded token secret round-trips through the same AEAD
/// key the server config carries (guards the seed helper itself).
#[test]
fn seeded_token_secret_decrypts_with_config_key() {
    let key = test_aead_key();
    let secret = TokenSecret::encrypt("acct_1", "live-bearer-token", None, &key, 0).unwrap();
    assert_eq!(
        secret.decrypt_access_token(&key).unwrap(),
        "live-bearer-token"
    );
}
