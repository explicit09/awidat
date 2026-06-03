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
    AccountEligibility, AccountKind, CampaignVariantTarget, ConnectedAccount,
    ConnectedAccountStatus, OwnerRef, Provider, ProviderCapabilities, PublishJob, PublishJobEvent,
    PublishJobEventType, PublishJobStatus, TeamAction, WorkspaceMemberRole,
};
use crate::oauth_url::OAuthProviderConfig;
use crate::provider::{ProviderRegistry, ProviderState};
use crate::publish_service::{
    BindTargetInput, PublishService, PublishServiceError, ScheduleTargetInput,
};
use crate::store::{SocialStore, SocialStoreError};
use crate::team_service::TeamPolicy;
use crate::token::LocalTokenKeyProvider;
use crate::token_bundle::ProviderTokenBundle;
use crate::upload_adapter::UploadAdapter;
use crate::upload_service::{ExecuteUploadInput, UploadService, UploadServiceError};
use crate::upload_status::{
    PollUploadStatusInput, UploadStatusAdapter, UploadStatusService, UploadStatusServiceError,
};
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

impl From<PublishServiceError> for SocialApiError {
    fn from(error: PublishServiceError) -> Self {
        match error {
            PublishServiceError::Store(store) => SocialApiError::Store(store),
            // An owner mismatch detected by the domain service is an
            // authorization failure from the caller's perspective.
            PublishServiceError::OwnerMismatch => SocialApiError::Unauthorized,
            other => SocialApiError::Publish(other.to_string()),
        }
    }
}

impl From<UploadServiceError> for SocialApiError {
    fn from(error: UploadServiceError) -> Self {
        match error {
            UploadServiceError::Store(store) => SocialApiError::Store(store),
            other => SocialApiError::Upload(other.to_string()),
        }
    }
}

impl From<UploadStatusServiceError> for SocialApiError {
    fn from(error: UploadStatusServiceError) -> Self {
        match error {
            UploadStatusServiceError::Store(store) => SocialApiError::Store(store),
            other => SocialApiError::Status(other.to_string()),
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

/// `POST /campaigns/:campaign_id/variants/:variant_id/target` body.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BindTargetRequest {
    pub target_id: String,
    pub campaign_id: String,
    pub variant_id: String,
    pub connected_account_id: String,
    pub platform_fields: serde_json::Value,
    pub scheduled_for: i64,
    pub now: i64,
}

/// `POST /campaigns/:campaign_id/variants/:variant_id/validate` body.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidateTargetRequest {
    pub target_id: String,
    pub now: i64,
}

/// `POST /campaigns/:campaign_id/variants/:variant_id/schedule` body.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleTargetRequest {
    pub target_id: String,
    pub job_id: String,
    pub artifact_ref: String,
    pub created_by: String,
    pub now: i64,
}

/// Publish job state, safe to serialize to clients.
///
/// Carries only public job fields: never token rows. The `raw_error_ref` is a
/// pointer to a server-side error blob, not the raw provider error body.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishJobResponse {
    pub id: String,
    pub campaign_id: String,
    pub variant_id: String,
    pub connected_account_id: String,
    pub provider: Provider,
    pub status: PublishJobStatus,
    pub attempt_count: u32,
    pub scheduled_for: i64,
    pub provider_post_id: Option<String>,
    pub provider_post_url: Option<String>,
    pub normalized_error: Option<String>,
    pub raw_error_ref: Option<String>,
    pub requires_action_reason: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub events: Vec<PublishJobEventResponse>,
}

impl PublishJobResponse {
    fn from_job(job: PublishJob, events: Vec<PublishJobEvent>) -> Self {
        Self {
            id: job.id,
            campaign_id: job.campaign_id,
            variant_id: job.variant_id,
            connected_account_id: job.connected_account_id,
            provider: job.provider,
            status: job.status,
            attempt_count: job.attempt_count,
            scheduled_for: job.scheduled_for,
            provider_post_id: job.provider_post_id,
            provider_post_url: job.provider_post_url,
            normalized_error: job.normalized_error,
            raw_error_ref: job.raw_error_ref,
            requires_action_reason: job.requires_action_reason,
            created_at: job.created_at,
            updated_at: job.updated_at,
            events: events
                .into_iter()
                .map(PublishJobEventResponse::from)
                .collect(),
        }
    }
}

/// Public audit-event summary, safe to serialize to clients.
///
/// Event `metadata` is forwarded as produced by the domain services, which
/// already keep provider token material out of event payloads (see the upload
/// and status service token-safety tests).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishJobEventResponse {
    pub id: String,
    pub event_type: PublishJobEventType,
    pub message: String,
    pub metadata: serde_json::Value,
    pub created_at: i64,
}

impl From<PublishJobEvent> for PublishJobEventResponse {
    fn from(event: PublishJobEvent) -> Self {
        Self {
            id: event.id,
            event_type: event.event_type,
            message: event.message,
            metadata: event.metadata,
            created_at: event.created_at,
        }
    }
}

/// Worker request to execute a claimed upload job.
///
/// This is the server worker boundary, not a public user route. The worker
/// supplies the resolved publish metadata (title/description/tags/thumbnail)
/// it composed from campaign and account defaults.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecuteUploadRequest {
    pub job_id: String,
    pub title: String,
    pub description: Option<String>,
    pub tags: Vec<String>,
    pub thumbnail_ref: Option<String>,
    pub now: i64,
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

    /// `POST /campaigns/:campaign_id/variants/:variant_id/target`: bind a
    /// connected account to a campaign variant.
    pub fn bind_target(
        store: &mut impl SocialStore,
        actor: &ApiActor,
        request: BindTargetRequest,
    ) -> Result<CampaignVariantTarget, SocialApiError> {
        let owner = account_owner(store, &request.connected_account_id)?;
        actor.authorize(&owner, TeamAction::SchedulePublish)?;
        let target = PublishService::bind_target(
            store,
            &owner,
            BindTargetInput {
                id: request.target_id,
                campaign_id: request.campaign_id,
                variant_id: request.variant_id,
                connected_account_id: request.connected_account_id,
                platform_fields: request.platform_fields,
                scheduled_for: request.scheduled_for,
                now: request.now,
            },
        )?;
        Ok(target)
    }

    /// `POST /campaigns/:campaign_id/variants/:variant_id/validate`: run the
    /// provider capability/eligibility validation rules for a bound target.
    pub fn validate_target(
        store: &mut impl SocialStore,
        registry: &ProviderRegistry,
        actor: &ApiActor,
        request: ValidateTargetRequest,
    ) -> Result<CampaignVariantTarget, SocialApiError> {
        let target = store.campaign_variant_target(&request.target_id)?;
        // Reject unknown providers before mutating any state.
        registry
            .get(&target.provider)
            .map_err(|err| SocialApiError::Publish(err.to_string()))?;
        let owner = account_owner(store, &target.connected_account_id)?;
        actor.authorize(&owner, TeamAction::SchedulePublish)?;
        PublishService::validate_target(store, &owner, &request.target_id, request.now)?;
        let validated = store.campaign_variant_target(&request.target_id)?;
        Ok(validated)
    }

    /// `POST /campaigns/:campaign_id/variants/:variant_id/schedule`: create a
    /// scheduled publish job for a validated target.
    pub fn schedule_target(
        store: &mut impl SocialStore,
        registry: &ProviderRegistry,
        actor: &ApiActor,
        request: ScheduleTargetRequest,
    ) -> Result<PublishJobResponse, SocialApiError> {
        let target = store.campaign_variant_target(&request.target_id)?;
        registry
            .get(&target.provider)
            .map_err(|err| SocialApiError::Publish(err.to_string()))?;
        let owner = account_owner(store, &target.connected_account_id)?;
        actor.authorize(&owner, TeamAction::SchedulePublish)?;
        let job = PublishService::schedule_target(
            store,
            &owner,
            ScheduleTargetInput {
                job_id: request.job_id,
                target_id: request.target_id,
                artifact_ref: request.artifact_ref,
                created_by: request.created_by,
                now: request.now,
            },
        )?;
        job_response(store, job)
    }

    /// `GET /publish-jobs/:id`: read a publish job and its audit trail.
    pub fn publish_job(
        store: &impl SocialStore,
        actor: &ApiActor,
        owner: &ApiOwner,
        job_id: &str,
    ) -> Result<PublishJobResponse, SocialApiError> {
        let job = store.publish_job(job_id)?;
        let account_owner = account_owner(store, &job.connected_account_id)?;
        // The requested owner must match the job's account owner, and the actor
        // must be permitted to read for that owner.
        if account_owner != owner.owner {
            return Err(SocialApiError::Unauthorized);
        }
        authorize_read(actor, &owner.owner)?;
        let events = store.publish_job_events(job_id)?;
        Ok(PublishJobResponse::from_job(job, events))
    }

    /// `POST /publish-jobs/:id/cancel`: cancel a publish job.
    pub fn cancel_job(
        store: &mut impl SocialStore,
        actor: &ApiActor,
        owner: &ApiOwner,
        job_id: &str,
        now: i64,
    ) -> Result<PublishJobResponse, SocialApiError> {
        authorize_job_owner(store, actor, owner, job_id, TeamAction::CancelPublish)?;
        let job = PublishService::cancel_job(store, &owner.owner, job_id, now)?;
        job_response(store, job)
    }

    /// `POST /publish-jobs/:id/retry`: retry a failed or action-required job.
    pub fn retry_job(
        store: &mut impl SocialStore,
        actor: &ApiActor,
        owner: &ApiOwner,
        job_id: &str,
        now: i64,
    ) -> Result<PublishJobResponse, SocialApiError> {
        authorize_job_owner(store, actor, owner, job_id, TeamAction::RetryPublish)?;
        let job = PublishService::retry_job(store, &owner.owner, job_id, now)?;
        job_response(store, job)
    }

    /// Worker entrypoint: execute an already-claimed upload job.
    ///
    /// Not a public user route, so it takes no [`ApiActor`]. It delegates to
    /// [`UploadService::execute_claimed_job`], preserving the Phase 4B upload
    /// lifecycle protections, and returns only a sanitized job response.
    pub fn execute_claimed_upload_job(
        store: &mut impl SocialStore,
        adapter: &impl UploadAdapter,
        request: ExecuteUploadRequest,
    ) -> Result<PublishJobResponse, SocialApiError> {
        let job = UploadService::execute_claimed_job(
            store,
            adapter,
            ExecuteUploadInput {
                job_id: request.job_id,
                title: request.title,
                description: request.description,
                tags: request.tags,
                thumbnail_ref: request.thumbnail_ref,
                now: request.now,
            },
        )?;
        job_response(store, job)
    }

    /// Worker entrypoint: poll a processing job's provider status.
    ///
    /// Not a public user route, so it takes no [`ApiActor`]. It delegates to
    /// [`UploadStatusService::poll_processing_job`], preserving the provider
    /// post-id mismatch guard, and returns only a sanitized job response.
    pub fn poll_upload_status(
        store: &mut impl SocialStore,
        adapter: &impl UploadStatusAdapter,
        job_id: &str,
        now: i64,
    ) -> Result<PublishJobResponse, SocialApiError> {
        let job = UploadStatusService::poll_processing_job(
            store,
            adapter,
            PollUploadStatusInput {
                job_id: job_id.into(),
                now,
            },
        )?;
        job_response(store, job)
    }
}

/// Returns the owner of the connected account, mapping a missing account to a
/// store not-found error.
fn account_owner(
    store: &impl SocialStore,
    connected_account_id: &str,
) -> Result<OwnerRef, SocialApiError> {
    Ok(store.connected_account(connected_account_id)?.owner)
}

/// Authorizes a read for the given owner. User-owned reads require the matching
/// user; workspace-owned reads require any membership role (Owner, Admin,
/// Publisher, or Viewer) in that workspace.
fn authorize_read(actor: &ApiActor, owner: &OwnerRef) -> Result<(), SocialApiError> {
    let allowed = match owner {
        OwnerRef::User(user_id) => *user_id == actor.user_id,
        OwnerRef::Workspace(workspace_id) => actor.workspace_roles.iter().any(|role| {
            role.workspace_id == *workspace_id && role.user_id == actor.user_id
        }),
    };
    if allowed {
        Ok(())
    } else {
        Err(SocialApiError::Unauthorized)
    }
}

/// Confirms the job belongs to the requested owner, then authorizes the actor
/// to perform `action` for that owner.
fn authorize_job_owner(
    store: &impl SocialStore,
    actor: &ApiActor,
    owner: &ApiOwner,
    job_id: &str,
    action: TeamAction,
) -> Result<(), SocialApiError> {
    let job = store.publish_job(job_id)?;
    let job_owner = account_owner(store, &job.connected_account_id)?;
    if job_owner != owner.owner {
        return Err(SocialApiError::Unauthorized);
    }
    actor.authorize(&owner.owner, action)
}

/// Builds a [`PublishJobResponse`] by loading the job's audit events.
fn job_response(
    store: &impl SocialStore,
    job: PublishJob,
) -> Result<PublishJobResponse, SocialApiError> {
    let events = store.publish_job_events(&job.id)?;
    Ok(PublishJobResponse::from_job(job, events))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        AccountEligibility, AccountKind, ConnectedAccount, ConnectedAccountStatus, OwnerRef,
        Provider, ProviderCapabilities, PublishJobEventType, PublishJobStatus, TeamRole,
        ValidationState, WorkspaceMemberRole,
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

    // --- Publish route facade -------------------------------------------------

    fn bind_request(account_id: &str) -> BindTargetRequest {
        BindTargetRequest {
            target_id: "target_1".into(),
            campaign_id: "campaign_1".into(),
            variant_id: "variant_1".into(),
            connected_account_id: account_id.into(),
            platform_fields: serde_json::json!({"privacy": "private"}),
            scheduled_for: 2_000,
            now: 1_000,
        }
    }

    /// Bind, validate, and schedule a job for `owner`'s `acct_1`, returning the
    /// scheduled job response.
    fn schedule_job(
        store: &mut InMemorySocialStore,
        registry: &ProviderRegistry,
        actor: &ApiActor,
        owner: OwnerRef,
    ) -> PublishJobResponse {
        store
            .save_connected_account(connected_account("acct_1", owner))
            .unwrap_or_else(|err| panic!("save account: {err}"));
        SocialApi::bind_target(store, actor, bind_request("acct_1"))
            .unwrap_or_else(|err| panic!("bind target: {err}"));
        SocialApi::validate_target(
            store,
            registry,
            actor,
            ValidateTargetRequest {
                target_id: "target_1".into(),
                now: 1_100,
            },
        )
        .unwrap_or_else(|err| panic!("validate target: {err}"));
        SocialApi::schedule_target(
            store,
            registry,
            actor,
            ScheduleTargetRequest {
                target_id: "target_1".into(),
                job_id: "job_1".into(),
                artifact_ref: "render://artifact_1".into(),
                created_by: "user_1".into(),
                now: 1_200,
            },
        )
        .unwrap_or_else(|err| panic!("schedule target: {err}"))
    }

    #[test]
    fn publish_api_binds_validates_and_schedules_target() {
        let mut store = InMemorySocialStore::default();
        let registry = ProviderRegistry::default_multi_platform();
        store
            .save_connected_account(connected_account("acct_1", user_owner()))
            .unwrap_or_else(|err| panic!("save account: {err}"));

        let target = SocialApi::bind_target(&mut store, &user_actor(), bind_request("acct_1"))
            .unwrap_or_else(|err| panic!("bind target: {err}"));
        assert_eq!(target.validation_state, ValidationState::Pending);

        let validated = SocialApi::validate_target(
            &mut store,
            &registry,
            &user_actor(),
            ValidateTargetRequest {
                target_id: "target_1".into(),
                now: 1_100,
            },
        )
        .unwrap_or_else(|err| panic!("validate target: {err}"));
        assert_eq!(validated.validation_state, ValidationState::Valid);

        let job = SocialApi::schedule_target(
            &mut store,
            &registry,
            &user_actor(),
            ScheduleTargetRequest {
                target_id: "target_1".into(),
                job_id: "job_1".into(),
                artifact_ref: "render://artifact_1".into(),
                created_by: "user_1".into(),
                now: 1_200,
            },
        )
        .unwrap_or_else(|err| panic!("schedule target: {err}"));

        assert_eq!(job.id, "job_1");
        assert_eq!(job.status, PublishJobStatus::Scheduled);
        assert_eq!(job.events.len(), 1);
        assert_eq!(job.events[0].event_type, PublishJobEventType::Scheduled);

        let looked_up = SocialApi::publish_job(
            &store,
            &user_actor(),
            &ApiOwner { owner: user_owner() },
            "job_1",
        )
        .unwrap_or_else(|err| panic!("publish job lookup: {err}"));
        assert_eq!(looked_up, job);
    }

    #[test]
    fn publish_api_bind_rejects_foreign_user() {
        let mut store = InMemorySocialStore::default();
        store
            .save_connected_account(connected_account("acct_1", user_owner()))
            .unwrap_or_else(|err| panic!("save account: {err}"));

        assert_eq!(
            SocialApi::bind_target(&mut store, &other_user_actor(), bind_request("acct_1")),
            Err(SocialApiError::Unauthorized)
        );
        assert!(store.campaign_variant_target("target_1").is_err());
    }

    #[test]
    fn publish_api_publish_job_rejects_wrong_owner() {
        let mut store = InMemorySocialStore::default();
        let registry = ProviderRegistry::default_multi_platform();
        schedule_job(&mut store, &registry, &user_actor(), user_owner());

        // Same actor but asking under the wrong owner key is rejected.
        assert_eq!(
            SocialApi::publish_job(
                &store,
                &user_actor(),
                &ApiOwner {
                    owner: OwnerRef::Workspace("workspace_1".into())
                },
                "job_1",
            ),
            Err(SocialApiError::Unauthorized)
        );
    }

    #[test]
    fn publish_api_cancel_and_retry_are_authorized() {
        let mut store = InMemorySocialStore::default();
        let registry = ProviderRegistry::default_multi_platform();
        schedule_job(&mut store, &registry, &user_actor(), user_owner());

        // Foreign user cannot cancel.
        assert_eq!(
            SocialApi::cancel_job(
                &mut store,
                &other_user_actor(),
                &ApiOwner { owner: user_owner() },
                "job_1",
                2_200,
            ),
            Err(SocialApiError::Unauthorized)
        );

        let cancelled = SocialApi::cancel_job(
            &mut store,
            &user_actor(),
            &ApiOwner { owner: user_owner() },
            "job_1",
            2_200,
        )
        .unwrap_or_else(|err| panic!("cancel job: {err}"));
        assert_eq!(cancelled.status, PublishJobStatus::Cancelled);

        // Cancelled job is not retryable; surfaced as a publish error.
        assert_eq!(
            SocialApi::retry_job(
                &mut store,
                &user_actor(),
                &ApiOwner { owner: user_owner() },
                "job_1",
                2_300,
            ),
            Err(SocialApiError::Publish(
                PublishServiceError::JobNotRetryable.to_string()
            ))
        );
    }

    #[test]
    fn publish_api_returns_job_without_token_material() {
        let mut store = InMemorySocialStore::default();
        let registry = ProviderRegistry::default_multi_platform();
        let job = schedule_job(&mut store, &registry, &user_actor(), user_owner());

        let json =
            serde_json::to_string(&job).unwrap_or_else(|err| panic!("serialize job: {err}"));
        assert!(!json.contains("access_token"));
        assert!(!json.contains("refresh_token"));
        assert!(!json.contains("encrypted"));
    }

    #[test]
    fn publish_api_workspace_publisher_can_schedule_but_not_disconnect_accounts() {
        let mut store = InMemorySocialStore::default();
        let registry = ProviderRegistry::default_multi_platform();
        let workspace_owner = OwnerRef::Workspace("workspace_1".into());
        let publisher = ApiActor::new(
            "publisher_user",
            vec![WorkspaceMemberRole::new(
                "workspace_1",
                "publisher_user",
                TeamRole::Publisher,
            )],
        );

        let job = schedule_job(&mut store, &registry, &publisher, workspace_owner.clone());
        assert_eq!(job.status, PublishJobStatus::Scheduled);

        let cancelled = SocialApi::cancel_job(
            &mut store,
            &publisher,
            &ApiOwner {
                owner: workspace_owner.clone(),
            },
            "job_1",
            2_200,
        )
        .unwrap_or_else(|err| panic!("publisher cancel: {err}"));
        assert_eq!(cancelled.status, PublishJobStatus::Cancelled);

        // A publisher cannot manage accounts.
        assert_eq!(
            SocialApi::disconnect_account(
                &mut store,
                &publisher,
                &ApiOwner {
                    owner: workspace_owner,
                },
                "acct_1",
                2_300,
            ),
            Err(SocialApiError::Unauthorized)
        );
    }

    // --- Worker route facade --------------------------------------------------

    use crate::token::TokenSecret;
    use crate::upload_adapter::MockUploadAdapter;
    use crate::upload_status::{
        UploadProcessingStatus, UploadStatusAdapterError, UploadStatusRequest, UploadStatusResult,
    };
    use std::cell::RefCell;

    struct StubStatusAdapter {
        result: Result<UploadStatusResult, UploadStatusAdapterError>,
        calls: RefCell<usize>,
    }

    impl StubStatusAdapter {
        fn new(result: UploadStatusResult) -> Self {
            Self {
                result: Ok(result),
                calls: RefCell::new(0),
            }
        }
    }

    impl UploadStatusAdapter for StubStatusAdapter {
        fn provider(&self) -> Provider {
            Provider::YouTube
        }

        fn poll_status(
            &self,
            _request: &UploadStatusRequest,
        ) -> Result<UploadStatusResult, UploadStatusAdapterError> {
            *self.calls.borrow_mut() += 1;
            self.result.clone()
        }
    }

    fn token_secret(account_id: &str) -> TokenSecret {
        TokenSecret::encrypt(
            account_id,
            "access-secret",
            Some("refresh-secret"),
            &TestKeyProvider::new("test-key-1", "local-key"),
            100,
        )
        .unwrap_or_else(|err| panic!("encrypt token secret: {err}"))
    }

    /// Store with an account, token secret, and a job in the `Uploading` state.
    fn store_with_claimed_job() -> InMemorySocialStore {
        let mut store = InMemorySocialStore::default();
        store
            .save_connected_account(connected_account("acct_1", user_owner()))
            .unwrap_or_else(|err| panic!("save account: {err}"));
        store
            .save_token_secret(token_secret("acct_1"))
            .unwrap_or_else(|err| panic!("save token secret: {err}"));
        let job = PublishJob::new(
            "job_1",
            "campaign_1",
            "variant_1",
            "acct_1",
            Provider::YouTube,
            "render://artifact_1",
            1_800,
            "user_1",
        )
        .claim_for_upload(1_900);
        store
            .save_publish_job(job)
            .unwrap_or_else(|err| panic!("save claimed job: {err}"));
        store
    }

    /// Store with an account, token secret, and a job in the `Processing` state.
    fn store_with_processing_job() -> InMemorySocialStore {
        let mut store = InMemorySocialStore::default();
        store
            .save_connected_account(connected_account("acct_1", user_owner()))
            .unwrap_or_else(|err| panic!("save account: {err}"));
        store
            .save_token_secret(token_secret("acct_1"))
            .unwrap_or_else(|err| panic!("save token secret: {err}"));
        let job = PublishJob::new(
            "job_1",
            "campaign_1",
            "variant_1",
            "acct_1",
            Provider::YouTube,
            "render://artifact_1",
            2_000,
            "user_1",
        )
        .schedule(2_000)
        .claim_for_upload(2_100)
        .processing("yt_video_1", 2_200);
        store
            .save_publish_job(job)
            .unwrap_or_else(|err| panic!("save processing job: {err}"));
        store
    }

    fn execute_request() -> ExecuteUploadRequest {
        ExecuteUploadRequest {
            job_id: "job_1".into(),
            title: "Launch clip".into(),
            description: Some("Description".into()),
            tags: vec!["awidat".into()],
            thumbnail_ref: Some("render://thumb_1".into()),
            now: 2_000,
        }
    }

    #[test]
    fn worker_api_executes_claimed_upload_job() {
        let mut store = store_with_claimed_job();
        let adapter = MockUploadAdapter::published(
            Provider::YouTube,
            "yt_video_1",
            "https://www.youtube.com/watch?v=yt_video_1",
        );

        let job = SocialApi::execute_claimed_upload_job(&mut store, &adapter, execute_request())
            .unwrap_or_else(|err| panic!("execute upload: {err}"));

        assert_eq!(job.status, PublishJobStatus::Published);
        assert_eq!(job.provider_post_id.as_deref(), Some("yt_video_1"));
        assert_eq!(
            job.provider_post_url.as_deref(),
            Some("https://www.youtube.com/watch?v=yt_video_1")
        );
        assert_eq!(job.events.last().map(|e| &e.event_type), Some(&PublishJobEventType::Uploaded));
    }

    #[test]
    fn worker_api_polls_processing_job_to_published() {
        let mut store = store_with_processing_job();
        let adapter = StubStatusAdapter::new(UploadStatusResult {
            provider_post_id: "yt_video_1".into(),
            provider_post_url: Some("https://www.youtube.com/watch?v=yt_video_1".into()),
            status: UploadProcessingStatus::Published,
            normalized_error: None,
            raw_error_ref: None,
        });

        let job = SocialApi::poll_upload_status(&mut store, &adapter, "job_1", 2_500)
            .unwrap_or_else(|err| panic!("poll status: {err}"));

        assert_eq!(job.status, PublishJobStatus::Published);
        assert_eq!(*adapter.calls.borrow(), 1);
        assert_eq!(
            job.events.last().map(|e| &e.event_type),
            Some(&PublishJobEventType::StatusPolled)
        );
    }

    #[test]
    fn worker_api_rejects_status_post_id_mismatch() {
        let mut store = store_with_processing_job();
        let adapter = StubStatusAdapter::new(UploadStatusResult {
            provider_post_id: "yt_video_other".into(),
            provider_post_url: None,
            status: UploadProcessingStatus::Processing,
            normalized_error: None,
            raw_error_ref: None,
        });

        assert_eq!(
            SocialApi::poll_upload_status(&mut store, &adapter, "job_1", 2_500),
            Err(SocialApiError::Status(
                UploadStatusServiceError::ProviderPostIdMismatch.to_string()
            ))
        );
        // Guard preserved: job stays processing, no event written.
        let job = store
            .publish_job("job_1")
            .unwrap_or_else(|err| panic!("job: {err}"));
        assert_eq!(job.status, PublishJobStatus::Processing);
        assert!(
            store
                .publish_job_events("job_1")
                .unwrap_or_else(|err| panic!("events: {err}"))
                .is_empty()
        );
    }

    #[test]
    fn worker_api_events_do_not_include_raw_token_material() {
        let mut store = store_with_claimed_job();
        let adapter = MockUploadAdapter::published(
            Provider::YouTube,
            "yt_video_1",
            "https://www.youtube.com/watch?v=yt_video_1",
        );

        let job = SocialApi::execute_claimed_upload_job(&mut store, &adapter, execute_request())
            .unwrap_or_else(|err| panic!("execute upload: {err}"));

        let json =
            serde_json::to_string(&job).unwrap_or_else(|err| panic!("serialize job: {err}"));
        assert!(!json.contains("access-secret"));
        assert!(!json.contains("refresh-secret"));
        assert!(!json.contains("access_token"));
        assert!(!json.contains("refresh_token"));
    }
}
