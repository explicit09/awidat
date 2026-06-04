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
    auth_context::JwtVerifier,
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

/// Authenticate a request and return its per-user actor + self-owner.
///
/// Two modes (Phase 7):
/// - **Supabase Auth (when `SUPABASE_JWT_SECRET` is set):** verify the bearer as
///   a Supabase JWT → real per-user `user_id`. This is the multi-user path.
/// - **Dev bearer (fallback):** compare the bearer to `DESKTOP_AUTH_TOKEN` →
///   the single fixed `DESKTOP_USER_ID`. Pre-Supabase single-user dev.
///
/// Workspace roles are left empty: the desktop only targets the caller's own
/// resources (`OwnerRef::User`), and `TeamPolicy` grants the owner self-access
/// without any workspace role. A future workspace-admin surface would load roles
/// via `workspace_member_roles_for_user` and pass an `OwnerRef::Workspace`.
fn desktop_auth(
    state: &SharedState,
    headers: &HeaderMap,
) -> Result<(ApiActor, ApiOwner), HttpError> {
    let user_id = authenticated_user_id(state, headers)?;
    Ok((
        ApiActor::new(user_id.clone(), Vec::new()),
        ApiOwner::user(user_id),
    ))
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

    // Server owns the connection id + CSRF state — the desktop never supplies
    // them. SECURITY: both carry CSPRNG entropy (not a guessable timestamp) so
    // the OAuth `state` is unforgeable and the connection handle is unguessable.
    let now = now_secs();
    let connection_id = format!("oauthconn-{provider_str}-{}", random_token());
    let raw_state = format!("st-{}", random_token());
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
    let (actor, owner) = desktop_auth(&state, &headers)?;
    let pool = state.pool.clone();
    let bucket = state.config.storage_bucket.clone();

    let job = tokio::task::spawn_blocking(move || {
        let mut store = PgSocialStore::new(pool);
        // Authorize via the read path before mutating anything.
        SocialApi::publish_job(&store, &actor, &owner, &job_id)?;
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
fn state_registry() -> awidat_social::provider::ProviderRegistry {
    awidat_social::provider::ProviderRegistry::default_multi_platform()
}

/// 32 bytes (256 bits) of CSPRNG entropy, hex-encoded. Used for the OAuth CSRF
/// `state` and the connection handle so neither is guessable.
fn random_token() -> String {
    use rand::TryRngCore;
    let mut buf = [0u8; 32];
    // OsRng pulls directly from the OS CSPRNG. try_fill_bytes only errors if the
    // OS entropy source is unavailable, which is fatal — fail loudly.
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
    fn random_token_is_long_and_unique() {
        let a = random_token();
        let b = random_token();
        assert_eq!(a.len(), 64, "32 bytes hex-encoded");
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b, "two draws must differ (CSPRNG)");
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
