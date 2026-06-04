//! Phase 5 user-facing routes — the API surface the desktop client calls.
//!
//! These mirror the `SocialApi` facade methods one-for-one. They are authed by
//! a single-user **desktop dev bearer** (`DESKTOP_AUTH_TOKEN`) that maps to a
//! fixed `ApiActor`/`ApiOwner` (`DESKTOP_USER_ID`). Phase 7 replaces this with
//! real Supabase Auth; until then the bearer is a static dev token the server
//! also accepts. No secret (client_secret / refresh token / access token) is
//! ever returned — the `SocialApi` DTOs are already redaction-tested.
//!
//! The sync `SocialApi` runs on the blocking pool via `spawn_blocking`, exactly
//! like the internal routes.

use crate::{SharedState, bearer_auth, now_secs, parse_provider, provider_client_id, redirect_uri};
use awidat_social::{
    api::{
        AccountSummary, ApiActor, ApiOwner, BindTargetRequest, OAuthStartRequest,
        OAuthStartResponse, PublishJobResponse, ScheduleTargetRequest, SocialApi, SocialApiError,
        ValidateTargetRequest,
    },
    model::AccountUsageAudit,
    oauth_url::OAuthProviderConfig,
    pg_store::PgSocialStore,
    store::SocialStore,
};
use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
};
use serde::{Deserialize, Serialize};

type HttpError = (StatusCode, Json<serde_json::Value>);
type HttpResult<T> = Result<Json<T>, HttpError>;

// ── Auth + error mapping ────────────────────────────────────────────────────

/// Authenticate the desktop dev bearer and return the single-user actor+owner.
///
/// Fails closed if `DESKTOP_AUTH_TOKEN` is unset (empty), so a misconfigured
/// deployment never accepts an empty bearer.
fn desktop_auth(
    state: &SharedState,
    headers: &HeaderMap,
) -> Result<(ApiActor, ApiOwner), HttpError> {
    if !desktop_token_ok(&state.config.desktop_auth_token, headers) {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error": "unauthorized"})),
        ));
    }
    let user_id = state.config.desktop_user_id.clone();
    // Single-user pre-Phase-7: no workspace roles; everything is owned by the
    // user directly.
    Ok((
        ApiActor::new(user_id.clone(), Vec::new()),
        ApiOwner::user(user_id),
    ))
}

/// Pure predicate for the dev-bearer check. Fails closed on an empty configured
/// token so a misconfigured deployment never accepts an empty bearer.
fn desktop_token_ok(configured: &str, headers: &HeaderMap) -> bool {
    !configured.is_empty() && bearer_auth(headers, configured)
}

/// Map a domain `SocialApiError` to an HTTP status + JSON body. Never leaks
/// token material (the facade already redacts).
fn map_api_error(e: SocialApiError) -> HttpError {
    let status = match &e {
        SocialApiError::Unauthorized => StatusCode::FORBIDDEN,
        SocialApiError::Store(awidat_social::store::SocialStoreError::NotFound) => {
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
    let (actor, owner) = desktop_auth(&state, &headers)?;
    let pool = state.pool.clone();
    let accounts = tokio::task::spawn_blocking(move || {
        let store = PgSocialStore::new(pool);
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
    let (actor, owner) = desktop_auth(&state, &headers)?;
    let provider = parse_provider(&provider_str)?;
    let client_id = provider_client_id(&state.config, &provider)?;
    let redirect_uri = redirect_uri(&state.config, &provider);
    let return_to = body.map(|b| b.0.return_to).unwrap_or_default();

    // Server owns the connection id + CSRF state — the desktop never supplies them.
    let now = now_secs();
    let connection_id = format!("oauthconn-{provider_str}-{now}");
    let raw_state = format!("st-{connection_id}-{now}");
    let config = OAuthProviderConfig {
        client_id,
        redirect_uri,
    };
    let pool = state.pool.clone();

    let resp = tokio::task::spawn_blocking(move || {
        let mut store = PgSocialStore::new(pool);
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
    let (actor, owner) = desktop_auth(&state, &headers)?;
    let pool = state.pool.clone();
    let now = now_secs();
    let account = tokio::task::spawn_blocking(move || {
        let mut store = PgSocialStore::new(pool);
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
    let (actor, owner) = desktop_auth(&state, &headers)?;
    let pool = state.pool.clone();
    let audit = tokio::task::spawn_blocking(move || {
        let store = PgSocialStore::new(pool);
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
) -> HttpResult<awidat_social::model::CampaignVariantTarget> {
    let (actor, _owner) = desktop_auth(&state, &headers)?;
    let pool = state.pool.clone();
    let target = tokio::task::spawn_blocking(move || {
        let mut store = PgSocialStore::new(pool);
        SocialApi::bind_target(&mut store, &actor, req)
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
) -> HttpResult<awidat_social::model::CampaignVariantTarget> {
    let (actor, _owner) = desktop_auth(&state, &headers)?;
    let pool = state.pool.clone();
    let target = tokio::task::spawn_blocking(move || {
        let mut store = PgSocialStore::new(pool);
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
    let (actor, _owner) = desktop_auth(&state, &headers)?;
    let pool = state.pool.clone();
    let job = tokio::task::spawn_blocking(move || {
        let mut store = PgSocialStore::new(pool);
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
    let (actor, owner) = desktop_auth(&state, &headers)?;
    let pool = state.pool.clone();
    let job = tokio::task::spawn_blocking(move || {
        let store = PgSocialStore::new(pool);
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
    let (actor, owner) = desktop_auth(&state, &headers)?;
    let pool = state.pool.clone();
    let now = now_secs();
    let job = tokio::task::spawn_blocking(move || {
        let mut store = PgSocialStore::new(pool);
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
    let (actor, owner) = desktop_auth(&state, &headers)?;
    let pool = state.pool.clone();
    let now = now_secs();
    let job = tokio::task::spawn_blocking(move || {
        let mut store = PgSocialStore::new(pool);
        SocialApi::retry_job(&mut store, &actor, &owner, &job_id, now)
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
    let (actor, owner) = desktop_auth(&state, &headers)?;

    // Authorize: the caller must be able to read the job (owner match).
    let pool = state.pool.clone();
    let job_id_for_check = job_id.clone();
    tokio::task::spawn_blocking(move || {
        let store = PgSocialStore::new(pool);
        SocialApi::publish_job(&store, &actor, &owner, &job_id_for_check)
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
    let object_path = format!("jobs/{job_id}/artifact.mp4");
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
    let storage_ref = format!("supabase-storage://{bucket}/{object_path}");

    Ok(Json(UploadUrlResponse {
        url,
        method: "PUT".into(),
        storage_ref,
        direct: false,
    }))
}

#[derive(Deserialize)]
pub(crate) struct UploadCompleteBody {
    pub storage_ref: String,
}

/// `POST /social/jobs/{id}/upload-complete` — record the staged storage ref on
/// the job so the worker reads the right artifact when it fires.
pub(crate) async fn upload_complete_handler(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Path(job_id): Path<String>,
    Json(body): Json<UploadCompleteBody>,
) -> HttpResult<PublishJobResponse> {
    let (actor, owner) = desktop_auth(&state, &headers)?;
    let pool = state.pool.clone();
    let storage_ref = body.storage_ref;

    let job = tokio::task::spawn_blocking(move || {
        let mut store = PgSocialStore::new(pool);
        // Authorize via the read path before mutating anything.
        SocialApi::publish_job(&store, &actor, &owner, &job_id)?;
        let mut job = store.publish_job(&job_id)?;
        job.artifact_ref = storage_ref;
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
fn state_registry() -> awidat_social::provider::ProviderRegistry {
    awidat_social::provider::ProviderRegistry::default_multi_platform()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use awidat_social::store::SocialStoreError;

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
