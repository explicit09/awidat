use crate::eligibility::{
    instagram_eligibility, tiktok_eligibility, youtube_eligibility, ProviderEligibilityReport,
};
use crate::model::{AccountEligibility, Provider, ProviderCapabilities};
use std::collections::BTreeMap;
use thiserror::Error;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderProfile {
    pub provider_account_id: String,
    pub display_name: String,
    pub handle: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderDescriptor {
    pub provider: Provider,
    pub display_name: &'static str,
    pub scopes: Vec<&'static str>,
    pub capabilities: ProviderCapabilities,
}

pub trait SocialProviderAdapter {
    fn provider(&self) -> Provider;

    fn fetch_capabilities(&self, scopes: &[&str]) -> ProviderEligibilityReport;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MockProviderAdapter {
    provider: Provider,
    provider_account_id: String,
    display_name: String,
    handle: Option<String>,
    is_professional: bool,
}

impl MockProviderAdapter {
    pub fn youtube(
        provider_account_id: impl Into<String>,
        display_name: impl Into<String>,
        handle: impl Into<String>,
    ) -> Self {
        Self {
            provider: Provider::YouTube,
            provider_account_id: provider_account_id.into(),
            display_name: display_name.into(),
            handle: Some(handle.into()),
            is_professional: false,
        }
    }

    pub fn tiktok(provider_account_id: impl Into<String>, display_name: impl Into<String>) -> Self {
        Self {
            provider: Provider::TikTok,
            provider_account_id: provider_account_id.into(),
            display_name: display_name.into(),
            handle: None,
            is_professional: false,
        }
    }

    pub fn instagram(
        provider_account_id: impl Into<String>,
        display_name: impl Into<String>,
        is_professional: bool,
    ) -> Self {
        Self {
            provider: Provider::Instagram,
            provider_account_id: provider_account_id.into(),
            display_name: display_name.into(),
            handle: None,
            is_professional,
        }
    }
}

impl SocialProviderAdapter for MockProviderAdapter {
    fn provider(&self) -> Provider {
        self.provider.clone()
    }

    fn fetch_capabilities(&self, scopes: &[&str]) -> ProviderEligibilityReport {
        match self.provider {
            Provider::YouTube => youtube_eligibility(
                &self.provider_account_id,
                &self.display_name,
                self.handle.as_deref(),
                scopes,
            ),
            Provider::TikTok => {
                tiktok_eligibility(&self.provider_account_id, &self.display_name, scopes)
            }
            Provider::Instagram => instagram_eligibility(
                &self.provider_account_id,
                &self.display_name,
                self.is_professional,
                scopes.contains(&"instagram_content_publish"),
            ),
        }
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ProviderRegistryError {
    #[error("unknown provider: {0}")]
    UnknownProvider(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderState {
    pub descriptor: ProviderDescriptor,
    pub eligibility: AccountEligibility,
}

#[derive(Clone, Debug, Default)]
pub struct ProviderRegistry {
    providers: BTreeMap<String, ProviderState>,
}

impl ProviderRegistry {
    pub fn default_multi_platform() -> Self {
        let mut registry = Self::default();
        registry.insert(ProviderState::new(
            ProviderDescriptor {
                provider: Provider::YouTube,
                display_name: "YouTube",
                scopes: vec!["youtube.upload"],
                capabilities: ProviderCapabilities {
                    native_scheduling: true,
                    queue_scheduling: true,
                    upload_video: true,
                    upload_thumbnail: true,
                    public_posting: true,
                    requires_user_consent: false,
                },
            },
            AccountEligibility::eligible(),
        ));
        registry.insert(ProviderState::new(
            ProviderDescriptor {
                provider: Provider::TikTok,
                display_name: "TikTok",
                scopes: vec!["video.publish"],
                capabilities: ProviderCapabilities {
                    native_scheduling: false,
                    queue_scheduling: true,
                    upload_video: false,
                    upload_thumbnail: false,
                    public_posting: false,
                    requires_user_consent: true,
                },
            },
            AccountEligibility::blocked("tiktok_direct_post_permission_required"),
        ));
        registry.insert(ProviderState::new(
            ProviderDescriptor {
                provider: Provider::Instagram,
                display_name: "Instagram",
                scopes: vec!["instagram_content_publish"],
                capabilities: ProviderCapabilities {
                    native_scheduling: false,
                    queue_scheduling: true,
                    upload_video: false,
                    upload_thumbnail: false,
                    public_posting: false,
                    requires_user_consent: false,
                },
            },
            AccountEligibility::blocked("instagram_professional_account_required"),
        ));
        registry
    }

    pub fn insert(&mut self, state: ProviderState) {
        self.providers
            .insert(state.descriptor.provider.as_str().to_string(), state);
    }

    pub fn get(&self, provider: &Provider) -> Result<&ProviderState, ProviderRegistryError> {
        self.providers
            .get(provider.as_str())
            .ok_or_else(|| ProviderRegistryError::UnknownProvider(provider.as_str().into()))
    }
}

impl ProviderState {
    fn new(descriptor: ProviderDescriptor, eligibility: AccountEligibility) -> Self {
        Self {
            descriptor,
            eligibility,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_registry_exposes_all_three_provider_slots() {
        let registry = ProviderRegistry::default_multi_platform();
        assert!(registry.get(&Provider::YouTube).is_ok());
        assert!(registry.get(&Provider::TikTok).is_ok());
        assert!(registry.get(&Provider::Instagram).is_ok());
    }

    #[test]
    fn youtube_is_upload_capable_while_tiktok_and_instagram_can_require_action() {
        let registry = ProviderRegistry::default_multi_platform();
        let youtube = registry
            .get(&Provider::YouTube)
            .unwrap_or_else(|err| panic!("youtube provider missing: {err}"));
        let tiktok = registry
            .get(&Provider::TikTok)
            .unwrap_or_else(|err| panic!("tiktok provider missing: {err}"));
        let instagram = registry
            .get(&Provider::Instagram)
            .unwrap_or_else(|err| panic!("instagram provider missing: {err}"));

        assert!(youtube.eligibility.eligible);
        assert!(youtube.descriptor.capabilities.upload_video);
        assert!(youtube.descriptor.capabilities.native_scheduling);
        assert!(!tiktok.eligibility.eligible);
        assert!(!instagram.eligibility.eligible);
        assert_eq!(
            tiktok.eligibility.reasons,
            vec!["tiktok_direct_post_permission_required"]
        );
        assert_eq!(
            instagram.eligibility.reasons,
            vec!["instagram_professional_account_required"]
        );
    }

    #[test]
    fn missing_provider_returns_unknown_provider_error() {
        let registry = ProviderRegistry::default();
        assert_eq!(
            registry.get(&Provider::YouTube),
            Err(ProviderRegistryError::UnknownProvider("youtube".into()))
        );
    }

    #[test]
    fn youtube_adapter_reports_profile_and_capabilities() {
        let adapter = MockProviderAdapter::youtube("channel_1", "Awidat", "@awidat");
        let report =
            adapter.fetch_capabilities(&["https://www.googleapis.com/auth/youtube.upload"]);

        assert_eq!(adapter.provider(), Provider::YouTube);
        assert_eq!(report.profile.provider, Provider::YouTube);
        assert_eq!(report.profile.provider_account_id, "channel_1");
        assert_eq!(report.profile.display_name, "Awidat");
        assert_eq!(report.profile.handle, Some("@awidat".into()));
        assert!(report.eligibility.eligible);
        assert!(report.capabilities.upload_video);
        assert!(report.capabilities.public_posting);
    }

    #[test]
    fn youtube_adapter_blocks_missing_upload_scope() {
        let adapter = MockProviderAdapter::youtube("channel_1", "Awidat", "@awidat");
        let report = adapter.fetch_capabilities(&[]);

        assert!(!report.eligibility.eligible);
        assert_eq!(
            report.eligibility.reasons,
            vec!["missing_youtube_upload_scope"]
        );
    }

    #[test]
    fn tiktok_adapter_reports_missing_publish_scope() {
        let adapter = MockProviderAdapter::tiktok("open_id_1", "Creator");
        let report = adapter.fetch_capabilities(&[]);

        assert!(!report.eligibility.eligible);
        assert_eq!(report.eligibility.reasons, vec!["missing_video_publish_scope"]);
        assert!(report.capabilities.requires_user_consent);
    }

    #[test]
    fn instagram_adapter_reports_professional_requirement() {
        let adapter = MockProviderAdapter::instagram("ig_1", "Creator", false);
        let report = adapter.fetch_capabilities(&["instagram_content_publish"]);

        assert!(!report.eligibility.eligible);
        assert_eq!(
            report.eligibility.reasons,
            vec!["instagram_professional_account_required"]
        );
    }

    #[test]
    fn instagram_adapter_allows_professional_publish_scope() {
        let adapter = MockProviderAdapter::instagram("ig_1", "Creator", true);
        let report = adapter.fetch_capabilities(&["instagram_content_publish"]);

        assert_eq!(adapter.provider(), Provider::Instagram);
        assert!(report.eligibility.eligible);
        assert!(report.capabilities.upload_video);
        assert!(report.capabilities.public_posting);
    }
}
