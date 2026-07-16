//! Phase 5 user-facing routes — the API surface the desktop client calls.
//!
//! These mirror the `SocialApi` facade methods one-for-one. In multi-user
//! deployments, they authenticate the desktop-forwarded Supabase bearer and
//! load workspace roles from `workspace_member_roles`. In local single-user
//! development, they can fall back to the static desktop dev bearer. No secret
//! (client_secret / refresh token / access token) is ever returned — the
//! `SocialApi` DTOs are already redaction-tested.
//!
//! The sync `SocialApi` runs on the blocking pool via `spawn_blocking`, exactly
//! like the internal routes.

use crate::{SharedState, bearer_auth, now_secs, parse_provider, provider_client_id, redirect_uri};
use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
};
use montage_social::{
    api::{
        AccountSummary, ApiActor, ApiOwner, BindTargetRequest, OAuthStartRequest,
        OAuthStartResponse, PublishJobResponse, RescheduleJobRequest, ScheduleTargetRequest,
        SocialApi, SocialApiError, UpdateTargetRequest, ValidateTargetRequest,
        ValidateTargetResponse,
    },
    auth_context::JwtVerifier,
    model::{AccountUsageAudit, TeamAction, WorkspaceMemberRole},
    oauth_url::OAuthProviderConfig,
    store::SocialStore,
};
use serde::{Deserialize, Serialize};

type HttpError = (StatusCode, Json<serde_json::Value>);
type HttpResult<T> = Result<Json<T>, HttpError>;

// ── Auth + error mapping ────────────────────────────────────────────────────

const WORKSPACE_ID_HEADER: &str = "x-montage-workspace-id";

/// Authenticate a request and return its per-user actor + targeted owner.
///
/// Two modes (Phase 7):
/// - **Supabase Auth (when `SUPABASE_JWT_SECRET` is set):** verify the bearer as
///   a Supabase JWT → real per-user `user_id`. This is the multi-user path.
/// - **Dev bearer (fallback):** compare the bearer to `DESKTOP_AUTH_TOKEN` →
///   the single fixed `DESKTOP_USER_ID`. Pre-Supabase single-user dev.
///
/// By default the desktop targets the caller's own resources
/// (`OwnerRef::User`). When `x-montage-workspace-id` is present, the request
/// targets that workspace; the actor carries roles loaded from the authoritative
/// social store so `TeamPolicy` can enforce Owner/Admin/Publisher/Viewer access.
async fn desktop_auth(
    state: &SharedState,
    headers: &HeaderMap,
) -> Result<(ApiActor, ApiOwner), HttpError> {
    let user_id = authenticated_user_id(state, headers)?;
    if !social_user_allowed(&user_id, &state.config.social_allowed_user_ids) {
        return Err(forbidden());
    }
    let owner = target_owner_from_headers(&user_id, headers);
    let roles = if !state.config.supabase_jwt_secret.is_empty() {
        workspace_roles_for_user(state, &user_id).await?
    } else {
        Vec::new()
    };
    Ok((ApiActor::new(user_id, roles), owner))
}

/// Resolve the authenticated user id from the request, or `Unauthorized`.
fn authenticated_user_id(state: &SharedState, headers: &HeaderMap) -> Result<String, HttpError> {
    let bearer = bearer_token(headers);

    // Supabase Auth path: verify the JWT into a real user id.
    if !state.config.supabase_jwt_secret.is_empty() {
        let bearer = bearer.ok_or_else(unauthorized)?;
        let verifier =
            crate::supabase_jwt::SupabaseJwtVerifier::new_hs256(&state.config.supabase_jwt_secret);
        let claims = verifier
            .verify(&bearer, crate::now_secs())
            .map_err(|_| unauthorized())?;
        return Ok(claims.user_id);
    }

    // Dev-bearer fallback (single user). Fails closed if unconfigured.
    if desktop_token_ok(&state.config.desktop_auth_token, headers) {
        return Ok(state.config.desktop_user_id.clone());
    }
    Err(unauthorized())
}

fn unauthorized() -> HttpError {
    (
        StatusCode::UNAUTHORIZED,
        Json(serde_json::json!({"error": "unauthorized"})),
    )
}

fn forbidden() -> HttpError {
    (
        StatusCode::FORBIDDEN,
        Json(serde_json::json!({"error": "forbidden"})),
    )
}

/// Extract the raw bearer value from the `Authorization` header, if present.
fn bearer_token(headers: &HeaderMap) -> Option<String> {
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::to_string)
}

/// Pure predicate for the dev-bearer check. Fails closed on an empty configured
/// token so a misconfigured deployment never accepts an empty bearer.
fn desktop_token_ok(configured: &str, headers: &HeaderMap) -> bool {
    !configured.is_empty() && bearer_auth(headers, configured)
}

fn social_user_allowed(user_id: &str, allowed_user_ids: &[String]) -> bool {
    allowed_user_ids.is_empty() || allowed_user_ids.iter().any(|allowed| allowed == user_id)
}

fn target_owner_from_headers(user_id: &str, headers: &HeaderMap) -> ApiOwner {
    workspace_id_from_headers(headers)
        .map(ApiOwner::workspace)
        .unwrap_or_else(|| ApiOwner::user(user_id))
}

fn workspace_id_from_headers(headers: &HeaderMap) -> Option<String> {
    headers
        .get(WORKSPACE_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

async fn workspace_roles_for_user(
    state: &SharedState,
    user_id: &str,
) -> Result<Vec<WorkspaceMemberRole>, HttpError> {
    let store_handle = state.store.clone();
    let user_id = user_id.to_string();
    tokio::task::spawn_blocking(move || {
        let store = store_handle.open();
        store.workspace_member_roles_for_user(&user_id)
    })
    .await
    .map_err(join_error)?
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("load workspace roles: {e}")})),
        )
    })
}

/// Map a domain `SocialApiError` to an HTTP status + JSON body. Never leaks
/// token material (the facade already redacts).
fn map_api_error(e: SocialApiError) -> HttpError {
    let status = match &e {
        SocialApiError::Unauthorized => StatusCode::FORBIDDEN,
        SocialApiError::Store(montage_social::store::SocialStoreError::NotFound) => {
            StatusCode::NOT_FOUND
        }
        SocialApiError::Account(_)
        | SocialApiError::Publish(_)
        | SocialApiError::Upload(_)
        | SocialApiError::Status(_)
        | SocialApiError::Team(_) => StatusCode::UNPROCESSABLE_ENTITY,
        SocialApiError::Store(_) => StatusCode::INTERNAL_SERVER_ERROR,
    };
    (status, Json(serde_json::json!({"error": e.to_string()})))
}

fn join_error(e: impl std::fmt::Display) -> HttpError {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(serde_json::json!({"error": format!("join error: {e}")})),
    )
}

// ── GET /social/accounts ────────────────────────────────────────────────────

pub(crate) async fn accounts_handler(
    State(state): State<SharedState>,
    headers: HeaderMap,
) -> HttpResult<Vec<AccountSummary>> {
    let (actor, owner) = desktop_auth(&state, &headers).await?;
    let store_handle = state.store.clone();
    let accounts = tokio::task::spawn_blocking(move || {
        let store = store_handle.open();
        SocialApi::accounts(&store, &actor, &owner)
    })
    .await
    .map_err(join_error)?
    .map_err(map_api_error)?;
    Ok(Json(accounts))
}

// ── POST /social/oauth/start/{provider} ─────────────────────────────────────

#[derive(Deserialize, Default)]
pub(crate) struct OAuthStartBody {
    #[serde(default)]
    return_to: String,
}

pub(crate) async fn oauth_start_handler(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Path(provider_str): Path<String>,
    body: Option<Json<OAuthStartBody>>,
) -> HttpResult<OAuthStartResponse> {
    let (actor, owner) = desktop_auth(&state, &headers).await?;
    let provider = parse_provider(&provider_str)?;
    let client_id = provider_client_id(&state.config, &provider)?;
    let redirect_uri = redirect_uri(&state.config, &provider);
    let return_to = body.map(|b| b.0.return_to).unwrap_or_default();

    // Server owns the connection id + CSRF state — the desktop never supplies
    // them. SECURITY: both carry CSPRNG entropy (not a guessable timestamp) so
    // the OAuth `state` is unforgeable and the connection handle is unguessable.
    //
    // The `state` round-trips through the provider verbatim and is the ONLY
    // correlation handle we get back on the callback (Google sends just
    // ?code&state). So we embed the connection id into `state` as
    // `<connection_id>~<random>` and recover it in the callback. The random
    // suffix keeps `state` unguessable; complete_oauth still validates the full
    // `state` against the stored hash.
    let now = now_secs();
    let handles = oauth_handles(&provider_str);
    let connection_id = handles.connection_id;
    let raw_state = handles.raw_state;
    let config = OAuthProviderConfig {
        client_id,
        redirect_uri,
    };
    let store_handle = state.store.clone();

    let resp = tokio::task::spawn_blocking(move || {
        let mut store = store_handle.open();
        SocialApi::oauth_start(
            &mut store,
            &actor,
            OAuthStartRequest {
                oauth_connection_id: connection_id,
                owner: owner.owner,
                provider,
                config,
                raw_state,
                return_to,
                created_at: now,
                // 15-minute OAuth window.
                expires_at: now + 900,
            },
        )
    })
    .await
    .map_err(join_error)?
    .map_err(map_api_error)?;
    Ok(Json(resp))
}

// ── POST /social/accounts/{id}/disconnect ───────────────────────────────────

pub(crate) async fn disconnect_handler(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Path(account_id): Path<String>,
) -> HttpResult<AccountSummary> {
    let (actor, owner) = desktop_auth(&state, &headers).await?;
    let store_handle = state.store.clone();
    let now = now_secs();
    let account = tokio::task::spawn_blocking(move || {
        let mut store = store_handle.open();
        SocialApi::disconnect_account(&mut store, &actor, &owner, &account_id, now)
    })
    .await
    .map_err(join_error)?
    .map_err(map_api_error)?;
    Ok(Json(account))
}

// ── GET /social/accounts/{id}/audit ─────────────────────────────────────────

pub(crate) async fn account_audit_handler(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Path(account_id): Path<String>,
) -> HttpResult<AccountUsageAudit> {
    let (actor, owner) = desktop_auth(&state, &headers).await?;
    let store_handle = state.store.clone();
    let audit = tokio::task::spawn_blocking(move || {
        let store = store_handle.open();
        SocialApi::account_usage_audit(&store, &actor, &owner, &account_id)
    })
    .await
    .map_err(join_error)?
    .map_err(map_api_error)?;
    Ok(Json(audit))
}

// ── POST /social/targets/bind ───────────────────────────────────────────────

pub(crate) async fn bind_target_handler(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(req): Json<BindTargetRequest>,
) -> HttpResult<montage_social::model::CampaignVariantTarget> {
    let (actor, _owner) = desktop_auth(&state, &headers).await?;
    let store_handle = state.store.clone();
    let target = tokio::task::spawn_blocking(move || {
        let mut store = store_handle.open();
        SocialApi::bind_target(&mut store, &actor, req)
    })
    .await
    .map_err(join_error)?
    .map_err(map_api_error)?;
    Ok(Json(target))
}

// ── POST /social/targets/update ─────────────────────────────────────────────

pub(crate) async fn update_target_handler(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(req): Json<UpdateTargetRequest>,
) -> HttpResult<montage_social::model::CampaignVariantTarget> {
    let (actor, _owner) = desktop_auth(&state, &headers).await?;
    let store_handle = state.store.clone();
    let target = tokio::task::spawn_blocking(move || {
        let mut store = store_handle.open();
        SocialApi::update_target(&mut store, &actor, req)
    })
    .await
    .map_err(join_error)?
    .map_err(map_api_error)?;
    Ok(Json(target))
}

// ── POST /social/targets/validate ───────────────────────────────────────────

pub(crate) async fn validate_target_handler(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(req): Json<ValidateTargetRequest>,
) -> HttpResult<ValidateTargetResponse> {
    let (actor, _owner) = desktop_auth(&state, &headers).await?;
    let store_handle = state.store.clone();
    let target = tokio::task::spawn_blocking(move || {
        let mut store = store_handle.open();
        SocialApi::validate_target(&mut store, &state_registry(), &actor, req)
    })
    .await
    .map_err(join_error)?
    .map_err(map_api_error)?;
    Ok(Json(target))
}

// ── POST /social/targets/schedule ───────────────────────────────────────────

pub(crate) async fn schedule_target_handler(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(req): Json<ScheduleTargetRequest>,
) -> HttpResult<PublishJobResponse> {
    let (actor, _owner) = desktop_auth(&state, &headers).await?;
    let store_handle = state.store.clone();
    let job = tokio::task::spawn_blocking(move || {
        let mut store = store_handle.open();
        SocialApi::schedule_target(&mut store, &state_registry(), &actor, req)
    })
    .await
    .map_err(join_error)?
    .map_err(map_api_error)?;
    Ok(Json(job))
}

// ── GET /social/jobs/{id} ───────────────────────────────────────────────────

pub(crate) async fn job_handler(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Path(job_id): Path<String>,
) -> HttpResult<PublishJobResponse> {
    let (actor, owner) = desktop_auth(&state, &headers).await?;
    let store_handle = state.store.clone();
    let job = tokio::task::spawn_blocking(move || {
        let store = store_handle.open();
        SocialApi::publish_job(&store, &actor, &owner, &job_id)
    })
    .await
    .map_err(join_error)?
    .map_err(map_api_error)?;
    Ok(Json(job))
}

// ── POST /social/jobs/{id}/cancel ───────────────────────────────────────────

pub(crate) async fn cancel_job_handler(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Path(job_id): Path<String>,
) -> HttpResult<PublishJobResponse> {
    let (actor, owner) = desktop_auth(&state, &headers).await?;
    let store_handle = state.store.clone();
    let now = now_secs();
    let job = tokio::task::spawn_blocking(move || {
        let mut store = store_handle.open();
        SocialApi::cancel_job(&mut store, &actor, &owner, &job_id, now)
    })
    .await
    .map_err(join_error)?
    .map_err(map_api_error)?;
    Ok(Json(job))
}

// ── POST /social/jobs/{id}/retry ────────────────────────────────────────────

pub(crate) async fn retry_job_handler(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Path(job_id): Path<String>,
) -> HttpResult<PublishJobResponse> {
    let (actor, owner) = desktop_auth(&state, &headers).await?;
    let store_handle = state.store.clone();
    let now = now_secs();
    let job = tokio::task::spawn_blocking(move || {
        let mut store = store_handle.open();
        SocialApi::retry_job(&mut store, &actor, &owner, &job_id, now)
    })
    .await
    .map_err(join_error)?
    .map_err(map_api_error)?;
    Ok(Json(job))
}

// ── POST /social/jobs/{id}/fire ─────────────────────────────────────────────

pub(crate) async fn fire_due_job_handler(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Path(job_id): Path<String>,
) -> HttpResult<PublishJobResponse> {
    let (actor, owner) = desktop_auth(&state, &headers).await?;
    let store_handle = state.store.clone();
    let job_id_for_check = job_id.clone();
    tokio::task::spawn_blocking(move || {
        let store = store_handle.open();
        SocialApi::authorize_publish_job_action(
            &store,
            &actor,
            &owner,
            &job_id_for_check,
            TeamAction::SchedulePublish,
        )
    })
    .await
    .map_err(join_error)?
    .map_err(map_api_error)?;

    crate::fire_due_publish_job(state.clone(), job_id.clone())
        .await
        .map_err(|error| {
            (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({ "error": error })),
            )
        })?;

    let (actor, owner) = desktop_auth(&state, &headers).await?;
    let store_handle = state.store.clone();
    let job = tokio::task::spawn_blocking(move || {
        let store = store_handle.open();
        SocialApi::publish_job(&store, &actor, &owner, &job_id)
    })
    .await
    .map_err(join_error)?
    .map_err(map_api_error)?;
    Ok(Json(job))
}

// ── POST /social/jobs/{id}/poll ─────────────────────────────────────────────

pub(crate) async fn poll_processing_job_handler(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Path(job_id): Path<String>,
) -> HttpResult<PublishJobResponse> {
    let (actor, owner) = desktop_auth(&state, &headers).await?;
    let store_handle = state.store.clone();
    let job_id_for_check = job_id.clone();
    tokio::task::spawn_blocking(move || {
        let store = store_handle.open();
        SocialApi::authorize_publish_job_action(
            &store,
            &actor,
            &owner,
            &job_id_for_check,
            TeamAction::SchedulePublish,
        )
    })
    .await
    .map_err(join_error)?
    .map_err(map_api_error)?;

    crate::poll_processing_publish_job(state.clone(), job_id.clone())
        .await
        .map_err(|error| {
            (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({ "error": error })),
            )
        })?;

    let (actor, owner) = desktop_auth(&state, &headers).await?;
    let store_handle = state.store.clone();
    let job = tokio::task::spawn_blocking(move || {
        let store = store_handle.open();
        SocialApi::publish_job(&store, &actor, &owner, &job_id)
    })
    .await
    .map_err(join_error)?
    .map_err(map_api_error)?;
    Ok(Json(job))
}

// ── POST /social/jobs/{id}/reschedule ──────────────────────────────────────

pub(crate) async fn reschedule_job_handler(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Path(job_id): Path<String>,
    Json(mut req): Json<RescheduleJobRequest>,
) -> HttpResult<PublishJobResponse> {
    let (actor, owner) = desktop_auth(&state, &headers).await?;
    let store_handle = state.store.clone();
    req.now = now_secs();
    let job = tokio::task::spawn_blocking(move || {
        let mut store = store_handle.open();
        SocialApi::reschedule_job(&mut store, &actor, &owner, &job_id, req)
    })
    .await
    .map_err(join_error)?
    .map_err(map_api_error)?;
    Ok(Json(job))
}

// ── Upload handshake ────────────────────────────────────────────────────────

#[derive(Serialize)]
pub(crate) struct UploadUrlResponse {
    /// Signed PUT URL the desktop streams the rendered file to.
    pub url: String,
    pub method: String,
    /// Opaque storage ref the desktop echoes back to `upload-complete`.
    pub storage_ref: String,
    /// When true the desktop must POST multipart to a server proxy instead
    /// (reserved; always false for the signed-URL path).
    pub direct: bool,
}

/// `POST /social/jobs/{id}/upload-url` — mint a Supabase Storage signed PUT URL
/// for the job's rendered artifact. The job-firing (provider upload) stays
/// server-side; this only stages the bytes the worker later reads.
pub(crate) async fn upload_url_handler(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Path(job_id): Path<String>,
) -> HttpResult<UploadUrlResponse> {
    let (actor, owner) = desktop_auth(&state, &headers).await?;

    // Authorize: staging rendered bytes is part of the publish workflow.
    let store_handle = state.store.clone();
    let job_id_for_check = job_id.clone();
    tokio::task::spawn_blocking(move || {
        let store = store_handle.open();
        SocialApi::authorize_publish_job_action(
            &store,
            &actor,
            &owner,
            &job_id_for_check,
            TeamAction::SchedulePublish,
        )
    })
    .await
    .map_err(join_error)?
    .map_err(map_api_error)?;

    if state.config.supabase_url.is_empty() || state.config.supabase_service_key.is_empty() {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({"error": "storage not configured"})),
        ));
    }

    let bucket = state.config.storage_bucket.clone();
    let object_path = artifact_object_path(&job_id);
    let api_url = format!(
        "{}/storage/v1/object/upload/sign/{bucket}/{object_path}",
        state.config.supabase_url
    );
    let client = reqwest::Client::new();
    let resp = client
        .post(&api_url)
        .header(
            "Authorization",
            format!("Bearer {}", state.config.supabase_service_key),
        )
        .header("Content-Type", "application/json")
        .json(&serde_json::json!({ "expiresIn": 3600 }))
        .send()
        .await
        .map_err(|e| {
            (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({"error": e.to_string()})),
            )
        })?;

    if !resp.status().is_success() {
        let status = resp.status();
        let detail = resp.text().await.unwrap_or_default();
        return Err((
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({"error": format!("storage API {status}: {detail}")})),
        ));
    }
    let json: serde_json::Value = resp.json().await.map_err(|e| {
        (
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({"error": e.to_string()})),
        )
    })?;
    let signed_path = json["url"].as_str().unwrap_or("").to_string();
    let url = format!("{}/storage/v1{signed_path}", state.config.supabase_url);
    let storage_ref = artifact_storage_ref(&bucket, &job_id);

    Ok(Json(UploadUrlResponse {
        url,
        method: "PUT".into(),
        storage_ref,
        direct: false,
    }))
}

/// `POST /social/jobs/{id}/upload-complete` — mark the staged artifact ready.
///
/// SECURITY: the storage ref is regenerated server-side from `(bucket, job_id)`
/// — never taken from the request body. The worker later reads `artifact_ref`
/// as a file/storage path, so accepting a client-supplied value would be an
/// arbitrary-file-read sink (e.g. `file:///etc/passwd`). No body is accepted.
pub(crate) async fn upload_complete_handler(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Path(job_id): Path<String>,
) -> HttpResult<PublishJobResponse> {
    let (actor, owner) = desktop_auth(&state, &headers).await?;
    let store_handle = state.store.clone();
    let bucket = state.config.storage_bucket.clone();

    let job = tokio::task::spawn_blocking(move || {
        let mut store = store_handle.open();
        SocialApi::authorize_publish_job_action(
            &store,
            &actor,
            &owner,
            &job_id,
            TeamAction::SchedulePublish,
        )?;
        let mut job = store.publish_job(&job_id)?;
        // Regenerated server-side — the client cannot influence this path.
        job.artifact_ref = artifact_storage_ref(&bucket, &job_id);
        store.save_publish_job(job)?;
        // Return the refreshed public response (re-reads + re-authorizes).
        SocialApi::publish_job(&store, &actor, &owner, &job_id)
    })
    .await
    .map_err(join_error)?
    .map_err(map_api_error)?;
    Ok(Json(job))
}

/// The provider registry used by validate/schedule. A fresh default registry is
/// cheap and stateless; building it per request avoids threading the shared one
/// into every blocking closure.
fn state_registry() -> montage_social::provider::ProviderRegistry {
    montage_social::provider::ProviderRegistry::default_multi_platform()
}

struct OAuthHandles {
    connection_id: String,
    raw_state: String,
}

fn oauth_handles(provider_str: &str) -> OAuthHandles {
    let connection_id = format!("oauthconn-{provider_str}-{}", random_token_128());
    OAuthHandles {
        raw_state: format!("{connection_id}~{}", random_token_128()),
        connection_id,
    }
}

fn random_token_128() -> String {
    use rand::TryRngCore;
    let mut buf = [0u8; 16];
    rand::rngs::OsRng
        .try_fill_bytes(&mut buf)
        .unwrap_or_else(|e| panic!("OS CSPRNG unavailable: {e}"));
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

/// The storage object path for a job's artifact. Derived solely from
/// `(bucket, job_id)` — never from client input — so the worker can only ever
/// read the artifact the server itself staged.
fn artifact_object_path(job_id: &str) -> String {
    format!("jobs/{job_id}/artifact.mp4")
}

/// The opaque storage ref recorded on the job. Regenerated server-side from
/// `(bucket, job_id)` so a client can never point the worker at an arbitrary
/// path (e.g. `file:///etc/passwd`).
fn artifact_storage_ref(bucket: &str, job_id: &str) -> String {
    format!(
        "supabase-storage://{bucket}/{}",
        artifact_object_path(job_id)
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use montage_social::model::OwnerRef;
    use montage_social::store::SocialStoreError;

    fn headers_with_auth(value: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert("authorization", value.parse().unwrap());
        h
    }

    #[test]
    fn desktop_token_accepts_matching_bearer() {
        assert!(desktop_token_ok(
            "dev-token",
            &headers_with_auth("Bearer dev-token")
        ));
    }

    #[test]
    fn desktop_token_rejects_wrong_bearer() {
        assert!(!desktop_token_ok(
            "dev-token",
            &headers_with_auth("Bearer nope")
        ));
    }

    #[test]
    fn desktop_token_rejects_missing_header() {
        assert!(!desktop_token_ok("dev-token", &HeaderMap::new()));
    }

    #[test]
    fn desktop_token_fails_closed_when_unconfigured() {
        // Empty configured token must never accept any bearer, even empty.
        assert!(!desktop_token_ok("", &headers_with_auth("Bearer ")));
        assert!(!desktop_token_ok("", &HeaderMap::new()));
    }

    #[test]
    fn social_user_allowlist_allows_everyone_when_empty() {
        assert!(social_user_allowed("user_1", &[]));
    }

    #[test]
    fn social_user_allowlist_accepts_configured_user() {
        let allowed = vec!["user_1".to_string(), "cohost_1".to_string()];

        assert!(social_user_allowed("cohost_1", &allowed));
    }

    #[test]
    fn social_user_allowlist_rejects_unlisted_user() {
        let allowed = vec!["user_1".to_string(), "cohost_1".to_string()];

        assert!(!social_user_allowed("random_user", &allowed));
    }

    #[test]
    fn target_owner_defaults_to_authenticated_user() {
        assert_eq!(
            target_owner_from_headers("user_1", &HeaderMap::new()).owner,
            OwnerRef::User("user_1".into())
        );
    }

    #[test]
    fn target_owner_uses_workspace_header() {
        let mut headers = HeaderMap::new();
        headers.insert("x-montage-workspace-id", "workspace_1".parse().unwrap());

        assert_eq!(
            target_owner_from_headers("user_1", &headers).owner,
            OwnerRef::Workspace("workspace_1".into())
        );
    }

    #[test]
    fn random_token_128_is_long_and_unique() {
        let a = random_token_128();
        let b = random_token_128();
        assert_eq!(a.len(), 32, "16 bytes hex-encoded");
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b, "two draws must differ (CSPRNG)");
    }

    #[test]
    fn oauth_handles_keep_twitter_x_state_within_pkce_verifier_limit() {
        let handles = oauth_handles("twitter_x");

        assert!(handles.connection_id.starts_with("oauthconn-twitter_x-"));
        assert!(
            handles
                .raw_state
                .starts_with(&format!("{}~", handles.connection_id))
        );
        assert!(
            (43..=128).contains(&handles.raw_state.len()),
            "Twitter/X reuses raw OAuth state as PKCE verifier, got {} chars",
            handles.raw_state.len()
        );
    }

    #[test]
    fn artifact_storage_ref_is_derived_from_bucket_and_job() {
        assert_eq!(
            artifact_storage_ref("renders", "job-9"),
            "supabase-storage://renders/jobs/job-9/artifact.mp4"
        );
    }

    #[test]
    fn api_error_status_mapping() {
        assert_eq!(
            map_api_error(SocialApiError::Unauthorized).0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            map_api_error(SocialApiError::Store(SocialStoreError::NotFound)).0,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            map_api_error(SocialApiError::Publish("bad".into())).0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
        assert_eq!(
            map_api_error(SocialApiError::Store(SocialStoreError::Storage("x".into()))).0,
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }
}
