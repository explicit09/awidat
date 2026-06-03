//! Server-backed social publishing Tauri commands.
//!
//! Each command is a thin translation: build a single-user actor/owner, lock
//! the file-backed `SqliteSocialStore` in [`AwidatState`], call `SocialApi`,
//! and return a serde response carrying no token material. No business logic
//! lives here — it all stays in the `awidat-social` domain services. Because
//! `SocialApi` is framework-neutral, these bodies lift onto an axum wrapper
//! later unchanged.

use awidat_social::api::{
    AccountSummary, ApiActor, ApiOwner, OAuthCompleteRequest, OAuthCompleteResponse,
    OAuthStartRequest, OAuthStartResponse, ProviderSummary, SocialApi, SocialApiError,
};
use awidat_social::model::{
    AccountEligibility, AccountKind, ConnectedAccount, ConnectedAccountStatus, OwnerRef, Provider,
    ProviderCapabilities,
};
use awidat_social::oauth_url::OAuthProviderConfig;
use awidat_social::provider::ProviderRegistry;
use awidat_social::sqlite_store::SqliteSocialStore;
use awidat_social::token::TestKeyProvider;
use awidat_social::token_bundle::ProviderTokenBundle;
use tauri::State;

use crate::state::AwidatState;

/// Stable single-user identity for this pass. Swapped for a real authenticated
/// user id when an identity service exists; see the design doc.
const LOCAL_USER_ID: &str = "local-user";

fn actor() -> ApiActor {
    ApiActor::new(LOCAL_USER_ID, Vec::new())
}

fn owner() -> ApiOwner {
    ApiOwner::user(LOCAL_USER_ID)
}

/// Local dev key provider for the token envelope. Real KMS lands with the
/// live-provider sub-project; this keeps the desktop self-contained meanwhile.
fn key_provider() -> TestKeyProvider {
    TestKeyProvider::new("desktop-local-key", "awidat-desktop-local-key")
}

/// Maps a `SocialApiError` to a stable string the frontend can branch on.
fn err_string(err: SocialApiError) -> String {
    match err {
        SocialApiError::Unauthorized => "unauthorized".to_string(),
        other => other.to_string(),
    }
}

/// Runs `f` with an exclusive lock on the initialized social store.
///
/// `AwidatState.social` is a `tokio::sync::Mutex`, so this is async and locks
/// via `.lock().await` — matching every other command in this crate. Every
/// call site therefore ends with `.await`.
async fn with_store<T>(
    state: &State<'_, AwidatState>,
    f: impl FnOnce(&mut SqliteSocialStore) -> Result<T, SocialApiError>,
) -> Result<T, String> {
    let mut guard = state.social.lock().await;
    let store = guard
        .as_mut()
        .ok_or_else(|| "social store not initialized".to_string())?;
    f(store).map_err(err_string)
}

#[tauri::command]
pub async fn social_providers() -> Result<Vec<ProviderSummary>, String> {
    let registry = ProviderRegistry::default_multi_platform();
    Ok(SocialApi::providers(&registry))
}

#[tauri::command]
pub async fn social_accounts(state: State<'_, AwidatState>) -> Result<Vec<AccountSummary>, String> {
    let actor = actor();
    let owner = owner();
    with_store(&state, |store| SocialApi::accounts(store, &actor, &owner)).await
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuthStartArgs {
    pub oauth_connection_id: String,
    pub provider: Provider,
    pub client_id: String,
    pub redirect_uri: String,
    pub raw_state: String,
    pub return_to: String,
    pub created_at: i64,
    pub expires_at: i64,
}

#[tauri::command]
pub async fn social_oauth_start(
    state: State<'_, AwidatState>,
    args: OAuthStartArgs,
) -> Result<OAuthStartResponse, String> {
    let actor = actor();
    with_store(&state, |store| {
        SocialApi::oauth_start(
            store,
            &actor,
            OAuthStartRequest {
                oauth_connection_id: args.oauth_connection_id,
                owner: OwnerRef::User(LOCAL_USER_ID.into()),
                provider: args.provider,
                config: OAuthProviderConfig {
                    client_id: args.client_id,
                    redirect_uri: args.redirect_uri,
                },
                raw_state: args.raw_state,
                return_to: args.return_to,
                created_at: args.created_at,
                expires_at: args.expires_at,
            },
        )
    })
    .await
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuthCompleteArgs {
    pub oauth_connection_id: String,
    pub provider: Provider,
    pub raw_state: String,
    pub account_id: String,
    pub provider_account_id: String,
    pub display_name: String,
    pub handle: Option<String>,
    pub now: i64,
}

#[tauri::command]
pub async fn social_oauth_complete(
    state: State<'_, AwidatState>,
    args: OAuthCompleteArgs,
) -> Result<OAuthCompleteResponse, String> {
    let actor = actor();
    let key = key_provider();
    with_store(&state, |store| {
        let connected_account = ConnectedAccount {
            id: args.account_id.clone(),
            owner: OwnerRef::User(LOCAL_USER_ID.into()),
            provider: args.provider.clone(),
            provider_account_id: args.provider_account_id.clone(),
            display_name: args.display_name.clone(),
            handle: args.handle.clone(),
            avatar_url: None,
            account_kind: AccountKind::Unknown,
            status: ConnectedAccountStatus::Connected,
            scopes: Vec::new(),
            capabilities: ProviderCapabilities::default(),
            eligibility: AccountEligibility::eligible(),
            last_verified_at: None,
            created_at: args.now,
            updated_at: args.now,
        };
        // Deterministic stub bundle — no live token exchange this pass. The
        // live provider sub-project swaps only this construction for a real
        // exchange; the command and UI are unchanged.
        let token_bundle = ProviderTokenBundle {
            provider: args.provider.clone(),
            provider_account_id: args.provider_account_id.clone(),
            scopes: Vec::new(),
            access_token_expires_at: args.now + 3_600,
            refresh_token_expires_at: Some(args.now + 86_400),
        };
        SocialApi::oauth_complete(
            store,
            &key,
            &actor,
            OAuthCompleteRequest {
                oauth_connection_id: args.oauth_connection_id.clone(),
                owner: OwnerRef::User(LOCAL_USER_ID.into()),
                raw_state: args.raw_state.clone(),
                connected_account,
                token_bundle,
                access_token: format!("stub-access-{}", args.account_id),
                refresh_token: Some(format!("stub-refresh-{}", args.account_id)),
                now: args.now,
            },
        )
    })
    .await
}

#[tauri::command]
pub async fn social_disconnect_account(
    state: State<'_, AwidatState>,
    account_id: String,
    now: i64,
) -> Result<AccountSummary, String> {
    let actor = actor();
    let owner = owner();
    with_store(&state, |store| {
        SocialApi::disconnect_account(store, &actor, &owner, &account_id, now)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use awidat_social::store::SocialStore;

    fn store_with_account() -> SqliteSocialStore {
        let mut store =
            SqliteSocialStore::new_in_memory().unwrap_or_else(|err| panic!("open store: {err}"));
        store
            .save_connected_account(ConnectedAccount {
                id: "acct_1".into(),
                owner: OwnerRef::User(LOCAL_USER_ID.into()),
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
            })
            .unwrap_or_else(|err| panic!("save account: {err}"));
        store
    }

    #[test]
    fn accounts_for_local_user_are_token_safe() {
        let store = store_with_account();
        let accounts = SocialApi::accounts(&store, &actor(), &owner())
            .unwrap_or_else(|err| panic!("accounts: {err}"));
        assert_eq!(accounts.len(), 1);
        let json =
            serde_json::to_string(&accounts).unwrap_or_else(|err| panic!("serialize: {err}"));
        assert!(!json.contains("access_token"));
        assert!(!json.contains("refresh_token"));
    }

    #[test]
    fn providers_list_has_three_slots() {
        let registry = ProviderRegistry::default_multi_platform();
        assert_eq!(SocialApi::providers(&registry).len(), 3);
    }

    #[test]
    fn oauth_complete_then_disconnect_round_trips_without_tokens() {
        let mut store =
            SqliteSocialStore::new_in_memory().unwrap_or_else(|err| panic!("open store: {err}"));
        let key = key_provider();

        // Start a connection so the callback state-hash validates.
        SocialApi::oauth_start(
            &mut store,
            &actor(),
            OAuthStartRequest {
                oauth_connection_id: "oauth_1".into(),
                owner: OwnerRef::User(LOCAL_USER_ID.into()),
                provider: Provider::YouTube,
                config: OAuthProviderConfig {
                    client_id: "client_1".into(),
                    redirect_uri: "https://app.awidat.test/cb".into(),
                },
                raw_state: "state-1".into(),
                return_to: "/".into(),
                created_at: 100,
                expires_at: 10_000,
            },
        )
        .unwrap_or_else(|err| panic!("oauth start: {err}"));

        let complete = SocialApi::oauth_complete(
            &mut store,
            &key,
            &actor(),
            OAuthCompleteRequest {
                oauth_connection_id: "oauth_1".into(),
                owner: OwnerRef::User(LOCAL_USER_ID.into()),
                raw_state: "state-1".into(),
                connected_account: ConnectedAccount {
                    id: "acct_1".into(),
                    owner: OwnerRef::User(LOCAL_USER_ID.into()),
                    provider: Provider::YouTube,
                    provider_account_id: "channel_1".into(),
                    display_name: "Awidat".into(),
                    handle: None,
                    avatar_url: None,
                    account_kind: AccountKind::Unknown,
                    status: ConnectedAccountStatus::Connected,
                    scopes: Vec::new(),
                    capabilities: ProviderCapabilities::default(),
                    eligibility: AccountEligibility::eligible(),
                    last_verified_at: None,
                    created_at: 1_000,
                    updated_at: 1_000,
                },
                token_bundle: ProviderTokenBundle {
                    provider: Provider::YouTube,
                    provider_account_id: "channel_1".into(),
                    scopes: Vec::new(),
                    access_token_expires_at: 4_600,
                    refresh_token_expires_at: Some(87_400),
                },
                access_token: "stub-access-acct_1".into(),
                refresh_token: Some("stub-refresh-acct_1".into()),
                now: 1_000,
            },
        )
        .unwrap_or_else(|err| panic!("oauth complete: {err}"));
        assert_eq!(complete.account.id, "acct_1");

        let json =
            serde_json::to_string(&complete).unwrap_or_else(|err| panic!("serialize: {err}"));
        assert!(!json.contains("stub-access"));
        assert!(!json.contains("stub-refresh"));

        let disabled =
            SocialApi::disconnect_account(&mut store, &actor(), &owner(), "acct_1", 2_000)
                .unwrap_or_else(|err| panic!("disconnect: {err}"));
        assert_eq!(disabled.status, ConnectedAccountStatus::Disabled);
    }
}
