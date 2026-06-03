use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Provider {
    #[serde(rename = "youtube")]
    YouTube,
    #[serde(rename = "tiktok")]
    TikTok,
    #[serde(rename = "instagram")]
    Instagram,
}

impl Provider {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::YouTube => "youtube",
            Self::TikTok => "tiktok",
            Self::Instagram => "instagram",
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_keys_are_stable() {
        for provider in [Provider::YouTube, Provider::TikTok, Provider::Instagram] {
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
            display_name: "Awidat Channel".into(),
            handle: Some("@awidat".into()),
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
        assert!(json.contains("Awidat Channel"));
        assert!(!json.contains("access_token"));
        assert!(!json.contains("refresh_token"));
    }
}
