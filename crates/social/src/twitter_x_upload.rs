use crate::model::Provider;
use crate::upload_adapter::{UploadAdapter, UploadAdapterError, UploadRequest, UploadResult};
use crate::upload_status::{
    UploadProcessingStatus, UploadStatusAdapter, UploadStatusAdapterError, UploadStatusRequest,
    UploadStatusResult,
};
use serde::{Deserialize, Serialize};

pub const TWITTER_X_API_BASE: &str = "https://api.x.com";
pub const TWITTER_X_VIDEO_MEDIA_TYPE: &str = "video/mp4";
pub const TWITTER_X_VIDEO_MEDIA_CATEGORY: &str = "tweet_video";
pub const TWITTER_X_VIDEO_MAX_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TwitterXUploadRequest {
    pub media_url: String,
    pub text: String,
    pub access_token_ref: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TwitterXUploadResponse {
    pub media_id: String,
    pub post_id: Option<String>,
    pub post_url: Option<String>,
    pub processing: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TwitterXProcessingRef {
    pub media_id: String,
    pub text: String,
}

impl TwitterXProcessingRef {
    pub fn encode(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| self.media_id.clone())
    }

    pub fn decode(raw: &str) -> Result<Self, TwitterXStatusClientError> {
        serde_json::from_str(raw).map_err(|e| {
            TwitterXStatusClientError::NetworkOrServer(format!(
                "invalid twitter_x processing ref: {e}"
            ))
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TwitterXUploadClientError {
    MissingScope,
    RateLimited,
    NetworkOrServer(String),
}

pub trait TwitterXUploadClient {
    fn create_post_with_media(
        &self,
        request: &TwitterXUploadRequest,
    ) -> Result<TwitterXUploadResponse, TwitterXUploadClientError>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TwitterXStatusRequest {
    pub processing_ref: TwitterXProcessingRef,
    pub access_token_ref: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TwitterXStatusResponse {
    pub state: TwitterXMediaState,
    pub post_id: Option<String>,
    pub post_url: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TwitterXMediaState {
    Processing,
    Published,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TwitterXStatusClientError {
    RateLimited,
    NetworkOrServer(String),
}

pub trait TwitterXStatusClient {
    fn publish_if_ready(
        &self,
        request: &TwitterXStatusRequest,
    ) -> Result<TwitterXStatusResponse, TwitterXStatusClientError>;
}

#[derive(Clone, Debug)]
pub struct TwitterXUploadAdapter<C> {
    client: C,
}

impl<C> TwitterXUploadAdapter<C> {
    pub fn new(client: C) -> Self {
        Self { client }
    }
}

impl<C: TwitterXUploadClient> UploadAdapter for TwitterXUploadAdapter<C> {
    fn provider(&self) -> Provider {
        Provider::TwitterX
    }

    fn upload(&self, request: &UploadRequest) -> Result<UploadResult, UploadAdapterError> {
        if request.provider != Provider::TwitterX {
            return Err(UploadAdapterError::ProviderMismatch);
        }
        if request.access_token_ref.trim().is_empty() {
            return Err(UploadAdapterError::MissingUploadToken);
        }
        let text = request.title.trim();
        if text.is_empty() {
            return Err(UploadAdapterError::MediaConstraintFailed {
                reason: "twitter_x_text_required".into(),
            });
        }
        let response = self
            .client
            .create_post_with_media(&TwitterXUploadRequest {
                media_url: request.artifact_ref.clone(),
                text: text.to_string(),
                access_token_ref: request.access_token_ref.clone(),
            })
            .map_err(twitter_x_client_error)?;
        if response.processing {
            let processing_ref = TwitterXProcessingRef {
                media_id: response.media_id,
                text: text.to_string(),
            };
            return Ok(UploadResult {
                provider_post_id: processing_ref.encode(),
                provider_post_url: String::new(),
                processing: true,
            });
        }
        let post_id = response
            .post_id
            .ok_or_else(|| UploadAdapterError::NetworkOrServer {
                message: "twitter_x upload response missing post id".into(),
            })?;
        let post_url = response
            .post_url
            .ok_or_else(|| UploadAdapterError::NetworkOrServer {
                message: "twitter_x upload response missing post url".into(),
            })?;
        Ok(UploadResult {
            provider_post_id: post_id,
            provider_post_url: post_url,
            processing: false,
        })
    }
}

#[derive(Clone, Debug)]
pub struct TwitterXStatusAdapter<C> {
    client: C,
}

impl<C> TwitterXStatusAdapter<C> {
    pub fn new(client: C) -> Self {
        Self { client }
    }
}

impl<C: TwitterXStatusClient> UploadStatusAdapter for TwitterXStatusAdapter<C> {
    fn provider(&self) -> Provider {
        Provider::TwitterX
    }

    fn poll_status(
        &self,
        request: &UploadStatusRequest,
    ) -> Result<UploadStatusResult, UploadStatusAdapterError> {
        if request.provider != Provider::TwitterX {
            return Err(UploadStatusAdapterError::ProviderMismatch);
        }
        if request.provider_post_id.trim().is_empty() {
            return Err(UploadStatusAdapterError::MissingProviderPostId);
        }
        let processing_ref = TwitterXProcessingRef::decode(&request.provider_post_id)
            .map_err(twitter_x_status_client_error)?;
        let response = self
            .client
            .publish_if_ready(&TwitterXStatusRequest {
                processing_ref,
                access_token_ref: request.access_token_ref.clone(),
            })
            .map_err(twitter_x_status_client_error)?;
        match response.state {
            TwitterXMediaState::Processing => Ok(UploadStatusResult {
                provider_post_id: request.provider_post_id.clone(),
                provider_post_url: None,
                status: UploadProcessingStatus::Processing,
                normalized_error: None,
                raw_error_ref: None,
            }),
            TwitterXMediaState::Failed => Ok(UploadStatusResult {
                provider_post_id: request.provider_post_id.clone(),
                provider_post_url: None,
                status: UploadProcessingStatus::Failed,
                normalized_error: Some("platform_processing_failed".into()),
                raw_error_ref: Some(format!(
                    "twitter_x/media/{}/failed",
                    request.provider_post_id
                )),
            }),
            TwitterXMediaState::Published => {
                let post_id =
                    response
                        .post_id
                        .ok_or_else(|| UploadStatusAdapterError::NetworkOrServer {
                            message: "twitter_x status response missing post id".into(),
                        })?;
                Ok(UploadStatusResult {
                    provider_post_id: post_id,
                    provider_post_url: response.post_url,
                    status: UploadProcessingStatus::Published,
                    normalized_error: None,
                    raw_error_ref: None,
                })
            }
        }
    }
}

fn twitter_x_status_client_error(error: TwitterXStatusClientError) -> UploadStatusAdapterError {
    match error {
        TwitterXStatusClientError::RateLimited => UploadStatusAdapterError::NetworkOrServer {
            message: "rate_limited".into(),
        },
        TwitterXStatusClientError::NetworkOrServer(message) => {
            UploadStatusAdapterError::NetworkOrServer { message }
        }
    }
}

fn twitter_x_client_error(error: TwitterXUploadClientError) -> UploadAdapterError {
    match error {
        TwitterXUploadClientError::MissingScope => UploadAdapterError::RequiresAction {
            reason: "missing_scope".into(),
        },
        TwitterXUploadClientError::RateLimited => UploadAdapterError::NetworkOrServer {
            message: "rate_limited".into(),
        },
        TwitterXUploadClientError::NetworkOrServer(message) => {
            UploadAdapterError::NetworkOrServer { message }
        }
    }
}

pub struct LiveTwitterXUploadClient<R> {
    token_resolver: R,
    api_base: String,
    http: reqwest::Client,
    max_video_bytes: u64,
}

pub struct LiveTwitterXStatusClient<R> {
    token_resolver: R,
    api_base: String,
    http: reqwest::Client,
}

impl<R: crate::youtube_upload::AccessTokenResolver> LiveTwitterXStatusClient<R> {
    pub fn new(token_resolver: R) -> Self {
        Self::with_base(token_resolver, TWITTER_X_API_BASE.to_string())
    }

    pub fn with_base(token_resolver: R, api_base: String) -> Self {
        Self {
            token_resolver,
            api_base,
            http: reqwest::Client::new(),
        }
    }

    async fn do_publish_if_ready(
        &self,
        request: &TwitterXStatusRequest,
        token: String,
    ) -> Result<TwitterXStatusResponse, TwitterXStatusClientError> {
        let state = self
            .media_state(&request.processing_ref.media_id, &token)
            .await?;
        if state != TwitterXMediaState::Published {
            return Ok(TwitterXStatusResponse {
                state,
                post_id: None,
                post_url: None,
            });
        }
        let (post_id, post_url) = create_tweet(
            &self.http,
            &self.api_base,
            &request.processing_ref.text,
            &request.processing_ref.media_id,
            &token,
        )
        .await
        .map_err(twitter_x_upload_to_status_error)?;
        Ok(TwitterXStatusResponse {
            state: TwitterXMediaState::Published,
            post_id: Some(post_id),
            post_url: Some(post_url),
        })
    }

    async fn media_state(
        &self,
        media_id: &str,
        token: &str,
    ) -> Result<TwitterXMediaState, TwitterXStatusClientError> {
        let url = format!("{}/2/media/upload", self.api_base.trim_end_matches('/'));
        let resp = self
            .http
            .get(url)
            .bearer_auth(token)
            .query(&[("media_id", media_id)])
            .send()
            .await
            .map_err(|e| TwitterXStatusClientError::NetworkOrServer(e.to_string()))?;
        let status = resp.status().as_u16();
        let json: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| TwitterXStatusClientError::NetworkOrServer(e.to_string()))?;
        if status == 429 {
            return Err(TwitterXStatusClientError::RateLimited);
        }
        if !http_success(status) {
            return Err(TwitterXStatusClientError::NetworkOrServer(format!(
                "twitter_x status {status}: {json}"
            )));
        }
        let state = json["data"]["processing_info"]["state"]
            .as_str()
            .unwrap_or("succeeded");
        Ok(match state {
            "succeeded" => TwitterXMediaState::Published,
            "failed" => TwitterXMediaState::Failed,
            _ => TwitterXMediaState::Processing,
        })
    }
}

impl<R: crate::youtube_upload::AccessTokenResolver> TwitterXStatusClient
    for LiveTwitterXStatusClient<R>
{
    fn publish_if_ready(
        &self,
        request: &TwitterXStatusRequest,
    ) -> Result<TwitterXStatusResponse, TwitterXStatusClientError> {
        let token = self
            .token_resolver
            .bearer_for(&request.access_token_ref)
            .map_err(|e| TwitterXStatusClientError::NetworkOrServer(e.to_string()))?;
        tokio::runtime::Handle::current().block_on(self.do_publish_if_ready(request, token))
    }
}

impl<R: crate::youtube_upload::AccessTokenResolver> LiveTwitterXUploadClient<R> {
    pub fn new(token_resolver: R) -> Self {
        Self::with_base(token_resolver, TWITTER_X_API_BASE.to_string())
    }

    pub fn with_base(token_resolver: R, api_base: String) -> Self {
        Self {
            token_resolver,
            api_base,
            http: reqwest::Client::new(),
            max_video_bytes: TWITTER_X_VIDEO_MAX_BYTES,
        }
    }

    #[cfg(test)]
    fn with_base_and_max_video_bytes(
        token_resolver: R,
        api_base: String,
        max_video_bytes: u64,
    ) -> Self {
        Self {
            token_resolver,
            api_base,
            http: reqwest::Client::new(),
            max_video_bytes,
        }
    }

    async fn do_create_post_with_media(
        &self,
        request: &TwitterXUploadRequest,
        token: String,
    ) -> Result<TwitterXUploadResponse, TwitterXUploadClientError> {
        let media = self.fetch_media(&request.media_url).await?;
        let media_id = self.init_media(media.len(), &token).await?;
        self.append_media(&media_id, media, &token).await?;
        let processing = self.finalize_media(&media_id, &token).await?;
        if processing {
            return Ok(TwitterXUploadResponse {
                media_id,
                post_id: None,
                post_url: None,
                processing: true,
            });
        }
        let (post_id, post_url) =
            create_tweet(&self.http, &self.api_base, &request.text, &media_id, &token).await?;
        Ok(TwitterXUploadResponse {
            media_id,
            post_id: Some(post_id),
            post_url: Some(post_url),
            processing: false,
        })
    }

    async fn fetch_media(&self, media_url: &str) -> Result<Vec<u8>, TwitterXUploadClientError> {
        let resp = self
            .http
            .get(media_url)
            .send()
            .await
            .map_err(|e| TwitterXUploadClientError::NetworkOrServer(e.to_string()))?;
        let status = resp.status().as_u16();
        if !http_success(status) {
            return Err(TwitterXUploadClientError::NetworkOrServer(format!(
                "twitter_x media fetch {status}"
            )));
        }
        if let Some(total_bytes) = declared_content_length(&resp)
            && total_bytes > self.max_video_bytes
        {
            return Err(twitter_x_size_error(self.max_video_bytes, total_bytes));
        }
        let media = resp
            .bytes()
            .await
            .map(|bytes| bytes.to_vec())
            .map_err(|e| TwitterXUploadClientError::NetworkOrServer(e.to_string()))?;
        if media.len() as u64 > self.max_video_bytes {
            return Err(twitter_x_size_error(
                self.max_video_bytes,
                media.len() as u64,
            ));
        }
        Ok(media)
    }

    async fn init_media(
        &self,
        total_bytes: usize,
        token: &str,
    ) -> Result<String, TwitterXUploadClientError> {
        let url = format!(
            "{}/2/media/upload/initialize",
            self.api_base.trim_end_matches('/')
        );
        let json =
            self.send_media_request(self.http.post(url).bearer_auth(token).json(
                &serde_json::json!({
                    "media_type": TWITTER_X_VIDEO_MEDIA_TYPE,
                    "total_bytes": total_bytes,
                    "media_category": TWITTER_X_VIDEO_MEDIA_CATEGORY,
                }),
            ))
            .await?;
        json["data"]["id"]
            .as_str()
            .map(ToOwned::to_owned)
            .ok_or_else(|| {
                TwitterXUploadClientError::NetworkOrServer(
                    "twitter_x INIT response missing media id".into(),
                )
            })
    }

    async fn append_media(
        &self,
        media_id: &str,
        media: Vec<u8>,
        token: &str,
    ) -> Result<(), TwitterXUploadClientError> {
        let url = format!(
            "{}/2/media/upload/{}/append",
            self.api_base.trim_end_matches('/'),
            media_id
        );
        let form = reqwest::multipart::Form::new()
            .text("segment_index", "0")
            .part(
                "media",
                reqwest::multipart::Part::bytes(media).file_name("render.mp4"),
            );
        let _ = self
            .send_media_request(self.http.post(url).bearer_auth(token).multipart(form))
            .await?;
        Ok(())
    }

    async fn finalize_media(
        &self,
        media_id: &str,
        token: &str,
    ) -> Result<bool, TwitterXUploadClientError> {
        let url = format!(
            "{}/2/media/upload/{}/finalize",
            self.api_base.trim_end_matches('/'),
            media_id
        );
        let json = self
            .send_media_request(self.http.post(url).bearer_auth(token))
            .await?;
        let state = json["data"]["processing_info"]["state"]
            .as_str()
            .unwrap_or("succeeded");
        if state == "failed" {
            return Err(TwitterXUploadClientError::NetworkOrServer(format!(
                "twitter_x media processing failed: {}",
                twitter_x_processing_error_reason(&json)
            )));
        }
        Ok(matches!(state, "pending" | "in_progress"))
    }

    async fn send_media_request(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<serde_json::Value, TwitterXUploadClientError> {
        let resp = request
            .send()
            .await
            .map_err(|e| TwitterXUploadClientError::NetworkOrServer(e.to_string()))?;
        let status = resp.status().as_u16();
        if status == 204 {
            return Ok(serde_json::json!({}));
        }
        let json: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| TwitterXUploadClientError::NetworkOrServer(e.to_string()))?;
        if let Some(error) = twitter_x_error(status, &json) {
            return Err(error);
        }
        Ok(json)
    }
}

impl<R: crate::youtube_upload::AccessTokenResolver> TwitterXUploadClient
    for LiveTwitterXUploadClient<R>
{
    fn create_post_with_media(
        &self,
        request: &TwitterXUploadRequest,
    ) -> Result<TwitterXUploadResponse, TwitterXUploadClientError> {
        let token = self
            .token_resolver
            .bearer_for(&request.access_token_ref)
            .map_err(|e| TwitterXUploadClientError::NetworkOrServer(e.to_string()))?;
        tokio::runtime::Handle::current().block_on(self.do_create_post_with_media(request, token))
    }
}

fn twitter_x_error(status: u16, json: &serde_json::Value) -> Option<TwitterXUploadClientError> {
    if status == 401 || status == 403 {
        return Some(TwitterXUploadClientError::MissingScope);
    }
    if status == 429 {
        return Some(TwitterXUploadClientError::RateLimited);
    }
    if !http_success(status) {
        return Some(TwitterXUploadClientError::NetworkOrServer(format!(
            "twitter_x api {status}: {json}"
        )));
    }
    None
}

fn twitter_x_size_error(max_bytes: u64, actual_bytes: u64) -> TwitterXUploadClientError {
    TwitterXUploadClientError::NetworkOrServer(format!(
        "twitter_x tweet_video supports media up to {max_bytes} bytes; got {actual_bytes}"
    ))
}

fn declared_content_length(resp: &reqwest::Response) -> Option<u64> {
    resp.headers()
        .get(reqwest::header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .or_else(|| resp.content_length())
}

fn twitter_x_processing_error_reason(json: &serde_json::Value) -> String {
    json["data"]["processing_info"]["error"]["name"]
        .as_str()
        .or_else(|| json["data"]["processing_info"]["error"]["code"].as_str())
        .unwrap_or("unknown")
        .to_string()
}

async fn create_tweet(
    http: &reqwest::Client,
    api_base: &str,
    text: &str,
    media_id: &str,
    token: &str,
) -> Result<(String, String), TwitterXUploadClientError> {
    let url = format!("{}/2/tweets", api_base.trim_end_matches('/'));
    let resp = http
        .post(url)
        .bearer_auth(token)
        .json(&serde_json::json!({
            "text": text,
            "media": { "media_ids": [media_id] }
        }))
        .send()
        .await
        .map_err(|e| TwitterXUploadClientError::NetworkOrServer(e.to_string()))?;
    let status = resp.status().as_u16();
    let json: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| TwitterXUploadClientError::NetworkOrServer(e.to_string()))?;
    if let Some(error) = twitter_x_error(status, &json) {
        return Err(error);
    }
    let post_id = json["data"]["id"]
        .as_str()
        .ok_or_else(|| {
            TwitterXUploadClientError::NetworkOrServer(
                "twitter_x create post response missing id".into(),
            )
        })?
        .to_string();
    let post_url = format!("https://x.com/i/web/status/{post_id}");
    Ok((post_id, post_url))
}

fn twitter_x_upload_to_status_error(error: TwitterXUploadClientError) -> TwitterXStatusClientError {
    match error {
        TwitterXUploadClientError::RateLimited => TwitterXStatusClientError::RateLimited,
        TwitterXUploadClientError::MissingScope => {
            TwitterXStatusClientError::NetworkOrServer("missing_scope".into())
        }
        TwitterXUploadClientError::NetworkOrServer(message) => {
            TwitterXStatusClientError::NetworkOrServer(message)
        }
    }
}

fn http_success(status: u16) -> bool {
    (200..300).contains(&status)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::upload_adapter::{UploadAdapter, UploadPrivacy, UploadRequest};
    use crate::upload_status::{UploadProcessingStatus, UploadStatusAdapter, UploadStatusRequest};
    use crate::youtube_upload::FixedTokenResolver;
    use wiremock::matchers::{
        bearer_token, body_json, body_string_contains, method, path, query_param,
    };
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn request() -> UploadRequest {
        UploadRequest {
            job_id: "job_1".into(),
            provider: crate::model::Provider::TwitterX,
            connected_account_id: "acct_1".into(),
            artifact_ref: "https://storage.example/render.mp4".into(),
            title: "Launch clip".into(),
            description: Some("Description".into()),
            tags: vec![],
            thumbnail_ref: None,
            privacy: UploadPrivacy::Public,
            tiktok_interactions: Default::default(),
            scheduled_for: Some(2_000),
            access_token_ref: "token-secret-ref".into(),
        }
    }

    #[tokio::test]
    async fn live_twitter_x_upload_client_uploads_media_and_creates_post() {
        let media = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/render.mp4"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![1, 2, 3, 4]))
            .mount(&media)
            .await;

        let api = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/2/media/upload/initialize"))
            .and(bearer_token("x-access"))
            .and(body_json(serde_json::json!({
                "media_type": "video/mp4",
                "total_bytes": 4,
                "media_category": "tweet_video"
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": { "id": "media_123", "media_key": "13_media_123" }
            })))
            .mount(&api)
            .await;
        Mock::given(method("POST"))
            .and(path("/2/media/upload/media_123/append"))
            .and(bearer_token("x-access"))
            .and(body_string_contains("segment_index"))
            .respond_with(ResponseTemplate::new(204))
            .mount(&api)
            .await;
        Mock::given(method("POST"))
            .and(path("/2/media/upload/media_123/finalize"))
            .and(bearer_token("x-access"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": { "id": "media_123" }
            })))
            .mount(&api)
            .await;
        Mock::given(method("POST"))
            .and(path("/2/tweets"))
            .and(bearer_token("x-access"))
            .and(body_json(serde_json::json!({
                "text": "Launch clip",
                "media": { "media_ids": ["media_123"] }
            })))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "data": { "id": "tweet_123", "text": "Launch clip" }
            })))
            .mount(&api)
            .await;

        let client =
            LiveTwitterXUploadClient::with_base(FixedTokenResolver("x-access".into()), api.uri());
        let response = tokio::task::spawn_blocking(move || {
            client.create_post_with_media(&TwitterXUploadRequest {
                media_url: format!("{}/render.mp4", media.uri()),
                text: "Launch clip".into(),
                access_token_ref: "token_secret:acct_1".into(),
            })
        })
        .await
        .unwrap()
        .unwrap();

        assert_eq!(response.post_id.as_deref(), Some("tweet_123"));
        assert_eq!(
            response.post_url.as_deref(),
            Some("https://x.com/i/web/status/tweet_123")
        );
        assert!(!response.processing);
    }

    #[tokio::test]
    async fn live_twitter_x_upload_client_rejects_failed_media_processing_before_tweet() {
        let media = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/render.mp4"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![1, 2, 3, 4]))
            .mount(&media)
            .await;

        let api = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/2/media/upload/initialize"))
            .and(bearer_token("x-access"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": { "id": "media_123", "media_key": "13_media_123" }
            })))
            .mount(&api)
            .await;
        Mock::given(method("POST"))
            .and(path("/2/media/upload/media_123/append"))
            .and(bearer_token("x-access"))
            .respond_with(ResponseTemplate::new(204))
            .mount(&api)
            .await;
        Mock::given(method("POST"))
            .and(path("/2/media/upload/media_123/finalize"))
            .and(bearer_token("x-access"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": {
                    "id": "media_123",
                    "processing_info": {
                        "state": "failed",
                        "error": { "code": "InvalidMedia", "name": "invalid_media" }
                    }
                }
            })))
            .mount(&api)
            .await;
        Mock::given(method("POST"))
            .and(path("/2/tweets"))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "data": { "id": "tweet_123", "text": "Launch clip" }
            })))
            .expect(0)
            .mount(&api)
            .await;

        let client =
            LiveTwitterXUploadClient::with_base(FixedTokenResolver("x-access".into()), api.uri());
        let response = tokio::task::spawn_blocking(move || {
            client.create_post_with_media(&TwitterXUploadRequest {
                media_url: format!("{}/render.mp4", media.uri()),
                text: "Launch clip".into(),
                access_token_ref: "token_secret:acct_1".into(),
            })
        })
        .await
        .unwrap();

        assert_eq!(
            response,
            Err(TwitterXUploadClientError::NetworkOrServer(
                "twitter_x media processing failed: invalid_media".into()
            ))
        );
    }

    #[tokio::test]
    async fn live_twitter_x_upload_client_rejects_oversized_media_before_init() {
        let media = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/render.mp4"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![1, 2, 3, 4]))
            .mount(&media)
            .await;

        let api = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/2/media/upload/initialize"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": { "id": "should_not_init" }
            })))
            .expect(0)
            .mount(&api)
            .await;

        let client = LiveTwitterXUploadClient::with_base_and_max_video_bytes(
            FixedTokenResolver("x-access".into()),
            api.uri(),
            3,
        );
        let response = tokio::task::spawn_blocking(move || {
            client.create_post_with_media(&TwitterXUploadRequest {
                media_url: format!("{}/render.mp4", media.uri()),
                text: "Launch clip".into(),
                access_token_ref: "token_secret:acct_1".into(),
            })
        })
        .await
        .unwrap();

        assert_eq!(
            response,
            Err(TwitterXUploadClientError::NetworkOrServer(format!(
                "twitter_x tweet_video supports media up to {} bytes; got {}",
                3, 4
            )))
        );
    }

    #[test]
    fn adapter_maps_upload_response_to_published_post() {
        let adapter = TwitterXUploadAdapter::new(StubTwitterXUploadClient {
            response: TwitterXUploadResponse {
                media_id: "media_123".into(),
                post_id: Some("tweet_123".into()),
                post_url: Some("https://x.com/i/web/status/tweet_123".into()),
                processing: false,
            },
        });

        let result = adapter.upload(&request()).unwrap();

        assert_eq!(result.provider_post_id, "tweet_123");
        assert_eq!(
            result.provider_post_url,
            "https://x.com/i/web/status/tweet_123"
        );
        assert!(!result.processing);
    }

    #[test]
    fn adapter_returns_processing_ref_when_media_is_still_processing() {
        let adapter = TwitterXUploadAdapter::new(StubTwitterXUploadClient {
            response: TwitterXUploadResponse {
                media_id: "media_123".into(),
                post_id: None,
                post_url: None,
                processing: true,
            },
        });

        let result = adapter.upload(&request()).unwrap();

        assert!(result.processing);
        assert_eq!(
            TwitterXProcessingRef::decode(&result.provider_post_id).unwrap(),
            TwitterXProcessingRef {
                media_id: "media_123".into(),
                text: "Launch clip".into()
            }
        );
    }

    #[tokio::test]
    async fn live_twitter_x_status_client_creates_post_after_media_succeeds() {
        let api = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/2/media/upload"))
            .and(query_param("media_id", "media_123"))
            .and(bearer_token("x-access"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": {
                    "id": "media_123",
                    "processing_info": { "state": "succeeded" }
                }
            })))
            .mount(&api)
            .await;
        Mock::given(method("POST"))
            .and(path("/2/tweets"))
            .and(bearer_token("x-access"))
            .and(body_json(serde_json::json!({
                "text": "Launch clip",
                "media": { "media_ids": ["media_123"] }
            })))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "data": { "id": "tweet_123", "text": "Launch clip" }
            })))
            .mount(&api)
            .await;

        let client =
            LiveTwitterXStatusClient::with_base(FixedTokenResolver("x-access".into()), api.uri());
        let response = tokio::task::spawn_blocking(move || {
            client.publish_if_ready(&TwitterXStatusRequest {
                processing_ref: TwitterXProcessingRef {
                    media_id: "media_123".into(),
                    text: "Launch clip".into(),
                },
                access_token_ref: "token_secret:acct_1".into(),
            })
        })
        .await
        .unwrap()
        .unwrap();

        assert_eq!(response.state, TwitterXMediaState::Published);
        assert_eq!(response.post_id.as_deref(), Some("tweet_123"));
        assert_eq!(
            response.post_url.as_deref(),
            Some("https://x.com/i/web/status/tweet_123")
        );
    }

    #[test]
    fn status_adapter_publishes_final_post_id_from_processing_ref() {
        let processing_ref = TwitterXProcessingRef {
            media_id: "media_123".into(),
            text: "Launch clip".into(),
        }
        .encode();
        let adapter = TwitterXStatusAdapter::new(StubTwitterXStatusClient {
            response: TwitterXStatusResponse {
                state: TwitterXMediaState::Published,
                post_id: Some("tweet_123".into()),
                post_url: Some("https://x.com/i/web/status/tweet_123".into()),
            },
        });

        let result = adapter
            .poll_status(&UploadStatusRequest {
                job_id: "job_1".into(),
                provider: crate::model::Provider::TwitterX,
                connected_account_id: "acct_1".into(),
                provider_post_id: processing_ref,
                access_token_ref: "token_secret:acct_1".into(),
            })
            .unwrap();

        assert_eq!(result.status, UploadProcessingStatus::Published);
        assert_eq!(result.provider_post_id, "tweet_123");
        assert_eq!(
            result.provider_post_url.as_deref(),
            Some("https://x.com/i/web/status/tweet_123")
        );
    }

    struct StubTwitterXUploadClient {
        response: TwitterXUploadResponse,
    }

    impl TwitterXUploadClient for StubTwitterXUploadClient {
        fn create_post_with_media(
            &self,
            _request: &TwitterXUploadRequest,
        ) -> Result<TwitterXUploadResponse, TwitterXUploadClientError> {
            Ok(self.response.clone())
        }
    }

    struct StubTwitterXStatusClient {
        response: TwitterXStatusResponse,
    }

    impl TwitterXStatusClient for StubTwitterXStatusClient {
        fn publish_if_ready(
            &self,
            _request: &TwitterXStatusRequest,
        ) -> Result<TwitterXStatusResponse, TwitterXStatusClientError> {
            Ok(self.response.clone())
        }
    }
}
