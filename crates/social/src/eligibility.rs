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

pub fn twitter_x_eligibility(
    provider_account_id: impl Into<String>,
    display_name: impl Into<String>,
    handle: Option<&str>,
    scopes: &[&str],
) -> ProviderEligibilityReport {
    let has_write_scope = scopes.contains(&"tweet.write") && scopes.contains(&"media.write");
    ProviderEligibilityReport {
        profile: ProviderAccountProfile {
            provider: Provider::TwitterX,
            provider_account_id: provider_account_id.into(),
            display_name: display_name.into(),
            handle: handle.map(ToOwned::to_owned),
            avatar_url: None,
            account_kind: AccountKind::Creator,
        },
        capabilities: ProviderCapabilities {
            native_scheduling: false,
            queue_scheduling: true,
            upload_video: has_write_scope,
            upload_thumbnail: false,
            public_posting: has_write_scope,
            requires_user_consent: false,
        },
        eligibility: if has_write_scope {
            AccountEligibility::eligible()
        } else {
            AccountEligibility::blocked("missing_twitter_x_write_scope")
        },
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

    #[test]
    fn tiktok_with_publish_scope_is_upload_eligible_and_public_capable() {
        // A TikTok account that carries video.publish flips to eligible and
        // exposes upload_video + public_posting — the adapter threads
        // public_posting into its privacy clamp (Phase 6 Task 2).
        let report = tiktok_eligibility("open_id_1", "Creator", &["video.publish"]);
        assert!(report.eligibility.eligible);
        assert!(report.capabilities.upload_video);
        assert!(
            report.capabilities.public_posting,
            "public_posting drives the adapter's eligible_for_public clamp"
        );
        assert_eq!(report.profile.account_kind, AccountKind::Creator);
    }

    #[test]
    fn instagram_professional_with_publish_scope_is_upload_eligible() {
        let report = instagram_eligibility("ig_1", "Creator", true, true);
        assert!(report.eligibility.eligible);
        assert!(report.capabilities.upload_video);
        assert!(report.capabilities.public_posting);
        assert_eq!(report.profile.account_kind, AccountKind::Professional);
    }

    #[test]
    fn twitter_x_missing_write_scope_is_ineligible() {
        let report = twitter_x_eligibility("x_1", "Creator", Some("@awidat"), &["users.read"]);
        assert!(!report.eligibility.eligible);
        assert_eq!(
            report.eligibility.reasons,
            vec!["missing_twitter_x_write_scope"]
        );
    }

    #[test]
    fn twitter_x_requires_both_tweet_and_media_write_scopes() {
        for scopes in [
            vec!["users.read", "tweet.write"],
            vec!["users.read", "media.write"],
        ] {
            let report = twitter_x_eligibility("x_1", "Creator", Some("@awidat"), &scopes);
            assert!(!report.eligibility.eligible);
            assert!(!report.capabilities.upload_video);
            assert!(!report.capabilities.public_posting);
            assert_eq!(
                report.eligibility.reasons,
                vec!["missing_twitter_x_write_scope"]
            );
        }

        let report = twitter_x_eligibility(
            "x_1",
            "Creator",
            Some("@awidat"),
            &["users.read", "tweet.write", "media.write"],
        );
        assert!(report.eligibility.eligible);
        assert!(report.capabilities.upload_video);
        assert!(report.capabilities.public_posting);
    }
}
