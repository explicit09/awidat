use crate::model::{AccountEligibility, AccountKind, Provider, ProviderCapabilities};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderAccountProfile {
    pub provider: Provider,
    pub provider_account_id: String,
    pub display_name: String,
    pub handle: Option<String>,
    pub avatar_url: Option<String>,
    pub account_kind: AccountKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderEligibilityReport {
    pub profile: ProviderAccountProfile,
    pub capabilities: ProviderCapabilities,
    pub eligibility: AccountEligibility,
}

pub fn youtube_eligibility(
    provider_account_id: impl Into<String>,
    display_name: impl Into<String>,
    handle: Option<&str>,
    scopes: &[&str],
) -> ProviderEligibilityReport {
    let has_upload_scope = scopes.contains(&"https://www.googleapis.com/auth/youtube.upload");
    ProviderEligibilityReport {
        profile: ProviderAccountProfile {
            provider: Provider::YouTube,
            provider_account_id: provider_account_id.into(),
            display_name: display_name.into(),
            handle: handle.map(ToOwned::to_owned),
            avatar_url: None,
            account_kind: AccountKind::Channel,
        },
        capabilities: ProviderCapabilities {
            native_scheduling: true,
            queue_scheduling: true,
            upload_video: has_upload_scope,
            upload_thumbnail: true,
            public_posting: has_upload_scope,
            requires_user_consent: false,
        },
        eligibility: if has_upload_scope {
            AccountEligibility::eligible()
        } else {
            AccountEligibility::blocked("missing_youtube_upload_scope")
        },
    }
}

pub fn tiktok_eligibility(
    provider_account_id: impl Into<String>,
    display_name: impl Into<String>,
    scopes: &[&str],
) -> ProviderEligibilityReport {
    let has_publish = scopes.contains(&"video.publish");
    ProviderEligibilityReport {
        profile: ProviderAccountProfile {
            provider: Provider::TikTok,
            provider_account_id: provider_account_id.into(),
            display_name: display_name.into(),
            handle: None,
            avatar_url: None,
            account_kind: AccountKind::Creator,
        },
        capabilities: ProviderCapabilities {
            native_scheduling: false,
            queue_scheduling: true,
            upload_video: has_publish,
            upload_thumbnail: false,
            public_posting: has_publish,
            requires_user_consent: true,
        },
        eligibility: if has_publish {
            AccountEligibility::eligible()
        } else {
            AccountEligibility::blocked("missing_video_publish_scope")
        },
    }
}

pub fn instagram_eligibility(
    provider_account_id: impl Into<String>,
    display_name: impl Into<String>,
    is_professional: bool,
    has_content_publish_scope: bool,
) -> ProviderEligibilityReport {
    let eligibility = match (is_professional, has_content_publish_scope) {
        (false, _) => AccountEligibility::blocked("instagram_professional_account_required"),
        (true, false) => AccountEligibility::blocked("missing_instagram_content_publish_scope"),
        (true, true) => AccountEligibility::eligible(),
    };

    ProviderEligibilityReport {
        profile: ProviderAccountProfile {
            provider: Provider::Instagram,
            provider_account_id: provider_account_id.into(),
            display_name: display_name.into(),
            handle: None,
            avatar_url: None,
            account_kind: if is_professional {
                AccountKind::Professional
            } else {
                AccountKind::Unknown
            },
        },
        capabilities: ProviderCapabilities {
            native_scheduling: false,
            queue_scheduling: true,
            upload_video: is_professional && has_content_publish_scope,
            upload_thumbnail: false,
            public_posting: is_professional && has_content_publish_scope,
            requires_user_consent: false,
        },
        eligibility,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn youtube_channel_profile_is_upload_eligible() {
        let report = youtube_eligibility(
            "channel_1",
            "Awidat",
            Some("@awidat"),
            &["https://www.googleapis.com/auth/youtube.upload"],
        );
        assert!(report.eligibility.eligible);
        assert!(report.capabilities.upload_video);
        assert!(report.capabilities.native_scheduling);
        assert_eq!(report.profile.account_kind, AccountKind::Channel);
    }

    #[test]
    fn youtube_missing_upload_scope_is_ineligible() {
        let report = youtube_eligibility("channel_1", "Awidat", Some("@awidat"), &[]);
        assert!(!report.eligibility.eligible);
        assert_eq!(
            report.eligibility.reasons,
            vec!["missing_youtube_upload_scope"]
        );
        assert!(!report.capabilities.upload_video);
        assert!(!report.capabilities.public_posting);
    }

    #[test]
    fn tiktok_missing_direct_post_scope_is_requires_action() {
        let report = tiktok_eligibility("open_id_1", "Creator", &["user.info.basic"]);
        assert!(!report.eligibility.eligible);
        assert_eq!(
            report.eligibility.reasons,
            vec!["missing_video_publish_scope"]
        );
        assert!(report.capabilities.requires_user_consent);
    }

    #[test]
    fn instagram_non_professional_account_is_ineligible() {
        let report = instagram_eligibility("ig_1", "Creator", false, true);
        assert!(!report.eligibility.eligible);
        assert_eq!(
            report.eligibility.reasons,
            vec!["instagram_professional_account_required"]
        );
    }

    #[test]
    fn instagram_missing_publish_scope_is_ineligible() {
        let report = instagram_eligibility("ig_1", "Creator", true, false);
        assert!(!report.eligibility.eligible);
        assert_eq!(
            report.eligibility.reasons,
            vec!["missing_instagram_content_publish_scope"]
        );
    }
}
