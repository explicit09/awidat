use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Provider {
    #[serde(rename = "youtube")]
    YouTube,
    #[serde(rename = "tiktok")]
    TikTok,
    #[serde(rename = "instagram")]
    Instagram,
    #[serde(rename = "twitter_x")]
    TwitterX,
}

impl Provider {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::YouTube => "youtube",
            Self::TikTok => "tiktok",
            Self::Instagram => "instagram",
            Self::TwitterX => "twitter_x",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OwnerRef {
    User(String),
    Workspace(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountKind {
    Channel,
    Creator,
    Business,
    Professional,
    Page,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectedAccountStatus {
    Connected,
    NeedsReauth,
    MissingScope,
    Ineligible,
    Disabled,
    Revoked,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderCapabilities {
    pub native_scheduling: bool,
    pub queue_scheduling: bool,
    pub upload_video: bool,
    pub upload_thumbnail: bool,
    pub public_posting: bool,
    pub requires_user_consent: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountEligibility {
    pub eligible: bool,
    pub reasons: Vec<String>,
}

impl AccountEligibility {
    pub fn eligible() -> Self {
        Self {
            eligible: true,
            reasons: Vec::new(),
        }
    }

    pub fn blocked(reason: impl Into<String>) -> Self {
        Self {
            eligible: false,
            reasons: vec![reason.into()],
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectedAccount {
    pub id: String,
    pub owner: OwnerRef,
    pub provider: Provider,
    pub provider_account_id: String,
    pub display_name: String,
    pub handle: Option<String>,
    pub avatar_url: Option<String>,
    pub account_kind: AccountKind,
    pub status: ConnectedAccountStatus,
    pub scopes: Vec<String>,
    pub capabilities: ProviderCapabilities,
    pub eligibility: AccountEligibility,
    pub last_verified_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CampaignVariantTarget {
    pub id: String,
    pub campaign_id: String,
    pub variant_id: String,
    pub connected_account_id: String,
    pub provider: Provider,
    pub platform_fields: serde_json::Value,
    pub scheduled_for: i64,
    pub validation_state: ValidationState,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationState {
    Pending,
    Valid,
    Invalid,
    RequiresAction,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PublishJobStatus {
    Draft,
    Validated,
    Scheduled,
    Uploading,
    Processing,
    Published,
    Failed,
    RequiresAction,
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishJob {
    pub id: String,
    pub campaign_id: String,
    pub variant_id: String,
    pub connected_account_id: String,
    pub provider: Provider,
    pub artifact_ref: String,
    pub idempotency_key: String,
    pub scheduled_for: i64,
    pub status: PublishJobStatus,
    pub attempt_count: u32,
    pub provider_post_id: Option<String>,
    pub provider_post_url: Option<String>,
    pub normalized_error: Option<String>,
    pub raw_error_ref: Option<String>,
    pub requires_action_reason: Option<String>,
    pub created_by: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PublishJobEventType {
    TargetBound,
    Validated,
    Scheduled,
    Claimed,
    Uploaded,
    StatusPolled,
    Cancelled,
    RetryQueued,
    Rescheduled,
    RequiresAction,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PublishJobActorType {
    User,
    System,
    Worker,
    Provider,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishJobEvent {
    pub id: String,
    pub publish_job_id: String,
    pub event_type: PublishJobEventType,
    pub actor_type: PublishJobActorType,
    pub message: String,
    pub metadata: serde_json::Value,
    pub created_at: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TeamRole {
    Owner,
    Admin,
    Publisher,
    Viewer,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TeamAction {
    ConnectAccount,
    DisconnectAccount,
    SchedulePublish,
    CancelPublish,
    RetryPublish,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceMemberRole {
    pub workspace_id: String,
    pub user_id: String,
    pub role: TeamRole,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountPublishDefaults {
    pub connected_account_id: String,
    pub default_privacy: Option<String>,
    pub default_tags: Vec<String>,
    pub title_prefix: Option<String>,
    pub description_suffix: Option<String>,
    pub updated_at: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishJobStatusCounts {
    pub scheduled: usize,
    pub processing: usize,
    pub published: usize,
    pub failed: usize,
    pub requires_action: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountUsageAudit {
    pub connected_account_id: String,
    pub owner: OwnerRef,
    pub jobs: Vec<PublishJob>,
    pub events: Vec<PublishJobEvent>,
    pub status_counts: PublishJobStatusCounts,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_keys_are_stable() {
        for provider in [
            Provider::YouTube,
            Provider::TikTok,
            Provider::Instagram,
            Provider::TwitterX,
        ] {
            let json = serde_json::to_value(&provider)
                .unwrap_or_else(|err| panic!("serialize provider: {err}"));
            assert_eq!(json, serde_json::Value::String(provider.as_str().into()));
        }
    }

    #[test]
    fn connected_account_never_contains_token_material() {
        let account = ConnectedAccount {
            id: "acct_1".into(),
            owner: OwnerRef::User("user_1".into()),
            provider: Provider::YouTube,
            provider_account_id: "channel_1".into(),
            display_name: "Montage Channel".into(),
            handle: Some("@montage".into()),
            avatar_url: None,
            account_kind: AccountKind::Channel,
            status: ConnectedAccountStatus::Connected,
            scopes: vec!["youtube.upload".into()],
            capabilities: ProviderCapabilities::default(),
            eligibility: AccountEligibility::eligible(),
            last_verified_at: None,
            created_at: 1,
            updated_at: 1,
        };

        let json = serde_json::to_string(&account)
            .unwrap_or_else(|err| panic!("serialize account: {err}"));
        assert!(json.contains("Montage Channel"));
        assert!(!json.contains("access_token"));
        assert!(!json.contains("refresh_token"));
    }
}
