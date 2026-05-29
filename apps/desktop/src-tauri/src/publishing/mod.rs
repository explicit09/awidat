//! Publishing subsystem — provider abstraction + per-platform stubs.
//!
//! Wave 5 Path A closes the render → upload → published loop. This
//! module is the scaffolding: the [`PublishingProvider`] trait, a
//! registry that owns one instance of each provider, and three stub
//! implementations (YouTube / TikTok / Instagram) that wire the
//! `is_configured` / OAuth / status surface but stop short of the
//! real HTTP upload.
//!
//! # Why stubs first
//!
//! Real upload requires the *user* (not us) to register their own
//! OAuth client at each platform's developer console:
//!
//! - YouTube: <https://console.cloud.google.com/apis/credentials>
//! - TikTok: <https://developers.tiktok.com/> (manual review)
//! - Instagram: <https://developers.facebook.com/apps> (manual review)
//!
//! Until the user does that, the OAuth flow can't complete; until
//! that flow can complete, the upload call has nothing to authenticate
//! with. The scaffolding here lets us wire the Tauri commands +
//! Settings UI now so when credentials show up later we plug into
//! existing infrastructure.

pub mod errors;
pub mod instagram;
pub mod oauth;
pub mod provider;
pub mod storage;
pub mod tiktok;
pub mod types;
pub mod youtube;

use std::path::PathBuf;
use std::sync::Arc;

pub use errors::ProviderError;
pub use provider::PublishingProvider;
pub use types::{
    ConnectionStatus, OAuthChallenge, ProviderInfo, UploadParams, UploadResult,
};
// Re-exported separately because tests are the only in-crate
// consumer today — bundling it with the line above tripped the
// "unused import" lint. Frontend deserialises `UploadParams` (which
// carries a `Visibility`) so this stays part of the public surface.
#[cfg(test)]
pub use types::Visibility;

use instagram::InstagramProvider;
use tiktok::TiktokProvider;
use youtube::YoutubeProvider;

/// Registry of every installed provider. Owns one boxed instance per
/// platform; lookups are by [`PublishingProvider::key`].
///
/// Cheap to clone — the inner provider trio is behind an `Arc` so
/// callers can stash a copy in Tauri state without doubling the
/// allocation.
#[derive(Clone)]
pub struct ProviderRegistry {
    providers: Arc<Vec<Box<dyn PublishingProvider>>>,
}

impl ProviderRegistry {
    /// Build the default registry — one YouTube + one TikTok + one
    /// Instagram provider, all backed by the platform `<config_dir>`
    /// credential file.
    pub fn new() -> Result<Self, ProviderError> {
        let path = storage::default_store_path()?;
        Ok(Self::with_store_path(path))
    }

    /// Build a registry rooted at a specific credential-file path.
    /// Used by tests and (eventually) the Settings UI when the user
    /// overrides the default location.
    pub fn with_store_path(store_path: PathBuf) -> Self {
        let providers: Vec<Box<dyn PublishingProvider>> = vec![
            Box::new(YoutubeProvider::with_store_path(store_path.clone())),
            Box::new(TiktokProvider::with_store_path(store_path.clone())),
            Box::new(InstagramProvider::with_store_path(store_path)),
        ];
        Self {
            providers: Arc::new(providers),
        }
    }

    /// Iterate every registered provider.
    pub fn iter(&self) -> impl Iterator<Item = &dyn PublishingProvider> {
        self.providers.iter().map(|b| b.as_ref())
    }

    /// Look up by stable key. Returns `None` for unknown keys — the
    /// Tauri command layer maps that to an "unsupported provider"
    /// string error so the frontend can show the right copy.
    pub fn get(&self, key: &str) -> Option<&dyn PublishingProvider> {
        self.providers
            .iter()
            .find(|p| p.key() == key)
            .map(|b| b.as_ref())
    }

    /// Build the `list_providers` payload — one `{key, display_name,
    /// configured}` row per provider.
    pub async fn list_info(&self) -> Vec<ProviderInfo> {
        let mut out = Vec::with_capacity(self.providers.len());
        for p in self.providers.iter() {
            out.push(ProviderInfo {
                key: p.key().to_string(),
                display_name: p.display_name().to_string(),
                configured: p.is_configured().await,
            });
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_store_path(tmp: &tempfile::TempDir) -> PathBuf {
        tmp.path().join("publishing.json")
    }

    /// Brief-specified test:
    /// `publishing::stub_provider_returns_not_configured` — uploading
    /// without creds errors correctly.
    #[tokio::test]
    async fn stub_provider_returns_not_configured() {
        let tmp = tempfile::tempdir().unwrap();
        let registry = ProviderRegistry::with_store_path(test_store_path(&tmp));

        // Every provider should reject upload with NotConfigured when
        // there are no stored credentials.
        for provider in registry.iter() {
            let err = provider
                .upload(UploadParams {
                    file_path: "/tmp/fake.mp4".into(),
                    title: "t".into(),
                    description: String::new(),
                    tags: vec![],
                    visibility: Visibility::Private,
                    scheduled_at: None,
                    thumbnail_path: None,
                })
                .await
                .unwrap_err();
            assert_eq!(
                err.kind(),
                "not_configured",
                "{} should reject upload without creds",
                provider.key(),
            );
        }
    }

    /// Brief-specified test:
    /// `publishing::oauth_flow_writes_token` — completing OAuth writes
    /// to storage.
    #[tokio::test]
    async fn oauth_flow_writes_token() {
        let tmp = tempfile::tempdir().unwrap();
        let path = test_store_path(&tmp);
        let registry = ProviderRegistry::with_store_path(path.clone());

        let yt = registry.get("youtube").unwrap();
        assert!(!yt.is_configured().await, "fresh registry → not configured");

        // Begin → URL has the placeholder client id + a state nonce.
        let challenge = yt.begin_oauth().await.unwrap();
        assert!(challenge.url.contains("client_id=YOUR_CLIENT_ID_HERE"));
        assert!(!challenge.state.is_empty());

        // Complete with a fake code → storage gets the placeholder token.
        yt.complete_oauth("fake-auth-code-123".into()).await.unwrap();
        assert!(yt.is_configured().await, "post-complete → configured");

        let on_disk = storage::load_from(&path).await.unwrap();
        let creds = on_disk.get("youtube").expect("youtube slot populated");
        assert!(creds.access_token.contains("fake-auth-code-123"));

        // Upload now reaches the second tier — Unsupported (not NotConfigured).
        let err = yt
            .upload(UploadParams {
                file_path: "/tmp/fake.mp4".into(),
                title: "t".into(),
                description: String::new(),
                tags: vec![],
                visibility: Visibility::Private,
                scheduled_at: None,
                thumbnail_path: None,
            })
            .await
            .unwrap_err();
        assert_eq!(err.kind(), "unsupported");
        // Sanity-check the message points at the YouTube dev console
        // — the frontend will surface that URL.
        let msg = err.to_string();
        assert!(msg.contains("console.cloud.google.com"), "{msg}");
    }

    #[tokio::test]
    async fn complete_oauth_rejects_empty_code() {
        let tmp = tempfile::tempdir().unwrap();
        let registry = ProviderRegistry::with_store_path(test_store_path(&tmp));
        let err = registry
            .get("tiktok")
            .unwrap()
            .complete_oauth(String::new())
            .await
            .unwrap_err();
        assert_eq!(err.kind(), "oauth_failed");
    }

    #[tokio::test]
    async fn list_info_returns_all_three_providers() {
        let tmp = tempfile::tempdir().unwrap();
        let registry = ProviderRegistry::with_store_path(test_store_path(&tmp));
        let info = registry.list_info().await;
        assert_eq!(info.len(), 3);
        let keys: Vec<&str> = info.iter().map(|p| p.key.as_str()).collect();
        assert!(keys.contains(&"youtube"));
        assert!(keys.contains(&"tiktok"));
        assert!(keys.contains(&"instagram"));
        // None configured on a fresh tempdir-backed registry.
        assert!(info.iter().all(|p| !p.configured));
    }

    #[tokio::test]
    async fn get_unknown_key_returns_none() {
        let tmp = tempfile::tempdir().unwrap();
        let registry = ProviderRegistry::with_store_path(test_store_path(&tmp));
        assert!(registry.get("vimeo").is_none());
    }

    #[tokio::test]
    async fn status_reflects_oauth_completion() {
        let tmp = tempfile::tempdir().unwrap();
        let registry = ProviderRegistry::with_store_path(test_store_path(&tmp));
        let ig = registry.get("instagram").unwrap();

        let before = ig.status().await;
        assert!(!before.connected);

        ig.complete_oauth("ig-code".into()).await.unwrap();

        let after = ig.status().await;
        assert!(after.connected);
    }
}
