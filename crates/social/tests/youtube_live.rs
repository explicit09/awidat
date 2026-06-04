//! Integration tests for the live YouTube resumable-upload + status clients.
//!
//! These run only under the `youtube-live` feature (which compiles the
//! `youtube_upload::live` module). They exercise the real reqwest HTTP paths
//! against a `wiremock` mock server, covering:
//!
//!   * a clean single-chunk upload (200 complete),
//!   * a resumable interruption (308 Resume Incomplete -> offset resume -> 200),
//!   * the missing-scope error mapping (401 on initiate),
//!   * the not-eligible error mapping (403 on initiate),
//!   * a token-resolution failure (resolver returns NotFound),
//!   * a status poll that reports processing, processed, and failed,
//!   * a status poll server error.
//!
//! The sync `upload_video` / `poll_status` trait methods bridge to async reqwest
//! via `Handle::current().block_on(...)`. Calling them directly from an async
//! test would panic ("Cannot block the current thread from within a runtime"),
//! so every call is wrapped in `spawn_blocking`, mirroring how the server worker
//! invokes them from inside `tokio::task::spawn_blocking`.
//!
//! Per the workspace lint policy (`unwrap_used`/`expect_used` are denied even in
//! tests), failures use `unwrap_or_else(|e| panic!(...))` instead of
//! `.unwrap()`/`.expect()`.

#![cfg(feature = "youtube-live")]

use awidat_social::youtube_upload::live::{LiveYouTubeStatusClient, LiveYouTubeUploadClient};
use awidat_social::youtube_upload::{
    AccessTokenResolver, AccessTokenResolverError, ArtifactBody, ArtifactSource,
    ArtifactSourceError, YouTubeClientConfig, YouTubeProcessingState, YouTubeStatusClient,
    YouTubeStatusClientError, YouTubeStatusRequest, YouTubeUploadClient, YouTubeUploadClientError,
    YouTubeUploadRequest, YouTubeUploadResponse,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, Respond, ResponseTemplate};

// ── Test seam impls ───────────────────────────────────────────────────────────

/// Token resolver that always returns a fixed bearer string.
struct FixedToken(&'static str);
impl AccessTokenResolver for FixedToken {
    fn bearer_for(&self, _r: &str) -> Result<String, AccessTokenResolverError> {
        Ok(self.0.to_string())
    }
}

/// Token resolver that always fails — exercises the resolver-error path.
struct FailingToken;
impl AccessTokenResolver for FailingToken {
    fn bearer_for(&self, _r: &str) -> Result<String, AccessTokenResolverError> {
        Err(AccessTokenResolverError::NotFound("no token".into()))
    }
}

/// Artifact source serving fixed bytes from memory.
struct InMemoryArtifact(Vec<u8>);
impl ArtifactSource for InMemoryArtifact {
    fn open(&self, _r: &str) -> Result<ArtifactBody, ArtifactSourceError> {
        Ok(ArtifactBody {
            total_bytes: self.0.len() as u64,
            data: self.0.clone(),
        })
    }
}

fn upload_request() -> YouTubeUploadRequest {
    YouTubeUploadRequest {
        artifact_ref: "file:///tmp/render.mp4".into(),
        thumbnail_ref: None,
        title: "Launch clip".into(),
        description: Some("Description".into()),
        tags: vec!["awidat".into()],
        privacy: "public".into(),
        scheduled_for: None,
        access_token_ref: "token_secret:acct-1".into(),
    }
}

fn status_request(id: &str) -> YouTubeStatusRequest {
    YouTubeStatusRequest {
        provider_post_id: id.into(),
        access_token_ref: "token_secret:acct-1".into(),
    }
}

/// Build an upload client whose initiate POST is redirected at `upload_base`.
fn upload_client(
    upload_base: String,
    chunk_size: usize,
    bytes: Vec<u8>,
) -> LiveYouTubeUploadClient<FixedToken, InMemoryArtifact> {
    LiveYouTubeUploadClient::new(
        FixedToken("ya29.test-token"),
        InMemoryArtifact(bytes),
        YouTubeClientConfig {
            force_private: true,
            chunk_size,
            upload_base,
        },
    )
}

/// Run a sync upload on the blocking pool (the live client blocks on its own
/// runtime handle internally; doing so on the test's async worker would panic).
async fn run_upload(
    client: LiveYouTubeUploadClient<FixedToken, InMemoryArtifact>,
) -> Result<YouTubeUploadResponse, YouTubeUploadClientError> {
    tokio::task::spawn_blocking(move || client.upload_video(&upload_request()))
        .await
        .unwrap_or_else(|e| panic!("spawn_blocking join failed: {e}"))
}

/// Run a sync status poll on the blocking pool.
async fn run_status(
    client: LiveYouTubeStatusClient<FixedToken>,
    id: &str,
) -> Result<awidat_social::youtube_upload::YouTubeStatusResponse, YouTubeStatusClientError> {
    let id = id.to_string();
    tokio::task::spawn_blocking(move || client.poll_status(&status_request(&id)))
        .await
        .unwrap_or_else(|e| panic!("spawn_blocking join failed: {e}"))
}

// ── Upload: clean single-chunk completion ─────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn upload_single_chunk_completes() {
    let server = MockServer::start().await;
    let session_uri = format!("{}/session/abc", server.uri());

    // Initiate POST -> 200 with Location header pointing at the session URI.
    Mock::given(method("POST"))
        .and(path("/upload"))
        .and(query_param("uploadType", "resumable"))
        .and(header("authorization", "Bearer ya29.test-token"))
        .respond_with(ResponseTemplate::new(200).insert_header("Location", session_uri.as_str()))
        .mount(&server)
        .await;

    // The single chunk PUT -> 200 with the completed video resource.
    Mock::given(method("PUT"))
        .and(path("/session/abc"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "vid-123",
            "status": { "uploadStatus": "uploaded" }
        })))
        .mount(&server)
        .await;

    let client = upload_client(
        format!("{}/upload", server.uri()),
        8 * 1024 * 1024,
        vec![1u8; 1024],
    );
    let resp = run_upload(client)
        .await
        .unwrap_or_else(|e| panic!("upload should succeed: {e:?}"));

    assert_eq!(resp.video_id, "vid-123");
    assert!(
        resp.processing,
        "uploadStatus=uploaded should mark processing"
    );
}

// ── Upload: 308 interruption then resume ──────────────────────────────────────

/// Responder that returns 308 (Resume Incomplete) for the first PUT and 201
/// (complete) for the second, confirming the client honours the `Range` header
/// to advance its offset and finishes the upload.
struct ResumeResponder {
    calls: Arc<AtomicUsize>,
    first_confirmed_byte: usize,
}

impl Respond for ResumeResponder {
    fn respond(&self, _req: &wiremock::Request) -> ResponseTemplate {
        let n = self.calls.fetch_add(1, Ordering::SeqCst);
        if n == 0 {
            // First chunk accepted partially; confirm bytes 0..=first_confirmed_byte.
            ResponseTemplate::new(308).insert_header(
                "Range",
                format!("bytes=0-{}", self.first_confirmed_byte).as_str(),
            )
        } else {
            ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "id": "vid-resumed",
                "status": { "uploadStatus": "processing" }
            }))
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn upload_resumes_after_308() {
    let server = MockServer::start().await;
    let session_uri = format!("{}/session/resume", server.uri());

    Mock::given(method("POST"))
        .and(path("/upload"))
        .respond_with(ResponseTemplate::new(200).insert_header("Location", session_uri.as_str()))
        .mount(&server)
        .await;

    let calls = Arc::new(AtomicUsize::new(0));
    // Total 200 bytes, 100-byte chunks -> first PUT covers bytes 0-99, server
    // confirms through byte 99, second PUT covers 100-199 and completes.
    Mock::given(method("PUT"))
        .and(path("/session/resume"))
        .respond_with(ResumeResponder {
            calls: calls.clone(),
            first_confirmed_byte: 99,
        })
        .expect(2)
        .mount(&server)
        .await;

    let client = upload_client(format!("{}/upload", server.uri()), 100, vec![7u8; 200]);
    let resp = run_upload(client)
        .await
        .unwrap_or_else(|e| panic!("resumed upload should succeed: {e:?}"));

    assert_eq!(resp.video_id, "vid-resumed");
    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "should take exactly two PUTs"
    );
}

// ── Upload: error mappings ────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn upload_401_maps_to_missing_scope() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/upload"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;

    let client = upload_client(
        format!("{}/upload", server.uri()),
        8 * 1024 * 1024,
        vec![1u8; 8],
    );
    let err = run_upload(client)
        .await
        .err()
        .unwrap_or_else(|| panic!("401 should error"));

    assert_eq!(err, YouTubeUploadClientError::MissingScope);
}

#[tokio::test(flavor = "multi_thread")]
async fn upload_403_other_maps_to_not_eligible() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/upload"))
        .respond_with(ResponseTemplate::new(403).set_body_string("youtubeSignupRequired"))
        .mount(&server)
        .await;

    let client = upload_client(
        format!("{}/upload", server.uri()),
        8 * 1024 * 1024,
        vec![1u8; 8],
    );
    let err = run_upload(client)
        .await
        .err()
        .unwrap_or_else(|| panic!("403 should error"));

    assert_eq!(err, YouTubeUploadClientError::AccountNotEligible);
}

#[tokio::test(flavor = "multi_thread")]
async fn upload_403_insufficient_permissions_maps_to_missing_scope() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/upload"))
        .respond_with(ResponseTemplate::new(403).set_body_string("insufficientPermissions"))
        .mount(&server)
        .await;

    let client = upload_client(
        format!("{}/upload", server.uri()),
        8 * 1024 * 1024,
        vec![1u8; 8],
    );
    let err = run_upload(client)
        .await
        .err()
        .unwrap_or_else(|| panic!("403 insufficientPermissions should error"));

    assert_eq!(err, YouTubeUploadClientError::MissingScope);
}

#[tokio::test(flavor = "multi_thread")]
async fn upload_token_resolution_failure_is_network_error() {
    // No HTTP should even be attempted; resolver fails first.
    let client = LiveYouTubeUploadClient::new(
        FailingToken,
        InMemoryArtifact(vec![1u8; 8]),
        YouTubeClientConfig {
            force_private: true,
            chunk_size: 8 * 1024 * 1024,
            upload_base: "http://127.0.0.1:1/upload".into(),
        },
    );
    let err = tokio::task::spawn_blocking(move || client.upload_video(&upload_request()))
        .await
        .unwrap_or_else(|e| panic!("spawn_blocking join failed: {e}"))
        .err()
        .unwrap_or_else(|| panic!("failing resolver should error"));

    match err {
        YouTubeUploadClientError::NetworkOrServer(msg) => {
            assert!(
                msg.contains("no token"),
                "expected resolver error, got: {msg}"
            );
        }
        other => panic!("expected NetworkOrServer, got {other:?}"),
    }
}

// ── Status polling ────────────────────────────────────────────────────────────

fn status_client(server: &MockServer) -> LiveYouTubeStatusClient<FixedToken> {
    LiveYouTubeStatusClient::with_base(
        FixedToken("ya29.test-token"),
        format!("{}/videos", server.uri()),
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn status_reports_processed() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/videos"))
        .and(query_param("id", "vid-1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "items": [{
                "status": { "uploadStatus": "processed" },
                "processingDetails": { "processingStatus": "succeeded" }
            }]
        })))
        .mount(&server)
        .await;

    let resp = run_status(status_client(&server), "vid-1")
        .await
        .unwrap_or_else(|e| panic!("status poll should succeed: {e:?}"));

    assert_eq!(resp.state, YouTubeProcessingState::Processed);
    assert_eq!(resp.failure_reason, None);
}

#[tokio::test(flavor = "multi_thread")]
async fn status_reports_processing() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/videos"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "items": [{
                "status": { "uploadStatus": "uploaded" },
                "processingDetails": { "processingStatus": "processing" }
            }]
        })))
        .mount(&server)
        .await;

    let resp = run_status(status_client(&server), "vid-2")
        .await
        .unwrap_or_else(|e| panic!("status poll should succeed: {e:?}"));

    assert_eq!(resp.state, YouTubeProcessingState::Processing);
}

#[tokio::test(flavor = "multi_thread")]
async fn status_reports_failed_with_reason() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/videos"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "items": [{
                "status": { "uploadStatus": "failed", "rejectionReason": "copyright" },
                "processingDetails": { "processingStatus": "failed" }
            }]
        })))
        .mount(&server)
        .await;

    let resp = run_status(status_client(&server), "vid-3")
        .await
        .unwrap_or_else(|e| panic!("status poll should succeed: {e:?}"));

    assert_eq!(resp.state, YouTubeProcessingState::Failed);
    assert_eq!(resp.failure_reason.as_deref(), Some("copyright"));
}

#[tokio::test(flavor = "multi_thread")]
async fn status_server_error_is_network_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/videos"))
        .respond_with(ResponseTemplate::new(500).set_body_string("backend error"))
        .mount(&server)
        .await;

    let err = run_status(status_client(&server), "vid-4")
        .await
        .err()
        .unwrap_or_else(|| panic!("500 should error"));

    match err {
        YouTubeStatusClientError::NetworkOrServer(msg) => {
            assert!(
                msg.contains("500"),
                "expected status code in message, got: {msg}"
            );
        }
    }
}
