//! YouTube Data API v3 — resumable `videos.insert` upload (W6.A3).
//!
//! Google's resumable upload protocol is a two-step dance:
//!
//! 1. **Initiate**: POST the metadata JSON to
//!    `https://www.googleapis.com/upload/youtube/v3/videos?uploadType=resumable&part=snippet,status`.
//!    Response includes a `Location` header — the upload session URL.
//! 2. **Chunks**: PUT bytes to that session URL with
//!    `Content-Range: bytes <start>-<end>/<total>`. Intermediate chunks
//!    return `308 Resume Incomplete`; the final chunk returns `200 OK`
//!    with the created video resource JSON.
//!
//! Spec: <https://developers.google.com/youtube/v3/guides/using_resumable_upload_protocol>
//!
//! # Why a separate module
//!
//! Keeping the resumable plumbing out of `youtube.rs` lets the trait
//! impl stay small (auth + metadata + delegation) and lets the upload
//! logic be unit-tested against `wiremock` without dragging the OAuth
//! refresh path into the same test setup.
//!
//! # Progress reporting
//!
//! The caller passes an `on_progress` closure invoked with
//! `0.0..=1.0` after each chunk completes. The dispatcher
//! (`upload_queue::run_upload`) bridges this onto an mpsc channel that
//! the [`UploadQueue`](super::UploadQueue) drains into
//! `mark_uploading`.
//!
//! # Secret hygiene
//!
//! Per the W6 contract: access tokens never appear in `tracing::error!`
//! lines. Only `tracing::debug!` calls reference token-bearing requests
//! and even there we redact the bearer value.

use std::path::Path;
use std::time::Duration;

use serde::Deserialize;
use serde_json::json;
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncSeekExt, SeekFrom};

use super::errors::ProviderError;
use super::types::{UploadParams, UploadResult, Visibility};

/// Resumable upload initiate endpoint. Hardcoded here (rather than
/// passed in) because production callers always hit YouTube; tests use
/// [`upload_video_with_endpoint`] to point at a wiremock server.
const INITIATE_URL: &str =
    "https://www.googleapis.com/upload/youtube/v3/videos?uploadType=resumable&part=snippet,status";

/// Chunk size for the resumable PUT body. Google requires a multiple
/// of 256 KB for non-final chunks; 1 MB gives ~32 progress ticks for
/// a 32 MB file (typical short-form export) which is granular enough
/// for the UI's progress bar without spamming the queue.
pub(crate) const CHUNK_SIZE: usize = 1024 * 1024;

/// Default category — "People & Blogs". YouTube requires a categoryId
/// on every upload; this is the safest broadly-applicable default. A
/// future task can make it user-configurable via the upload form.
const DEFAULT_CATEGORY_ID: &str = "22";

/// Per-call HTTP timeout. Each chunk should land in well under 30s on
/// any reasonable connection; longer timeouts just delay the error
/// surface when the network has actually gone away.
const HTTP_TIMEOUT: Duration = Duration::from_secs(60);

/// Public entry point. Production callers use this; tests use
/// [`upload_video_with_endpoint`] to swap the initiate URL.
pub async fn upload_video<F>(
    access_token: String,
    params: UploadParams,
    on_progress: F,
) -> Result<UploadResult, ProviderError>
where
    F: Fn(f32) + Send + Sync,
{
    upload_video_with_endpoint(INITIATE_URL, access_token, params, on_progress).await
}

/// Endpoint-parameterised variant — the test seam. Production routes
/// through [`upload_video`] which pins the real Google URL.
pub async fn upload_video_with_endpoint<F>(
    initiate_url: &str,
    access_token: String,
    params: UploadParams,
    on_progress: F,
) -> Result<UploadResult, ProviderError>
where
    F: Fn(f32) + Send + Sync,
{
    let file_path = Path::new(&params.file_path);
    let metadata = tokio::fs::metadata(file_path).await.map_err(|e| {
        ProviderError::Io(format!(
            "stat {}: {e}",
            file_path.display(),
        ))
    })?;
    let total_size = metadata.len();
    if total_size == 0 {
        return Err(ProviderError::Io(format!(
            "render output is empty: {}",
            file_path.display(),
        )));
    }

    let client = build_client()?;
    let body = build_metadata_body(&params);
    let session_url = initiate_resumable(&client, initiate_url, &access_token, &body).await?;
    let video_id = upload_chunks(
        &client,
        &session_url,
        &access_token,
        file_path,
        total_size,
        on_progress,
    )
    .await?;

    Ok(UploadResult {
        remote_url: format!("https://youtu.be/{}", video_id),
        remote_id: video_id,
        uploaded_at: now_unix_seconds(),
    })
}

/// Build the `videos.insert` metadata payload per the YouTube Data API
/// v3 reference. Visibility maps onto `privacyStatus`; a scheduled
/// upload requires `privacyStatus = "private"` (Google's constraint —
/// `publishAt` is only honoured for private videos that the API will
/// flip to public at the scheduled time).
fn build_metadata_body(params: &UploadParams) -> serde_json::Value {
    let privacy_status = match params.visibility {
        Visibility::Public => "public",
        Visibility::Unlisted => "unlisted",
        Visibility::Private => "private",
    };

    let mut status = json!({
        "privacyStatus": privacy_status,
    });

    // Schedule: API constraint — `publishAt` only valid when
    // `privacyStatus == "private"`. We promote to private when a
    // schedule is set rather than rejecting the upload outright; the
    // platform will flip to public at the scheduled time.
    if let Some(epoch) = params.scheduled_at {
        if let Some(ts) = format_rfc3339_utc(epoch) {
            status["privacyStatus"] = json!("private");
            status["publishAt"] = json!(ts);
        }
    }

    // AI disclosure (W5.A4 → W6.A3): when the cut contains generated
    // media, set `containsSyntheticMedia` so YouTube surfaces the
    // "Altered or synthetic content" badge to viewers.
    if let Some(disclosure) = &params.ai_disclosure {
        if disclosure.has_synthetic_content {
            status["containsSyntheticMedia"] = json!(true);
        }
    }

    json!({
        "snippet": {
            "title": params.title,
            "description": params.description,
            "tags": params.tags,
            "categoryId": DEFAULT_CATEGORY_ID,
        },
        "status": status,
    })
}

/// Format a unix-epoch seconds value as RFC 3339 UTC (the wire shape
/// `publishAt` expects). Returns `None` for negative timestamps —
/// the caller drops the field rather than erroring.
///
/// Implemented by hand to avoid pulling in a `chrono` / `time`
/// dependency for one timestamp. Algorithm: civil-from-days per
/// Howard Hinnant, then concat the wall-clock portion of the day.
fn format_rfc3339_utc(epoch_seconds: i64) -> Option<String> {
    if epoch_seconds < 0 {
        return None;
    }
    let days = epoch_seconds.div_euclid(86_400);
    let secs_of_day = epoch_seconds.rem_euclid(86_400);
    let hours = secs_of_day / 3600;
    let minutes = (secs_of_day % 3600) / 60;
    let seconds = secs_of_day % 60;
    let (y, m, d) = civil_from_days(days);
    Some(format!(
        "{y:04}-{m:02}-{d:02}T{hours:02}:{minutes:02}:{seconds:02}Z",
    ))
}

/// Howard Hinnant's `civil_from_days` — days-since-1970 → (year, month, day).
/// Public-domain algorithm from <http://howardhinnant.github.io/date_algorithms.html>.
fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m, d)
}

/// Step 1: POST the metadata, parse the `Location` header out of the
/// response, return it as the session URL. Per the spec a 200 with a
/// missing Location is a protocol error — we surface that as `Io`
/// (it's a parse-shape failure, not a network failure).
async fn initiate_resumable(
    client: &reqwest::Client,
    initiate_url: &str,
    access_token: &str,
    body: &serde_json::Value,
) -> Result<String, ProviderError> {
    tracing::debug!(endpoint = initiate_url, "youtube resumable initiate POST");
    let response = client
        .post(initiate_url)
        .bearer_auth(access_token)
        .header("X-Upload-Content-Type", "video/*")
        .json(body)
        .send()
        .await
        .map_err(|e| ProviderError::NetworkError(format!("initiate POST failed: {e}")))?;

    let status = response.status();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Err(ProviderError::OAuthFailed(format!(
            "youtube rejected access token (HTTP {})",
            status.as_u16(),
        )));
    }
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        tracing::debug!(
            status = status.as_u16(),
            body = body.as_str(),
            "youtube initiate non-success",
        );
        return Err(ProviderError::NetworkError(format!(
            "initiate returned HTTP {}: {}",
            status.as_u16(),
            truncate_for_error(&body, 200),
        )));
    }

    let location = response
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
        .ok_or_else(|| {
            ProviderError::Io(
                "youtube initiate response missing Location header".to_string(),
            )
        })?;
    Ok(location)
}

/// Step 2: stream the file in [`CHUNK_SIZE`] pieces, PUT each to the
/// session URL with the correct `Content-Range`, call `on_progress`
/// after each successful chunk, return the parsed video id from the
/// final response.
async fn upload_chunks<F>(
    client: &reqwest::Client,
    session_url: &str,
    access_token: &str,
    file_path: &Path,
    total_size: u64,
    on_progress: F,
) -> Result<String, ProviderError>
where
    F: Fn(f32) + Send + Sync,
{
    let mut file = File::open(file_path).await.map_err(|e| {
        ProviderError::Io(format!("open {}: {e}", file_path.display()))
    })?;

    let mut offset: u64 = 0;
    let mut buffer = vec![0u8; CHUNK_SIZE];

    loop {
        // Resume from the recorded offset on each iteration. Cheap on
        // a freshly-opened file; the seek-then-read pattern also makes
        // a future "resume after network blip" extension trivial.
        file.seek(SeekFrom::Start(offset)).await.map_err(|e| {
            ProviderError::Io(format!("seek {}: {e}", file_path.display()))
        })?;

        let bytes_remaining = total_size.saturating_sub(offset);
        let target = std::cmp::min(buffer.len() as u64, bytes_remaining) as usize;
        let mut filled = 0;
        while filled < target {
            let n = file
                .read(&mut buffer[filled..target])
                .await
                .map_err(|e| {
                    ProviderError::Io(format!(
                        "read {} at offset {offset}: {e}",
                        file_path.display(),
                    ))
                })?;
            if n == 0 {
                return Err(ProviderError::Io(format!(
                    "unexpected EOF reading {} at offset {offset}",
                    file_path.display(),
                )));
            }
            filled += n;
        }

        let chunk_end = offset + filled as u64 - 1;
        let content_range = format!("bytes {}-{}/{}", offset, chunk_end, total_size);
        let chunk_bytes = buffer[..filled].to_vec();

        tracing::debug!(
            session = redact_session_url(session_url).as_str(),
            range = content_range.as_str(),
            "youtube chunk PUT",
        );

        let response = client
            .put(session_url)
            .bearer_auth(access_token)
            .header(reqwest::header::CONTENT_LENGTH, filled)
            .header(reqwest::header::CONTENT_RANGE, content_range.as_str())
            .body(chunk_bytes)
            .send()
            .await
            .map_err(|e| ProviderError::NetworkError(format!("chunk PUT failed: {e}")))?;

        let status = response.status();
        // 308 Resume Incomplete is the "keep going" signal for
        // intermediate chunks. 200 / 201 mean the upload is done and
        // the body is the created video resource.
        if status.as_u16() == 308 {
            offset = chunk_end + 1;
            on_progress((offset as f32 / total_size as f32).min(1.0));
            continue;
        }
        if status == reqwest::StatusCode::UNAUTHORIZED
            || status == reqwest::StatusCode::FORBIDDEN
        {
            return Err(ProviderError::OAuthFailed(format!(
                "youtube rejected access token mid-upload (HTTP {})",
                status.as_u16(),
            )));
        }
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            tracing::debug!(
                status = status.as_u16(),
                body = body.as_str(),
                "youtube chunk PUT non-success",
            );
            return Err(ProviderError::NetworkError(format!(
                "chunk PUT returned HTTP {}: {}",
                status.as_u16(),
                truncate_for_error(&body, 200),
            )));
        }

        // Final chunk: response body carries the created video resource.
        let body_text = response
            .text()
            .await
            .map_err(|e| ProviderError::NetworkError(format!("read final body: {e}")))?;
        let video: VideoResource = serde_json::from_str(&body_text).map_err(|e| {
            tracing::debug!(error = %e, body = body_text.as_str(), "parse final video resource failed");
            ProviderError::Io(format!("parse final video resource: {e}"))
        })?;
        on_progress(1.0);
        return Ok(video.id);
    }
}

/// Shape of YouTube's final `videos.insert` response. We only pull the
/// `id` — the rest is echoed-back metadata the caller already knows.
#[derive(Debug, Deserialize)]
struct VideoResource {
    id: String,
}

/// Build the shared HTTPS client with the per-upload timeout. Each
/// call constructs its own because the upload loop is fundamentally
/// long-lived (multi-MB streams) and we don't want a shared pool
/// holding stuck connections beyond one upload's lifetime.
fn build_client() -> Result<reqwest::Client, ProviderError> {
    reqwest::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .build()
        .map_err(|e| ProviderError::Io(format!("build reqwest client: {e}")))
}

/// Strip the upload_id query parameter from a session URL for log
/// lines. The upload_id is bearer-ish (it grants resume rights on the
/// in-flight upload) — keep it out of trace output.
fn redact_session_url(url: &str) -> String {
    match url.find('?') {
        Some(idx) => format!("{}?<redacted>", &url[..idx]),
        None => url.to_string(),
    }
}

/// Clip an error-payload string for inclusion in a user-facing error
/// message. Real Google error bodies can be hundreds of bytes; we
/// keep enough to be diagnostic without spamming the UI.
fn truncate_for_error(body: &str, limit: usize) -> String {
    if body.len() <= limit {
        body.to_string()
    } else {
        format!("{}…", &body[..limit])
    }
}

/// Unix epoch seconds — local copy so this module's dep graph stays
/// independent of the OAuth helper.
fn now_unix_seconds() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::publishing::ai_disclosure::{AiDisclosure, GeneratedMediaCredit};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use wiremock::matchers::{header, method, path as match_path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// Build a fake render file on disk. Returns the absolute path
    /// and the byte count for reuse in test assertions.
    fn write_test_file(tmp: &tempfile::TempDir, size_bytes: usize) -> std::path::PathBuf {
        let path = tmp.path().join("clip.mp4");
        // Distinct bytes so a tested range read can be content-checked
        // if a future regression makes us suspect offset arithmetic.
        let payload: Vec<u8> = (0..size_bytes).map(|i| (i % 251) as u8).collect();
        std::fs::write(&path, &payload).expect("write test file");
        path
    }

    fn params_for(file_path: &std::path::Path) -> UploadParams {
        UploadParams {
            file_path: file_path.to_string_lossy().into_owned(),
            title: "Test render".into(),
            description: "A test description".into(),
            tags: vec!["tag-a".into(), "tag-b".into()],
            visibility: Visibility::Private,
            scheduled_at: None,
            thumbnail_path: None,
            ai_disclosure: None,
            progress_tx: None,
        }
    }

    fn no_progress() -> impl Fn(f32) + Send + Sync {
        |_| {}
    }

    #[test]
    fn build_metadata_body_includes_category_and_visibility() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("x.mp4");
        let mut p = params_for(&path);
        p.visibility = Visibility::Unlisted;
        let body = build_metadata_body(&p);
        assert_eq!(body["snippet"]["title"], "Test render");
        assert_eq!(body["snippet"]["categoryId"], DEFAULT_CATEGORY_ID);
        assert_eq!(body["status"]["privacyStatus"], "unlisted");
        assert!(
            body["status"].get("containsSyntheticMedia").is_none(),
            "no disclosure → no flag",
        );
    }

    #[test]
    fn build_metadata_body_sets_synthetic_flag_when_disclosed() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("x.mp4");
        let mut p = params_for(&path);
        p.ai_disclosure = Some(AiDisclosure::from_credits(vec![GeneratedMediaCredit {
            provider: "runway".into(),
            model: None,
            prompt: "sunset".into(),
            generated_at: None,
            asset_id: "a".into(),
        }]));
        let body = build_metadata_body(&p);
        assert_eq!(body["status"]["containsSyntheticMedia"], true);
    }

    #[test]
    fn build_metadata_body_schedule_promotes_to_private() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("x.mp4");
        let mut p = params_for(&path);
        p.visibility = Visibility::Public;
        p.scheduled_at = Some(1_700_000_000);
        let body = build_metadata_body(&p);
        // Google's API: publishAt only honoured when privacyStatus =
        // "private". The metadata builder defends in depth.
        assert_eq!(body["status"]["privacyStatus"], "private");
        assert!(body["status"]["publishAt"].is_string());
    }

    #[tokio::test]
    async fn initiate_returns_session_url() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let file_path = write_test_file(&tmp, 1024);
        let server = MockServer::start().await;
        let session = format!("{}/upload-session/abc123?upload_id=secret", server.uri());

        // Initiate: 200 + Location header.
        Mock::given(method("POST"))
            .and(match_path("/initiate"))
            .and(header("authorization", "Bearer ya29.test"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("Location", session.as_str()),
            )
            .mount(&server)
            .await;

        // Chunk PUT: returns the final video resource.
        Mock::given(method("PUT"))
            .and(match_path("/upload-session/abc123"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({ "id": "vid-xyz" })),
            )
            .mount(&server)
            .await;

        let result = upload_video_with_endpoint(
            &format!("{}/initiate", server.uri()),
            "ya29.test".into(),
            params_for(&file_path),
            no_progress(),
        )
        .await
        .expect("upload ok");
        assert_eq!(result.remote_id, "vid-xyz");
        assert_eq!(result.remote_url, "https://youtu.be/vid-xyz");
    }

    #[tokio::test]
    async fn chunked_upload_reports_progress() {
        // 2.5 MB file → 3 chunks at 1 MB granularity (1 MB, 1 MB, 0.5 MB).
        // Progress callback should fire after each chunk.
        let tmp = tempfile::tempdir().expect("tempdir");
        let total_size: usize = (CHUNK_SIZE * 2) + (CHUNK_SIZE / 2);
        let file_path = write_test_file(&tmp, total_size);

        let server = MockServer::start().await;
        let session = format!("{}/session/abc?upload_id=sec", server.uri());

        Mock::given(method("POST"))
            .and(match_path("/initiate"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("Location", session.as_str()),
            )
            .mount(&server)
            .await;

        // First two PUTs: 308 (keep going). Third: 200 with video id.
        // wiremock doesn't support stateful per-call sequences out of
        // the box, so we route by Content-Range header instead — which
        // is exactly the discriminator the protocol uses.
        Mock::given(method("PUT"))
            .and(match_path("/session/abc"))
            .and(header(
                "content-range",
                format!("bytes 0-{}/{}", CHUNK_SIZE - 1, total_size).as_str(),
            ))
            .respond_with(ResponseTemplate::new(308))
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(match_path("/session/abc"))
            .and(header(
                "content-range",
                format!(
                    "bytes {}-{}/{}",
                    CHUNK_SIZE,
                    (CHUNK_SIZE * 2) - 1,
                    total_size,
                )
                .as_str(),
            ))
            .respond_with(ResponseTemplate::new(308))
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(match_path("/session/abc"))
            .and(header(
                "content-range",
                format!(
                    "bytes {}-{}/{}",
                    CHUNK_SIZE * 2,
                    total_size - 1,
                    total_size,
                )
                .as_str(),
            ))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({ "id": "vid-final" })),
            )
            .mount(&server)
            .await;

        let ticks = Arc::new(std::sync::Mutex::new(Vec::<f32>::new()));
        let ticks_clone = ticks.clone();
        let result = upload_video_with_endpoint(
            &format!("{}/initiate", server.uri()),
            "ya29.test".into(),
            params_for(&file_path),
            move |p| {
                ticks_clone
                    .lock()
                    .unwrap_or_else(|e| panic!("lock: {e}"))
                    .push(p);
            },
        )
        .await
        .expect("upload ok");
        assert_eq!(result.remote_id, "vid-final");

        let captured = ticks.lock().unwrap_or_else(|e| panic!("lock: {e}")).clone();
        assert_eq!(
            captured.len(),
            3,
            "expected one tick per chunk, got {captured:?}",
        );
        // First tick at 1 MB / 2.5 MB = 0.4, second at 2 MB / 2.5 MB =
        // 0.8, third = 1.0 from the final-chunk hook.
        assert!((captured[0] - 0.4).abs() < 0.01, "first tick: {captured:?}");
        assert!((captured[1] - 0.8).abs() < 0.01, "second tick: {captured:?}");
        assert!((captured[2] - 1.0).abs() < 0.001, "final tick: {captured:?}");
    }

    #[tokio::test]
    async fn final_chunk_returns_video_id() {
        // Smallest case: file fits in one chunk, single PUT returns 200.
        let tmp = tempfile::tempdir().expect("tempdir");
        let file_path = write_test_file(&tmp, 512);
        let server = MockServer::start().await;
        let session = format!("{}/s/u?upload_id=k", server.uri());

        Mock::given(method("POST"))
            .and(match_path("/initiate"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("Location", session.as_str()),
            )
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(match_path("/s/u"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({ "id": "single-chunk-id" })),
            )
            .mount(&server)
            .await;

        let result = upload_video_with_endpoint(
            &format!("{}/initiate", server.uri()),
            "ya29.tok".into(),
            params_for(&file_path),
            no_progress(),
        )
        .await
        .expect("upload ok");
        assert_eq!(result.remote_id, "single-chunk-id");
    }

    #[tokio::test]
    async fn auth_error_returns_oauth_failed() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let file_path = write_test_file(&tmp, 256);
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(match_path("/initiate"))
            .respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({
                "error": { "code": 401, "message": "Invalid Credentials" },
            })))
            .mount(&server)
            .await;
        let err = upload_video_with_endpoint(
            &format!("{}/initiate", server.uri()),
            "stale".into(),
            params_for(&file_path),
            no_progress(),
        )
        .await
        .expect_err("must error on 401");
        assert_eq!(err.kind(), "oauth_failed");
    }

    #[tokio::test]
    async fn missing_location_header_returns_io_error() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let file_path = write_test_file(&tmp, 256);
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(match_path("/initiate"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
        let err = upload_video_with_endpoint(
            &format!("{}/initiate", server.uri()),
            "ya29".into(),
            params_for(&file_path),
            no_progress(),
        )
        .await
        .expect_err("must error without Location");
        assert_eq!(err.kind(), "io");
    }

    #[tokio::test]
    async fn chunk_put_failure_returns_network_error() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let file_path = write_test_file(&tmp, 256);
        let server = MockServer::start().await;
        let session = format!("{}/sess/x?upload_id=k", server.uri());
        Mock::given(method("POST"))
            .and(match_path("/initiate"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("Location", session.as_str()),
            )
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(match_path("/sess/x"))
            .respond_with(ResponseTemplate::new(500).set_body_string("Server Error"))
            .mount(&server)
            .await;

        let err = upload_video_with_endpoint(
            &format!("{}/initiate", server.uri()),
            "ya29".into(),
            params_for(&file_path),
            no_progress(),
        )
        .await
        .expect_err("must error on chunk 500");
        assert_eq!(err.kind(), "network_error");
    }

    #[tokio::test]
    async fn empty_file_returns_io_error() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let file_path = tmp.path().join("empty.mp4");
        std::fs::write(&file_path, b"").expect("write empty");
        let err = upload_video_with_endpoint(
            "http://127.0.0.1:1/never",
            "ya29".into(),
            params_for(&file_path),
            no_progress(),
        )
        .await
        .expect_err("must reject empty file");
        assert_eq!(err.kind(), "io");
    }

    /// Sanity: every tick we feed `on_progress` must remain clamped to
    /// `0.0..=1.0` even on the final-tick hook. This is the contract
    /// the upload-queue drain depends on for the UI progress bar.
    #[tokio::test]
    async fn progress_values_stay_in_unit_range() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let total_size: usize = CHUNK_SIZE + (CHUNK_SIZE / 4);
        let file_path = write_test_file(&tmp, total_size);
        let server = MockServer::start().await;
        let session = format!("{}/u/sess?upload_id=k", server.uri());
        Mock::given(method("POST"))
            .and(match_path("/initiate"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("Location", session.as_str()),
            )
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(match_path("/u/sess"))
            .and(header(
                "content-range",
                format!("bytes 0-{}/{}", CHUNK_SIZE - 1, total_size).as_str(),
            ))
            .respond_with(ResponseTemplate::new(308))
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(match_path("/u/sess"))
            .and(header(
                "content-range",
                format!(
                    "bytes {}-{}/{}",
                    CHUNK_SIZE,
                    total_size - 1,
                    total_size,
                )
                .as_str(),
            ))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({ "id": "vid-clamp" })),
            )
            .mount(&server)
            .await;

        let counter = Arc::new(AtomicUsize::new(0));
        let counter_clone = counter.clone();
        let result = upload_video_with_endpoint(
            &format!("{}/initiate", server.uri()),
            "ya29".into(),
            params_for(&file_path),
            move |p| {
                assert!((0.0..=1.0).contains(&p), "out-of-range progress: {p}");
                counter_clone.fetch_add(1, Ordering::Relaxed);
            },
        )
        .await
        .expect("upload ok");
        assert_eq!(result.remote_id, "vid-clamp");
        assert!(counter.load(Ordering::Relaxed) >= 2);
    }

    #[test]
    fn redact_session_url_strips_query() {
        assert_eq!(
            redact_session_url("https://example.com/upload?upload_id=secret"),
            "https://example.com/upload?<redacted>",
        );
        assert_eq!(
            redact_session_url("https://example.com/upload"),
            "https://example.com/upload",
        );
    }

    #[test]
    fn truncate_for_error_keeps_short_bodies() {
        assert_eq!(truncate_for_error("short", 20), "short");
        assert_eq!(truncate_for_error("0123456789", 5), "01234…");
    }

    #[test]
    fn civil_from_days_matches_known_dates() {
        // 0 days from 1970-01-01 = 1970-01-01.
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        // 31 days = 1970-02-01.
        assert_eq!(civil_from_days(31), (1970, 2, 1));
        // Leap-year sanity: 2020-02-29 = day 18_321.
        assert_eq!(civil_from_days(18_321), (2020, 2, 29));
    }
}
