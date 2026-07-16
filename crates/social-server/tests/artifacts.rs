//! R16 — hermetic route tests for the artifact surfaces:
//! `POST /artifacts/upload-url` (service-bearer gated) and
//! `GET /public/artifacts/{filename}` (HMAC-signed public URLs).

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use common::{now_secs, scheduled_job, serve, state_with};
use hmac::{Hmac, Mac};
use montage_social::model::Provider;
use montage_social::store::SocialStore;
use montage_social_server::{ServerConfig, StoreHandle};
use sha2::Sha256;

const SECRET: &str = "artifact-signing-secret";

/// Recompute the server's public-artifact signature:
/// base64url_nopad(HMAC-SHA256(secret, "{job_id}.{expires_at}")).
fn sign(secret: &str, job_id: &str, expires_at: i64) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(format!("{job_id}.{expires_at}").as_bytes());
    URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
}

/// R16.8 — the upload-url handler requires the service bearer.
#[tokio::test]
async fn upload_url_requires_service_bearer() {
    let config = ServerConfig {
        service_shared_secret: SECRET.into(),
        ..ServerConfig::default()
    };
    let base = serve(state_with(config, StoreHandle::in_memory())).await;
    let client = reqwest::Client::new();

    let unauthed = client
        .post(format!("{base}/artifacts/upload-url"))
        .json(&serde_json::json!({"object_path": "jobs/j1/artifact.mp4"}))
        .send()
        .await
        .unwrap();
    assert_eq!(unauthed.status().as_u16(), 401, "no bearer → 401");

    let wrong = client
        .post(format!("{base}/artifacts/upload-url"))
        .bearer_auth("wrong-secret")
        .json(&serde_json::json!({"object_path": "jobs/j1/artifact.mp4"}))
        .send()
        .await
        .unwrap();
    assert_eq!(wrong.status().as_u16(), 401, "wrong bearer → 401");
}

/// R16.9 — public_artifact_handler rejects a bad/expired HMAC and serves the
/// artifact bytes for a valid signature.
#[tokio::test]
async fn public_artifact_rejects_bad_hmac_and_serves_valid_one() {
    let now = now_secs();

    // Artifact bytes on disk, confined under the configured base dir.
    let artifacts = tempfile::tempdir().expect("artifact dir");
    let artifact_path = artifacts.path().join("render.mp4");
    let payload = b"these-are-the-artifact-bytes";
    std::fs::write(&artifact_path, payload).expect("write artifact");
    let artifact_ref = format!("file://{}", artifact_path.to_string_lossy());

    let store = StoreHandle::in_memory();
    store
        .open()
        .save_publish_job(scheduled_job(
            "job_pub",
            "acct_1",
            Provider::YouTube,
            &artifact_ref,
            now + 600,
            now,
        ))
        .expect("seed job");

    let config = ServerConfig {
        service_shared_secret: SECRET.into(),
        artifact_base_dir: artifacts.path().to_string_lossy().into_owned(),
        ..ServerConfig::default()
    };
    let base = serve(state_with(config, store)).await;
    let client = reqwest::Client::new();
    let exp = now + 600;

    // Tampered signature → 401.
    let bad_sig = sign("some-other-secret", "job_pub", exp);
    let rejected = client
        .get(format!(
            "{base}/public/artifacts/job_pub.mp4?exp={exp}&sig={bad_sig}"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(rejected.status().as_u16(), 401, "forged HMAC rejected");

    // Signature for a DIFFERENT job id → 401 (payload binding).
    let stolen_sig = sign(SECRET, "job_other", exp);
    let cross = client
        .get(format!(
            "{base}/public/artifacts/job_pub.mp4?exp={exp}&sig={stolen_sig}"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(cross.status().as_u16(), 401, "signature bound to job id");

    // Valid signature but expired timestamp → 401.
    let past = now - 10;
    let expired_sig = sign(SECRET, "job_pub", past);
    let expired = client
        .get(format!(
            "{base}/public/artifacts/job_pub.mp4?exp={past}&sig={expired_sig}"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(expired.status().as_u16(), 401, "expired link rejected");

    // Valid signature → the artifact is served.
    let good_sig = sign(SECRET, "job_pub", exp);
    let served = client
        .get(format!(
            "{base}/public/artifacts/job_pub.mp4?exp={exp}&sig={good_sig}"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(served.status().as_u16(), 200);
    assert_eq!(
        served
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok()),
        Some("video/mp4")
    );
    assert_eq!(
        served
            .headers()
            .get("cache-control")
            .and_then(|v| v.to_str().ok()),
        Some("private, no-store")
    );
    let body = served.bytes().await.unwrap();
    assert_eq!(&body[..], payload, "exact artifact bytes served");
}
