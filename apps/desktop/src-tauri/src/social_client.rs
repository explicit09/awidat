//! Thin authenticated HTTPS client of the `awidat-social-server`.
//!
//! Phase 5 moves the desktop's social-publishing path off the in-process
//! `SocialApi` + local SQLite store and onto the server (Phases 1-4). This
//! module is the only place the desktop talks to that server: a typed wrapper
//! over the user-facing routes in `crates/social-server/src/user_routes.rs`.
//!
//! Every method:
//! - attaches `Authorization: Bearer <auth_token>` (the pre-Phase-7 dev token),
//! - sends/receives the **re-exported `awidat_social::api` DTOs** so client and
//!   server agree on exactly one serde shape (the big reuse win), and
//! - maps a non-2xx response to a stable error string (`401` → `"unauthorized"`)
//!   the frontend can branch on, never leaking a token or response body.
//!
//! The rendered-artifact upload streams from disk via
//! [`reqwest::Body::wrap_stream`] over a [`tokio::fs::File`] so a multi-GB
//! render never loads fully into RAM (mirrors the streaming concern documented
//! on the `MediaServer` state).

use awidat_social::api::{
    AccountSummary, BindTargetRequest, OAuthStartResponse, PublishJobResponse,
    ScheduleTargetRequest, ValidateTargetRequest,
};
use awidat_social::model::{AccountUsageAudit, CampaignVariantTarget, Provider};
use serde::{Deserialize, Serialize};
use tokio_util::io::ReaderStream;

/// Authenticated HTTPS client for the social-publishing server.
///
/// Cheap to clone — `reqwest::Client` is an `Arc` internally — so it can be
/// stashed behind a `Mutex<Option<_>>` in `AwidatState` and handed to every
/// `social_*` command.
#[derive(Clone)]
pub struct SocialClient {
    base_url: String,
    auth_token: String,
    http: reqwest::Client,
}

/// Server upload-handshake response (`POST /social/jobs/{id}/upload-url`).
///
/// Mirrors `UploadUrlResponse` in `crates/social-server/src/user_routes.rs`
/// (default serde = snake_case).
#[derive(Debug, Clone, Deserialize)]
pub struct UploadUrl {
    /// Signed PUT URL the desktop streams the rendered file to.
    pub url: String,
    /// HTTP method to use for the upload (`PUT` for the signed-URL path). Part
    /// of the wire contract; we always PUT, so it is read only in tests today.
    #[allow(dead_code)]
    pub method: String,
    /// Opaque storage ref the server staged. Informational only — the desktop
    /// no longer echoes it back (the server re-derives it on upload-complete).
    #[allow(dead_code)]
    pub storage_ref: String,
    /// When true the desktop must POST multipart to a server proxy instead.
    /// Reserved; always false for the signed-URL path today.
    pub direct: bool,
}

/// Body of `POST /social/oauth/start/{provider}` (snake_case `return_to`).
#[derive(Debug, Clone, Serialize)]
struct OAuthStartBody {
    return_to: String,
}

impl SocialClient {
    /// Build a client from a base URL + dev bearer token. Trailing slashes on
    /// `base_url` are trimmed so route joins never double-slash.
    pub fn new(base_url: impl Into<String>, auth_token: impl Into<String>) -> Self {
        let base_url = base_url.into().trim_end_matches('/').to_string();
        Self {
            base_url,
            auth_token: auth_token.into(),
            http: reqwest::Client::new(),
        }
    }

    /// Resolve the client from the environment.
    ///
    /// Per RECONCILIATION G6 there is no per-field desktop config struct, so the
    /// server URL + dev token are read from env at setup time (mirroring how
    /// `project_root` defaults from `AWIDAT_DESKTOP_PROJECT`). Returns `None`
    /// when `AWIDAT_SOCIAL_SERVER_URL` is unset, so the desktop boots fine
    /// without the server configured — the `social_*` commands then surface a
    /// clear "social client not initialized" error if used.
    pub fn from_env() -> Option<Self> {
        let base_url = std::env::var("AWIDAT_SOCIAL_SERVER_URL").ok()?;
        if base_url.trim().is_empty() {
            return None;
        }
        let auth_token = std::env::var("AWIDAT_SOCIAL_AUTH_TOKEN").unwrap_or_default();
        Some(Self::new(base_url, auth_token))
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }

    /// `GET /social/accounts`
    pub async fn accounts(&self) -> Result<Vec<AccountSummary>, String> {
        self.get_json("/social/accounts").await
    }

    /// `POST /social/oauth/start/{provider}`
    pub async fn oauth_start(
        &self,
        provider: &Provider,
        return_to: String,
    ) -> Result<OAuthStartResponse, String> {
        let path = format!("/social/oauth/start/{}", provider.as_str());
        self.post_json(&path, &OAuthStartBody { return_to }).await
    }

    /// `POST /social/accounts/{id}/disconnect`
    pub async fn disconnect_account(&self, account_id: &str) -> Result<AccountSummary, String> {
        let path = format!("/social/accounts/{account_id}/disconnect");
        self.post_empty(&path).await
    }

    /// `GET /social/accounts/{id}/audit`
    pub async fn account_audit(&self, account_id: &str) -> Result<AccountUsageAudit, String> {
        let path = format!("/social/accounts/{account_id}/audit");
        self.get_json(&path).await
    }

    /// `POST /social/targets/bind`
    pub async fn bind_target(
        &self,
        request: &BindTargetRequest,
    ) -> Result<CampaignVariantTarget, String> {
        self.post_json("/social/targets/bind", request).await
    }

    /// `POST /social/targets/validate`
    pub async fn validate_target(
        &self,
        request: &ValidateTargetRequest,
    ) -> Result<CampaignVariantTarget, String> {
        self.post_json("/social/targets/validate", request).await
    }

    /// `POST /social/targets/schedule`
    pub async fn schedule_target(
        &self,
        request: &ScheduleTargetRequest,
    ) -> Result<PublishJobResponse, String> {
        self.post_json("/social/targets/schedule", request).await
    }

    /// `GET /social/jobs/{id}`
    pub async fn publish_job(&self, job_id: &str) -> Result<PublishJobResponse, String> {
        let path = format!("/social/jobs/{job_id}");
        self.get_json(&path).await
    }

    /// `POST /social/jobs/{id}/cancel`
    pub async fn cancel_job(&self, job_id: &str) -> Result<PublishJobResponse, String> {
        let path = format!("/social/jobs/{job_id}/cancel");
        self.post_empty(&path).await
    }

    /// `POST /social/jobs/{id}/retry`
    pub async fn retry_job(&self, job_id: &str) -> Result<PublishJobResponse, String> {
        let path = format!("/social/jobs/{job_id}/retry");
        self.post_empty(&path).await
    }

    /// `POST /social/jobs/{id}/upload-url`
    pub async fn request_upload_url(&self, job_id: &str) -> Result<UploadUrl, String> {
        let path = format!("/social/jobs/{job_id}/upload-url");
        self.post_empty(&path).await
    }

    /// `POST /social/jobs/{id}/upload-complete`
    ///
    /// No body: the server regenerates the storage ref from `(bucket, job_id)`
    /// server-side (it never trusts a client-supplied path — that would be an
    /// arbitrary-file-read sink). The desktop just signals "bytes are staged".
    pub async fn complete_upload(&self, job_id: &str) -> Result<PublishJobResponse, String> {
        let path = format!("/social/jobs/{job_id}/upload-complete");
        self.post_empty(&path).await
    }

    /// Stream `path`'s bytes to a signed PUT URL.
    ///
    /// The body is wrapped over a [`tokio::fs::File`] reader stream, so the file
    /// is sent in chunks straight from disk — a multi-GB render never lands in
    /// RAM. The signed URL already carries its own auth, so this request does
    /// **not** attach the server bearer.
    pub async fn put_file(&self, url: &str, file_path: &std::path::Path) -> Result<(), String> {
        let file = tokio::fs::File::open(file_path)
            .await
            .map_err(|e| format!("open render file: {e}"))?;
        let len = file
            .metadata()
            .await
            .map(|m| m.len())
            .map_err(|e| format!("stat render file: {e}"))?;
        let body = reqwest::Body::wrap_stream(ReaderStream::new(file));
        let resp = self
            .http
            .put(url)
            .header(reqwest::header::CONTENT_LENGTH, len)
            .body(body)
            .send()
            .await
            .map_err(|e| format!("upload PUT failed: {e}"))?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(status_error(resp.status()))
        }
    }

    // ── shared helpers ──────────────────────────────────────────────────────

    async fn get_json<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T, String> {
        let resp = self
            .http
            .get(self.url(path))
            .bearer_auth(&self.auth_token)
            .send()
            .await
            .map_err(|e| format!("request failed: {e}"))?;
        Self::decode(resp).await
    }

    async fn post_json<B: Serialize, T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T, String> {
        let resp = self
            .http
            .post(self.url(path))
            .bearer_auth(&self.auth_token)
            .json(body)
            .send()
            .await
            .map_err(|e| format!("request failed: {e}"))?;
        Self::decode(resp).await
    }

    async fn post_empty<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T, String> {
        let resp = self
            .http
            .post(self.url(path))
            .bearer_auth(&self.auth_token)
            .send()
            .await
            .map_err(|e| format!("request failed: {e}"))?;
        Self::decode(resp).await
    }

    /// Map a response into the typed DTO, or a stable error string. A non-2xx
    /// status maps via [`status_error`]; a JSON shape mismatch maps to a parse
    /// error. The raw body is never surfaced for a failure status (it could
    /// carry server detail), only the mapped string.
    async fn decode<T: serde::de::DeserializeOwned>(resp: reqwest::Response) -> Result<T, String> {
        let status = resp.status();
        if !status.is_success() {
            return Err(status_error(status));
        }
        resp.json::<T>()
            .await
            .map_err(|e| format!("decode response: {e}"))
    }
}

/// Map an HTTP status to a stable, token-safe error string. `401` collapses to
/// `"unauthorized"` (matching the prior `social.rs` convention); everything else
/// carries just the status code.
fn status_error(status: reqwest::StatusCode) -> String {
    if status == reqwest::StatusCode::UNAUTHORIZED {
        "unauthorized".to_string()
    } else {
        format!("server returned {}", status.as_u16())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use awidat_social::model::{
        AccountEligibility, AccountKind, ConnectedAccountStatus, OwnerRef, ProviderCapabilities,
    };
    use wiremock::matchers::{header, method, path as match_path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    // The server's AccountSummary DTO is camelCase (#[serde(rename_all)]); the
    // wire fixtures must match (incl. the camelCase CapabilitiesDto).
    fn account_json() -> serde_json::Value {
        serde_json::json!({
            "id": "acct_1",
            "owner": { "user": "local-user" },
            "provider": "youtube",
            "providerAccountId": "channel_1",
            "displayName": "Awidat Channel",
            "handle": "@awidat",
            "avatarUrl": null,
            "accountKind": "channel",
            "status": "connected",
            "scopes": ["youtube.upload"],
            "capabilities": {
                "nativeScheduling": true,
                "queueScheduling": true,
                "uploadVideo": true,
                "uploadThumbnail": true,
                "publicPosting": true,
                "requiresUserConsent": false
            },
            "eligibility": AccountEligibility::eligible(),
            "lastVerifiedAt": null,
            "createdAt": 1,
            "updatedAt": 1,
        })
    }

    #[tokio::test]
    async fn accounts_sends_bearer_and_round_trips_dto() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(match_path("/social/accounts"))
            .and(header("authorization", "Bearer dev-token"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!([account_json()])),
            )
            .mount(&server)
            .await;

        let client = SocialClient::new(server.uri(), "dev-token");
        let accounts = client.accounts().await.expect("accounts ok");
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].id, "acct_1");
        assert_eq!(accounts[0].display_name, "Awidat Channel");
        assert_eq!(accounts[0].provider, Provider::YouTube);
        assert_eq!(accounts[0].status, ConnectedAccountStatus::Connected);
        assert_eq!(accounts[0].account_kind, AccountKind::Channel);
        assert_eq!(accounts[0].owner, OwnerRef::User("local-user".into()));
    }

    #[tokio::test]
    async fn oauth_start_posts_provider_slug_and_return_to() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(match_path("/social/oauth/start/youtube"))
            .and(header("authorization", "Bearer dev-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "oauthConnectionId": "oauthconn-youtube-1",
                "provider": "youtube",
                "authorizationUrl": "https://accounts.google.com/o/oauth2/v2/auth?x=1",
            })))
            .mount(&server)
            .await;

        let client = SocialClient::new(server.uri(), "dev-token");
        let start = client
            .oauth_start(&Provider::YouTube, "/campaigns".into())
            .await
            .expect("oauth start ok");
        assert_eq!(start.oauth_connection_id, "oauthconn-youtube-1");
        assert_eq!(start.provider, Provider::YouTube);
        assert!(
            start
                .authorization_url
                .starts_with("https://accounts.google.com/")
        );
    }

    #[tokio::test]
    async fn upload_url_round_trips_snake_case_fields() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(match_path("/social/jobs/job_1/upload-url"))
            .and(header("authorization", "Bearer dev-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "url": "https://storage.example/signed",
                "method": "PUT",
                "storage_ref": "supabase-storage://bucket/jobs/job_1/artifact.mp4",
                "direct": false,
            })))
            .mount(&server)
            .await;

        let client = SocialClient::new(server.uri(), "dev-token");
        let upload = client.request_upload_url("job_1").await.expect("url ok");
        assert_eq!(upload.url, "https://storage.example/signed");
        assert_eq!(upload.method, "PUT");
        assert_eq!(
            upload.storage_ref,
            "supabase-storage://bucket/jobs/job_1/artifact.mp4"
        );
        assert!(!upload.direct);
    }

    #[tokio::test]
    async fn unauthorized_status_maps_to_stable_string() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(match_path("/social/accounts"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;

        let client = SocialClient::new(server.uri(), "wrong-token");
        let err = client.accounts().await.expect_err("must be unauthorized");
        assert_eq!(err, "unauthorized");
    }

    #[tokio::test]
    async fn non_2xx_status_maps_to_code_string() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(match_path("/social/jobs/job_1/cancel"))
            .respond_with(ResponseTemplate::new(422))
            .mount(&server)
            .await;

        let client = SocialClient::new(server.uri(), "dev-token");
        let err = client.cancel_job("job_1").await.expect_err("must be 422");
        assert_eq!(err, "server returned 422");
    }

    #[tokio::test]
    async fn put_file_streams_body_to_signed_url() {
        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .and(match_path("/signed-put"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let tmp = tempfile::tempdir().expect("tempdir");
        let file_path = tmp.path().join("artifact.mp4");
        tokio::fs::write(&file_path, b"rendered-bytes")
            .await
            .expect("write file");

        let client = SocialClient::new(server.uri(), "dev-token");
        let url = format!("{}/signed-put", server.uri());
        client.put_file(&url, &file_path).await.expect("put ok");
    }
}
