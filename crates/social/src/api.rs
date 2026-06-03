//! Framework-neutral API facade over the social publishing domain services.
//!
//! Phase 6 exposes route-shaped Rust methods, request/response DTOs, and an
//! actor/owner authorization boundary that a future Axum, Tauri, or Next server
//! wrapper can mount directly. The facade never returns provider token material.
//!
//! The facade composes the existing domain services
//! ([`SocialAccountService`], [`PublishService`], [`UploadService`], and
//! [`UploadStatusService`]) and the Phase 5 ownership policy ([`TeamPolicy`]).
//! It does not reimplement account, publish, upload, status, or team logic.

use crate::account_service::{AccountServiceError, CompleteOAuthInput, SocialAccountService};
use crate::model::{
    AccountEligibility, AccountKind, ConnectedAccount, ConnectedAccountStatus, OwnerRef, Provider,
    ProviderCapabilities, TeamAction, WorkspaceMemberRole,
};
use crate::oauth_url::OAuthProviderConfig;
use crate::provider::{ProviderRegistry, ProviderState};
use crate::store::{SocialStore, SocialStoreError};
use crate::team_service::TeamPolicy;
use crate::token::LocalTokenKeyProvider;
use crate::token_bundle::ProviderTokenBundle;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// The authenticated Awidat caller and the workspace roles known to the caller.
///
/// This is the app-level identity. It is intentionally separate from social
/// provider OAuth credentials: it decides *which user or workspace* is making
/// the request, never *which creator account* receives a post.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiActor {
    pub user_id: String,
    pub workspace_roles: Vec<WorkspaceMemberRole>,
}

impl ApiActor {
    pub fn new(user_id: impl Into<String>, workspace_roles: Vec<WorkspaceMemberRole>) -> Self {
        Self {
            user_id: user_id.into(),
            workspace_roles,
        }
    }

    /// Returns `Ok(())` when this actor is allowed to perform `action` against
    /// resources owned by `owner`, otherwise [`SocialApiError::Unauthorized`].
    fn authorize(&self, owner: &OwnerRef, action: TeamAction) -> Result<(), SocialApiError> {
        if TeamPolicy::can_perform(owner, &self.user_id, action, &self.workspace_roles) {
            Ok(())
        } else {
            Err(SocialApiError::Unauthorized)
        }
    }
}

/// The owner a request targets: a single Awidat user or a shared workspace.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiOwner {
    pub owner: OwnerRef,
}

impl ApiOwner {
    pub fn user(user_id: impl Into<String>) -> Self {
        Self {
            owner: OwnerRef::User(user_id.into()),
        }
    }

    pub fn workspace(workspace_id: impl Into<String>) -> Self {
        Self {
            owner: OwnerRef::Workspace(workspace_id.into()),
        }
    }
}

/// API-facing error that preserves the domain error source without leaking
/// sensitive provider data. HTTP status mapping is deferred to the future
/// concrete server wrapper.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum SocialApiError {
    #[error(transparent)]
    Store(#[from] SocialStoreError),
    #[error("account error: {0}")]
    Account(String),
    #[error("publish error: {0}")]
    Publish(String),
    #[error("upload error: {0}")]
    Upload(String),
    #[error("status error: {0}")]
    Status(String),
    #[error("team error: {0}")]
    Team(String),
    #[error("caller is not authorized for this resource")]
    Unauthorized,
}

impl From<AccountServiceError> for SocialApiError {
    fn from(error: AccountServiceError) -> Self {
        match error {
            AccountServiceError::Store(store) => SocialApiError::Store(store),
            other => SocialApiError::Account(other.to_string()),
        }
    }
}

/// Provider slot summary, safe to serialize to clients.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderSummary {
    pub provider: Provider,
    pub display_name: String,
    pub scopes: Vec<String>,
    pub capabilities: ProviderCapabilities,
    pub eligibility: AccountEligibility,
}

impl ProviderSummary {
    fn from_state(state: &ProviderState) -> Self {
        Self {
            provider: state.descriptor.provider.clone(),
            display_name: state.descriptor.display_name.to_string(),
            scopes: state
                .descriptor
                .scopes
                .iter()
                .map(|scope| (*scope).to_string())
                .collect(),
            capabilities: state.descriptor.capabilities.clone(),
            eligibility: state.eligibility.clone(),
        }
    }
}

/// Connected account display data, safe to serialize to clients.
///
/// This deliberately mirrors only the non-secret fields of
/// [`ConnectedAccount`]; it never carries token rows, KMS ids, or OAuth state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountSummary {
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

impl From<ConnectedAccount> for AccountSummary {
    fn from(account: ConnectedAccount) -> Self {
        Self {
            id: account.id,
            owner: account.owner,
            provider: account.provider,
            provider_account_id: account.provider_account_id,
            display_name: account.display_name,
            handle: account.handle,
            avatar_url: account.avatar_url,
            account_kind: account.account_kind,
            status: account.status,
            scopes: account.scopes,
            capabilities: account.capabilities,
            eligibility: account.eligibility,
            last_verified_at: account.last_verified_at,
            created_at: account.created_at,
            updated_at: account.updated_at,
        }
    }
}

/// Request to begin a provider OAuth connection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OAuthStartRequest {
    pub oauth_connection_id: String,
    pub owner: OwnerRef,
    pub provider: Provider,
    pub config: OAuthProviderConfig,
    pub raw_state: String,
    pub return_to: String,
    pub created_at: i64,
    pub expires_at: i64,
}

/// Response after a provider OAuth start, carrying the authorization URL.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OAuthStartResponse {
    pub oauth_connection_id: String,
    pub provider: Provider,
    pub authorization_url: String,
}

/// Request to complete a provider OAuth callback.
///
/// The server wrapper supplies the freshly exchanged tokens; the facade only
/// forwards them to the account service, which encrypts them at rest. The
/// tokens never appear in any response.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OAuthCompleteRequest {
    pub oauth_connection_id: String,
    pub owner: OwnerRef,
    pub raw_state: String,
    pub connected_account: ConnectedAccount,
    pub token_bundle: ProviderTokenBundle,
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub now: i64,
}

/// Response after completing OAuth, carrying only sanitized account data.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OAuthCompleteResponse {
    pub account: AccountSummary,
}

/// Static, framework-neutral API facade.
///
/// Like the existing domain services, `SocialApi` is not an instantiated server
/// object; it is a namespace of route-shaped associated functions.
pub struct SocialApi;

impl SocialApi {
    /// `GET /social/providers`: list provider slots and capability summaries.
    pub fn providers(registry: &ProviderRegistry) -> Vec<ProviderSummary> {
        [Provider::YouTube, Provider::TikTok, Provider::Instagram]
            .iter()
            .filter_map(|provider| registry.get(provider).ok())
            .map(ProviderSummary::from_state)
            .collect()
    }

    /// `GET /social/accounts`: list connected accounts for an owner.
    pub fn accounts(
        store: &impl SocialStore,
        actor: &ApiActor,
        owner: &ApiOwner,
    ) -> Result<Vec<AccountSummary>, SocialApiError> {
        // Listing is a read; the same identity gate used for management applies
        // so that only the owning user or a workspace member can enumerate.
        actor.authorize(&owner.owner, TeamAction::ConnectAccount)?;
        let accounts = SocialAccountService::list_accounts(store, &owner.owner)?;
        Ok(accounts.into_iter().map(AccountSummary::from).collect())
    }

    /// `POST /social/oauth/:provider/start`: begin a provider OAuth connection.
    pub fn oauth_start(
        store: &mut impl SocialStore,
        actor: &ApiActor,
        request: OAuthStartRequest,
    ) -> Result<OAuthStartResponse, SocialApiError> {
        actor.authorize(&request.owner, TeamAction::ConnectAccount)?;
        let authorize = SocialAccountService::start_oauth(
            store,
            request.oauth_connection_id.clone(),
            request.owner,
            request.provider.clone(),
            &request.config,
            &request.raw_state,
            request.return_to,
            request.created_at,
            request.expires_at,
        )?;
        Ok(OAuthStartResponse {
            oauth_connection_id: authorize.connection.id,
            provider: request.provider,
            authorization_url: authorize.authorization_url,
        })
    }

    /// `GET /social/oauth/:provider/callback`: complete a provider OAuth flow.
    pub fn oauth_complete(
        store: &mut impl SocialStore,
        key_provider: &impl LocalTokenKeyProvider,
        actor: &ApiActor,
        request: OAuthCompleteRequest,
    ) -> Result<OAuthCompleteResponse, SocialApiError> {
        actor.authorize(&request.owner, TeamAction::ConnectAccount)?;
        let account = SocialAccountService::complete_oauth(
            store,
            key_provider,
            CompleteOAuthInput {
                oauth_connection_id: request.oauth_connection_id,
                raw_state: request.raw_state,
                connected_account: request.connected_account,
                token_bundle: request.token_bundle,
                access_token: request.access_token,
                refresh_token: request.refresh_token,
                now: request.now,
            },
        )?;
        Ok(OAuthCompleteResponse {
            account: account.into(),
        })
    }

    /// `DELETE /social/accounts/:id`: disconnect (disable) a connected account.
    pub fn disconnect_account(
        store: &mut impl SocialStore,
        actor: &ApiActor,
        owner: &ApiOwner,
        account_id: &str,
        now: i64,
    ) -> Result<AccountSummary, SocialApiError> {
        actor.authorize(&owner.owner, TeamAction::DisconnectAccount)?;
        let account =
            SocialAccountService::disconnect_account(store, account_id, &owner.owner, now)?;
        Ok(account.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        AccountEligibility, AccountKind, ConnectedAccount, ConnectedAccountStatus, OwnerRef,
        Provider, ProviderCapabilities, TeamRole, WorkspaceMemberRole,
    };
    use crate::oauth::OAuthConnectionStatus;
    use crate::provider::ProviderRegistry;
    use crate::store::{InMemorySocialStore, SocialStore};
    use crate::token::TestKeyProvider;
    use crate::token_bundle::ProviderTokenBundle;

    fn user_owner() -> OwnerRef {
        OwnerRef::User("user_1".into())
    }

    fn user_actor() -> ApiActor {
        ApiActor::new("user_1", Vec::new())
    }

    fn other_user_actor() -> ApiActor {
        ApiActor::new("user_2", Vec::new())
    }

    fn config() -> OAuthProviderConfig {
        OAuthProviderConfig {
            client_id: "client_123".into(),
            redirect_uri: "https://app.awidat.test/social/oauth/callback".into(),
        }
    }

    fn connected_account(id: &str, owner: OwnerRef) -> ConnectedAccount {
        ConnectedAccount {
            id: id.into(),
            owner,
            provider: Provider::YouTube,
            provider_account_id: "channel_1".into(),
            display_name: "Awidat Channel".into(),
            handle: Some("@awidat".into()),
            avatar_url: None,
            account_kind: AccountKind::Channel,
            status: ConnectedAccountStatus::Connected,
            scopes: vec!["https://www.googleapis.com/auth/youtube.upload".into()],
            capabilities: ProviderCapabilities {
                upload_video: true,
                upload_thumbnail: true,
                public_posting: true,
                ..ProviderCapabilities::default()
            },
            eligibility: AccountEligibility::eligible(),
            last_verified_at: None,
            created_at: 100,
            updated_at: 100,
        }
    }

    fn token_bundle() -> ProviderTokenBundle {
        ProviderTokenBundle {
            provider: Provider::YouTube,
            provider_account_id: "channel_1".into(),
            scopes: vec!["https://www.googleapis.com/auth/youtube.upload".into()],
            access_token_expires_at: 4_700,
            refresh_token_expires_at: Some(8_700),
        }
    }

    fn start_request() -> OAuthStartRequest {
        OAuthStartRequest {
            oauth_connection_id: "oauth_1".into(),
            owner: user_owner(),
            provider: Provider::YouTube,
            config: config(),
            raw_state: "state-secret".into(),
            return_to: "/campaigns/campaign_1".into(),
            created_at: 100,
            expires_at: 2_000,
        }
    }

    fn complete_request() -> OAuthCompleteRequest {
        OAuthCompleteRequest {
            oauth_connection_id: "oauth_1".into(),
            owner: user_owner(),
            raw_state: "state-secret".into(),
            connected_account: connected_account("acct_1", user_owner()),
            token_bundle: token_bundle(),
            access_token: "access-secret".into(),
            refresh_token: Some("refresh-secret".into()),
            now: 1_000,
        }
    }

    #[test]
    fn account_api_lists_providers_and_accounts_without_tokens() {
        let registry = ProviderRegistry::default_multi_platform();
        let providers = SocialApi::providers(&registry);
        let provider_ids: Vec<&Provider> = providers.iter().map(|p| &p.provider).collect();
        assert_eq!(
            provider_ids,
            vec![&Provider::YouTube, &Provider::TikTok, &Provider::Instagram]
        );

        let mut store = InMemorySocialStore::default();
        store
            .save_connected_account(connected_account("acct_1", user_owner()))
            .unwrap_or_else(|err| panic!("save account: {err}"));
        store
            .save_connected_account(connected_account(
                "acct_2",
                OwnerRef::Workspace("workspace_1".into()),
            ))
            .unwrap_or_else(|err| panic!("save other account: {err}"));

        let accounts = SocialApi::accounts(&store, &user_actor(), &ApiOwner { owner: user_owner() })
            .unwrap_or_else(|err| panic!("list accounts: {err}"));

        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].id, "acct_1");
        assert_eq!(accounts[0].display_name, "Awidat Channel");

        let json = serde_json::to_string(&accounts)
            .unwrap_or_else(|err| panic!("serialize accounts: {err}"));
        assert!(!json.contains("access_token"));
        assert!(!json.contains("refresh_token"));
    }

    #[test]
    fn account_api_lists_accounts_rejects_foreign_user() {
        let store = InMemorySocialStore::default();
        assert_eq!(
            SocialApi::accounts(&store, &other_user_actor(), &ApiOwner { owner: user_owner() }),
            Err(SocialApiError::Unauthorized)
        );
    }

    #[test]
    fn account_api_starts_and_completes_oauth() {
        let mut store = InMemorySocialStore::default();
        let key_provider = TestKeyProvider::new("test-key-1", "local-key");

        let start = SocialApi::oauth_start(&mut store, &user_actor(), start_request())
            .unwrap_or_else(|err| panic!("oauth start: {err}"));
        assert_eq!(start.oauth_connection_id, "oauth_1");
        assert!(
            start
                .authorization_url
                .starts_with("https://accounts.google.com/o/oauth2/v2/auth?")
        );
        assert!(store.oauth_connection("oauth_1").is_ok());

        let complete =
            SocialApi::oauth_complete(&mut store, &key_provider, &user_actor(), complete_request())
                .unwrap_or_else(|err| panic!("oauth complete: {err}"));

        assert_eq!(complete.account.id, "acct_1");
        assert_eq!(complete.account.last_verified_at, Some(1_000));

        let json = serde_json::to_string(&complete)
            .unwrap_or_else(|err| panic!("serialize complete response: {err}"));
        assert!(!json.contains("access-secret"));
        assert!(!json.contains("refresh-secret"));
        assert!(!json.contains("access_token"));
        assert!(!json.contains("refresh_token"));

        // Token secret was persisted server-side, encrypted.
        let secret = store
            .token_secret_for_account("acct_1")
            .unwrap_or_else(|err| panic!("load token secret: {err}"));
        assert_ne!(secret.encrypted_access_token, "access-secret");
        let connection = store
            .oauth_connection("oauth_1")
            .unwrap_or_else(|err| panic!("load oauth connection: {err}"));
        assert_eq!(connection.status, OAuthConnectionStatus::Completed);
    }

    #[test]
    fn account_api_oauth_start_rejects_foreign_user() {
        let mut store = InMemorySocialStore::default();
        assert_eq!(
            SocialApi::oauth_start(&mut store, &other_user_actor(), start_request()),
            Err(SocialApiError::Unauthorized)
        );
        assert!(store.oauth_connection("oauth_1").is_err());
    }

    #[test]
    fn account_api_disconnect_checks_owner() {
        let mut store = InMemorySocialStore::default();
        store
            .save_connected_account(connected_account("acct_1", user_owner()))
            .unwrap_or_else(|err| panic!("save account: {err}"));

        // Foreign actor is rejected before touching the store.
        assert_eq!(
            SocialApi::disconnect_account(
                &mut store,
                &other_user_actor(),
                &ApiOwner { owner: user_owner() },
                "acct_1",
                2_000,
            ),
            Err(SocialApiError::Unauthorized)
        );
        assert_eq!(
            store
                .connected_account("acct_1")
                .unwrap_or_else(|err| panic!("account: {err}"))
                .status,
            ConnectedAccountStatus::Connected
        );

        let disabled = SocialApi::disconnect_account(
            &mut store,
            &user_actor(),
            &ApiOwner { owner: user_owner() },
            "acct_1",
            2_000,
        )
        .unwrap_or_else(|err| panic!("disconnect: {err}"));
        assert_eq!(disabled.status, ConnectedAccountStatus::Disabled);
    }

    #[test]
    fn account_api_workspace_admin_can_manage_but_viewer_cannot() {
        let mut store = InMemorySocialStore::default();
        let workspace_owner = OwnerRef::Workspace("workspace_1".into());
        store
            .save_connected_account(connected_account("acct_1", workspace_owner.clone()))
            .unwrap_or_else(|err| panic!("save account: {err}"));

        let admin = ApiActor::new(
            "admin_user",
            vec![WorkspaceMemberRole::new(
                "workspace_1",
                "admin_user",
                TeamRole::Admin,
            )],
        );
        let viewer = ApiActor::new(
            "viewer_user",
            vec![WorkspaceMemberRole::new(
                "workspace_1",
                "viewer_user",
                TeamRole::Viewer,
            )],
        );

        assert_eq!(
            SocialApi::disconnect_account(
                &mut store,
                &viewer,
                &ApiOwner {
                    owner: workspace_owner.clone()
                },
                "acct_1",
                2_000,
            ),
            Err(SocialApiError::Unauthorized)
        );

        let disabled = SocialApi::disconnect_account(
            &mut store,
            &admin,
            &ApiOwner {
                owner: workspace_owner,
            },
            "acct_1",
            2_000,
        )
        .unwrap_or_else(|err| panic!("admin disconnect: {err}"));
        assert_eq!(disabled.status, ConnectedAccountStatus::Disabled);
    }
}
