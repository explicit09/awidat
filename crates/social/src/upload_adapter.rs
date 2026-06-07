use crate::model::Provider;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UploadPrivacy {
    Private,
    Unlisted,
    Public,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TikTokInteractionSettings {
    pub disable_duet: bool,
    pub disable_comment: bool,
    pub disable_stitch: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UploadRequest {
    pub job_id: String,
    pub provider: Provider,
    pub connected_account_id: String,
    pub artifact_ref: String,
    pub title: String,
    pub description: Option<String>,
    pub tags: Vec<String>,
    pub thumbnail_ref: Option<String>,
    pub privacy: UploadPrivacy,
    #[serde(default)]
    pub tiktok_interactions: TikTokInteractionSettings,
    pub scheduled_for: Option<i64>,
    #[serde(rename = "token_ref")]
    pub access_token_ref: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UploadResult {
    pub provider_post_id: String,
    pub provider_post_url: String,
    pub processing: bool,
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum UploadAdapterError {
    #[error("provider mismatch")]
    ProviderMismatch,
    #[error("missing upload token")]
    MissingUploadToken,
    #[error("media constraint failed: {reason}")]
    MediaConstraintFailed { reason: String },
    #[error("requires action: {reason}")]
    RequiresAction { reason: String },
    #[error("network or server error: {message}")]
    NetworkOrServer { message: String },
}

pub trait UploadAdapter {
    fn provider(&self) -> Provider;
    fn upload(&self, request: &UploadRequest) -> Result<UploadResult, UploadAdapterError>;
}

#[derive(Clone, Debug)]
pub struct MockUploadAdapter {
    provider: Provider,
    result: Result<UploadResult, UploadAdapterError>,
}

impl MockUploadAdapter {
    pub fn published(
        provider: Provider,
        provider_post_id: impl Into<String>,
        provider_post_url: impl Into<String>,
    ) -> Self {
        Self {
            provider,
            result: Ok(UploadResult {
                provider_post_id: provider_post_id.into(),
                provider_post_url: provider_post_url.into(),
                processing: false,
            }),
        }
    }

    pub fn failing(provider: Provider, error: UploadAdapterError) -> Self {
        Self {
            provider,
            result: Err(error),
        }
    }
}

impl UploadAdapter for MockUploadAdapter {
    fn provider(&self) -> Provider {
        self.provider.clone()
    }

    fn upload(&self, request: &UploadRequest) -> Result<UploadResult, UploadAdapterError> {
        if request.provider != self.provider {
            return Err(UploadAdapterError::ProviderMismatch);
        }
        self.result.clone()
    }
}

#[derive(Clone, Debug)]
pub struct BlockedUploadAdapter {
    provider: Provider,
    reason: String,
}

impl BlockedUploadAdapter {
    pub fn new(provider: Provider, reason: impl Into<String>) -> Self {
        Self {
            provider,
            reason: reason.into(),
        }
    }
}

impl UploadAdapter for BlockedUploadAdapter {
    fn provider(&self) -> Provider {
        self.provider.clone()
    }

    fn upload(&self, request: &UploadRequest) -> Result<UploadResult, UploadAdapterError> {
        if request.provider != self.provider {
            return Err(UploadAdapterError::ProviderMismatch);
        }
        Err(UploadAdapterError::RequiresAction {
            reason: self.reason.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Provider;

    #[test]
    fn mock_adapter_returns_published_post_without_token_material() {
        let adapter = MockUploadAdapter::published(
            Provider::YouTube,
            "video_123",
            "https://youtube.com/watch?v=video_123",
        );
        let request = UploadRequest {
            job_id: "job_1".into(),
            provider: Provider::YouTube,
            connected_account_id: "acct_1".into(),
            artifact_ref: "file:///tmp/render.mp4".into(),
            title: "Launch clip".into(),
            description: Some("Description".into()),
            tags: vec!["awidat".into()],
            thumbnail_ref: Some("file:///tmp/thumb.jpg".into()),
            privacy: UploadPrivacy::Private,
            tiktok_interactions: Default::default(),
            scheduled_for: Some(2_000),
            access_token_ref: "token-secret-ref".into(),
        };

        let result = adapter.upload(&request).unwrap_or_else(|err| {
            panic!("upload through mock adapter: {err:?}");
        });

        assert_eq!(result.provider_post_id, "video_123");
        assert_eq!(
            result.provider_post_url,
            "https://youtube.com/watch?v=video_123"
        );
        let json = serde_json::to_string(&request)
            .unwrap_or_else(|err| panic!("serialize upload request: {err}"));
        assert!(json.contains("token-secret-ref"));
        assert!(!json.contains("access_token"));
        assert!(!json.contains("refresh_token"));
    }

    #[test]
    fn blocked_adapter_maps_to_requires_action() {
        let adapter =
            BlockedUploadAdapter::new(Provider::TikTok, "tiktok_direct_post_permission_required");
        let request = UploadRequest {
            job_id: "job_1".into(),
            provider: Provider::TikTok,
            connected_account_id: "acct_1".into(),
            artifact_ref: "file:///tmp/render.mp4".into(),
            title: "Launch clip".into(),
            description: None,
            tags: Vec::new(),
            thumbnail_ref: None,
            privacy: UploadPrivacy::Private,
            tiktok_interactions: Default::default(),
            scheduled_for: None,
            access_token_ref: "token-secret-ref".into(),
        };

        assert_eq!(
            adapter.upload(&request),
            Err(UploadAdapterError::RequiresAction {
                reason: "tiktok_direct_post_permission_required".into(),
            })
        );
    }
}
