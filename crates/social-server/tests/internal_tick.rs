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

/// R8.4 / R28 — provider 5xx: the ACTUAL contract is bounded-backoff requeue,
/// not terminal failure. The job returns to `Scheduled` with `scheduled_for`
/// pushed ~60s out (base backoff, attempt 1 of 5), attempt_count stays at 1,
/// and a `RetryQueued` event is appended. NOTE (R28, now-intentional
/// contract): quota counts actual upload attempts that reached the provider,
/// not "did the job end up Published." A 5xx means the request WAS dispatched
/// to the provider (see `TrackedUploadAdapter` in lib.rs, which flags
/// `dispatched()` from inside `adapter.upload()` itself) — the failure just
/// happened on the far side of that round-trip — so burning a quota unit here
/// is correct, unlike a purely local pre-provider short-circuit (e.g. R27's
/// unconfigured-refresher case), which must NOT burn one.
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

/// R27 / R28 — a due YouTube job is claimed by the sweep, but Google OAuth
/// creds are unconfigured (empty `google_client_id`/`google_client_secret`),
/// so `token_refresher_for_provider` returns `None`. Before the fix this hit
/// a bare `continue` after the claim, stranding the job in `Uploading`
/// forever with no requeue and no event. The fix must:
///   - still answer the tick request OK (not 5xx the whole sweep);
///   - restore the job to `Scheduled` (not leave it `Uploading`), with the
///     claim's attempt refunded, mirroring the quota-exhausted restore path;
///   - append an event recording why (so the misconfiguration is visible in
///     the job's audit trail, not just a log line);
///   - never contact the provider;
///   - (R28) never increment the YouTube daily quota, since the upload was
///     never actually dispatched to the provider.
#[tokio::test]
async fn tick_restores_due_youtube_job_when_refresher_unconfigured() {
    let mock = MockServer::start().await;
    let now = now_secs();
    let store = StoreHandle::in_memory();
    seed_due_youtube_job(&store, "job_due", "file:///tmp/unused.mp4", now);

    let mut config = tick_config(&mock.uri(), true, "/tmp");
    config.google_client_id = String::new();
    config.google_client_secret = String::new();
    let base = serve(state_with(config, store.clone())).await;

    let resp = reqwest::Client::new()
        .post(format!("{base}/internal/tick"))
        .bearer_auth("tick-secret")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200, "tick responds OK, not 5xx");
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "ok");
    assert_eq!(body["claimed"], 1, "the sweep did claim the due job");

    let opened = store.open();
    let job = opened.publish_job("job_due").unwrap();
    assert_eq!(
        job.status,
        PublishJobStatus::Scheduled,
        "job is restored, not stranded in Uploading"
    );
    assert_eq!(job.attempt_count, 0, "claim attempt refunded");

    let events = opened.publish_job_events("job_due").unwrap();
    assert!(
        events
            .iter()
            .any(|e| e.event_type == PublishJobEventType::RetryQueued
                && e.message.to_lowercase().contains("refresh")),
        "an event records the refresher-unconfigured condition, got: {:?}",
        events
            .iter()
            .map(|e| (&e.event_type, &e.message))
            .collect::<Vec<_>>()
    );

    assert!(
        mock.received_requests().await.unwrap().is_empty(),
        "provider never contacted: the refresher gate fires before any upload dispatch"
    );
    assert_eq!(
        opened.youtube_upload_quota_today(now_secs()).unwrap(),
        0,
        "R28: no quota unit burned — the upload never reached the provider"
    );
}

/// R28 (direct-fire path) — `POST /social/jobs/{id}/fire` reaches
/// `fire_due_publish_job`, which has its own quota-increment site separate
/// from the /internal/tick sweep. Same contract: quota counts only upload
/// attempts actually dispatched to the provider.
///   - Local failure (Google OAuth creds unconfigured → refresher gate errors
///     before any provider contact): fire fails with 422, provider untouched,
///     quota NOT incremented — and (R27 analog) the claimed job is restored
///     to `Scheduled` with the attempt refunded plus a RetryQueued event,
///     not stranded in `Uploading`.
///   - Provider-dispatched attempt (upload initiate answered with 5xx → job
///     requeued with backoff): the request DID reach the provider, so one
///     quota unit IS burned even though the upload failed.
#[tokio::test]
async fn direct_fire_counts_quota_only_for_provider_dispatched_attempts() {
    let client = reqwest::Client::new();

    // ── Phase 1: local failure (refresher unconfigured) → no quota burned. ──
    let mock_a = MockServer::start().await;
    let now = now_secs();
    let store_a = StoreHandle::in_memory();
    seed_due_youtube_job(&store_a, "job_local", "file:///tmp/unused.mp4", now);

    let mut config_a = tick_config(&mock_a.uri(), true, "/tmp");
    config_a.google_client_id = String::new();
    config_a.google_client_secret = String::new();
    // Desktop dev bearer mapping to the seeded account owner `user_1`.
    config_a.desktop_auth_token = "desktop-dev".into();
    config_a.desktop_user_id = "user_1".into();
    let base_a = serve(state_with(config_a, store_a.clone())).await;

    let resp = client
        .post(format!("{base_a}/social/jobs/job_local/fire"))
        .bearer_auth("desktop-dev")
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status().as_u16(),
        422,
        "local pre-provider failure surfaces as unprocessable"
    );
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(
        body["error"]
            .as_str()
            .unwrap_or_default()
            .contains("youtube OAuth not configured"),
        "error names the misconfiguration, got: {body}"
    );
    assert!(
        mock_a.received_requests().await.unwrap().is_empty(),
        "provider never contacted on the local-failure path"
    );
    let opened_a = store_a.open();
    assert_eq!(
        opened_a.youtube_upload_quota_today(now_secs()).unwrap(),
        0,
        "R28: no quota unit burned — the upload never reached the provider"
    );
    // R27 (direct-fire analog): despite the 422, the claimed job must be
    // restored — not stranded in `Uploading` — with the claim's attempt
    // refunded and an event naming the missing-refresher cause.
    let job_local = opened_a.publish_job("job_local").unwrap();
    assert_eq!(
        job_local.status,
        PublishJobStatus::Scheduled,
        "job restored to Scheduled after the failed fire, not stuck Uploading"
    );
    assert_eq!(job_local.attempt_count, 0, "claim attempt refunded");
    let events_local = opened_a.publish_job_events("job_local").unwrap();
    assert!(
        events_local
            .iter()
            .any(|e| e.event_type == PublishJobEventType::RetryQueued
                && e.message.to_lowercase().contains("refresh")),
        "an event records the refresher-unconfigured condition, got: {:?}",
        events_local
            .iter()
            .map(|e| (&e.event_type, &e.message))
            .collect::<Vec<_>>()
    );
    drop(opened_a);

    // ── Phase 2: provider-dispatched 5xx → exactly one quota unit burned. ──
    let mock_b = MockServer::start().await;
    let artifacts = tempfile::tempdir().expect("artifact dir");
    let artifact_path = artifacts.path().join("render.mp4");
    std::fs::write(&artifact_path, b"fake-mp4-bytes").expect("write artifact");
    let artifact_ref = format!("file://{}", artifact_path.to_string_lossy());

    let now = now_secs();
    let store_b = StoreHandle::in_memory();
    seed_due_youtube_job(&store_b, "job_dispatched", &artifact_ref, now);

    Mock::given(method("POST"))
        .and(path("/upload/youtube/v3/videos"))
        .respond_with(ResponseTemplate::new(500).set_body_string("backend exploded"))
        .expect(1)
        .mount(&mock_b)
        .await;

    let mut config_b = tick_config(&mock_b.uri(), true, &artifacts.path().to_string_lossy());
    config_b.desktop_auth_token = "desktop-dev".into();
    config_b.desktop_user_id = "user_1".into();
    let base_b = serve(state_with(config_b, store_b.clone())).await;

    let resp = client
        .post(format!("{base_b}/social/jobs/job_dispatched/fire"))
        .bearer_auth("desktop-dev")
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status().as_u16(),
        200,
        "5xx requeue is a non-error to the fire route (job requeued, not failed)"
    );

    let opened = store_b.open();
    let job = opened.publish_job("job_dispatched").unwrap();
    assert_eq!(
        job.status,
        PublishJobStatus::Scheduled,
        "5xx requeues with backoff on the direct-fire path too"
    );
    assert_eq!(
        opened.youtube_upload_quota_today(now_secs()).unwrap(),
        1,
        "R28: the attempt reached the provider, so one quota unit is burned"
    );
    // `.expect(1)` on the mock verifies exactly one provider dispatch on drop.
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
