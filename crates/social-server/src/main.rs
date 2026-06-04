//! awidat-social-server — Axum HTTP wrapper over the awidat-social domain crate.
//!
//! Phase 1: skeleton with mock upload adapters, real PgSocialStore, and the
//! /internal/tick endpoint code-guarded by SOCIAL_FIRING_ENABLED=false.
//! Phase 2: server-side OAuth exchange (Google/YouTube), AEAD token storage,
//!          and the /oauth/callback/{provider} handler.
//! Phase 3: real YouTube resumable-upload adapter, status client, quota gate,
//!          and production AccessTokenResolver + ArtifactSource.
//! Phase 4: poll-processing + token-refresh cron routes, server TokenRefresher,
//!          and the pg_cron schedules (migration 0004) that drive all three.
//!
//! Environment variables (all required at runtime):
//!   DATABASE_URL            — Supavisor session-pooler URL
//!   SERVICE_SHARED_SECRET   — bearer token that pg_net sends to /internal/tick
//!   BIND_ADDR               — e.g. "0.0.0.0:3000" (default "0.0.0.0:3000")
//!   SOCIAL_FIRING_ENABLED   — "true" enables real job execution (default "false")
//!   SUPABASE_URL            — Supabase project URL (for Storage signed URLs)
//!   SUPABASE_SERVICE_KEY    — service_role key (for Storage signed URL minting)
//!   STORAGE_BUCKET          — name of the Supabase Storage bucket for artifacts
//!   GOOGLE_CLIENT_ID        — Google OAuth client ID (Phase 2)
//!   GOOGLE_CLIENT_SECRET    — Google OAuth client secret (Phase 2; server-only, never in desktop)
//!   SOCIAL_TOKEN_AEAD_KEY   — 64 hex chars = 32-byte ChaCha20-Poly1305 key (Phase 2)
//!   SOCIAL_TOKEN_KEY_ID     — key identifier stored alongside every token (Phase 2)
//!   OAUTH_REDIRECT_BASE     — base URL for OAuth redirect URIs, e.g. "https://awidat-social.fly.dev"
//!   YOUTUBE_FORCE_PRIVATE   — "false" allows non-private uploads (default "true"; keep true pre-audit)
//!   ARTIFACT_BASE_DIR       — root dir for file:// artifact refs (default "/var/lib/awidat-artifacts")

mod artifact_source;
mod token_refresher;
mod token_resolver;
mod user_routes;

use awidat_social::{
    account_service::{CompleteOAuthInput, SocialAccountService},
    api::{ExecuteUploadRequest, SocialApi},
    model::{ConnectedAccount, OwnerRef, Provider},
    oauth_exchange::{
        GoogleOAuthExchange, GoogleOAuthExchangeConfig, OAuthTokenExchange, TokenExchangeInput,
    },
    oauth_url::OAuthProviderConfig,
    pg_store::PgSocialStore,
    provider::ProviderRegistry,
    store::SocialStore,
    token::{Aead256Key, LocalTokenKeyProvider},
    token_bundle::ProviderTokenBundle,
    token_refresh::{TokenRefreshError, TokenRefresher},
    youtube_upload::{
        YouTubeClientConfig, YouTubeStatusAdapter, YouTubeUploadAdapter,
        live::{LiveYouTubeStatusClient, LiveYouTubeUploadClient},
    },
};
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
};
use constant_time_eq::constant_time_eq;
use r2d2::Pool;
use r2d2_postgres::PostgresConnectionManager;
use r2d2_postgres::postgres::NoTls;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use token_resolver::ServerAccessTokenResolver;
use tracing::info;

// ── Server config ─────────────────────────────────────────────────────────────

#[derive(Clone)]
pub(crate) struct ServerConfig {
    service_shared_secret: String,
    pub(crate) social_firing_enabled: bool,
    pub(crate) supabase_url: String,
    pub(crate) supabase_service_key: String,
    pub(crate) storage_bucket: String,
    oauth_redirect_base: String,
    // Phase 2: Google OAuth credentials.
    google_client_id: String,
    google_client_secret: String,
    // Phase 2: AEAD token encryption.
    token_key_id: String,
    token_key_hex: String,
    // Phase 3: YouTube upload config.
    // When true, forces all uploads to private regardless of job privacy setting.
    // Must be true until the YouTube TOS audit clears.
    youtube_force_private: bool,
    // Phase 3: artifact root. `file://` artifact refs are confined to this
    // directory (path-traversal defense). Phase 5 replaces local files with
    // Supabase Storage signed URLs.
    artifact_base_dir: String,
    // Phase 5: desktop client auth (pre-Phase-7 single-user dev bearer).
    // The desktop sends `Authorization: Bearer <desktop_auth_token>` to the
    // user-facing `/social/*` routes; it maps to the fixed `desktop_user_id`.
    // Phase 7 replaces this with real Supabase Auth.
    pub(crate) desktop_auth_token: String,
    pub(crate) desktop_user_id: String,
}

// ── App state ─────────────────────────────────────────────────────────────────

/// All routes share this state.
/// `spawn_blocking` moves a clone of the pool so the sync domain layer runs
/// on the blocking thread pool without holding any async lock across awaits.
pub(crate) struct AppState {
    pub(crate) pool: Pool<PostgresConnectionManager<NoTls>>,
    pub(crate) registry: ProviderRegistry,
    pub(crate) config: ServerConfig,
}

pub(crate) type SharedState = Arc<AppState>;

// ── Entry point ───────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let database_url = env_required("DATABASE_URL");
    let service_shared_secret = env_required("SERVICE_SHARED_SECRET");
    let bind_addr = std::env::var("BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:3000".into());
    let social_firing_enabled = std::env::var("SOCIAL_FIRING_ENABLED")
        .map(|v| v == "true")
        .unwrap_or(false);
    let supabase_url = std::env::var("SUPABASE_URL").unwrap_or_default();
    let supabase_service_key = std::env::var("SUPABASE_SERVICE_KEY").unwrap_or_default();
    let storage_bucket = std::env::var("STORAGE_BUCKET").unwrap_or_else(|_| "artifacts".into());
    let oauth_redirect_base = std::env::var("OAUTH_REDIRECT_BASE").unwrap_or_default();
    let google_client_id = std::env::var("GOOGLE_CLIENT_ID").unwrap_or_default();
    let google_client_secret = std::env::var("GOOGLE_CLIENT_SECRET").unwrap_or_default();
    let token_key_hex = std::env::var("SOCIAL_TOKEN_AEAD_KEY").unwrap_or_default();
    let token_key_id = std::env::var("SOCIAL_TOKEN_KEY_ID").unwrap_or_else(|_| "k1".into());
    // Default true: force private until the YouTube TOS audit clears.
    let youtube_force_private = std::env::var("YOUTUBE_FORCE_PRIVATE")
        .map(|v| v != "false")
        .unwrap_or(true);
    let artifact_base_dir =
        std::env::var("ARTIFACT_BASE_DIR").unwrap_or_else(|_| "/var/lib/awidat-artifacts".into());
    // Phase 5 desktop dev bearer (single-user until Phase 7 Supabase Auth).
    let desktop_auth_token = std::env::var("DESKTOP_AUTH_TOKEN").unwrap_or_default();
    let desktop_user_id =
        std::env::var("DESKTOP_USER_ID").unwrap_or_else(|_| "desktop-user".into());

    info!(
        social_firing_enabled,
        "awidat-social-server starting — firing enabled: {social_firing_enabled}"
    );

    let manager = PostgresConnectionManager::new(
        database_url
            .parse()
            .unwrap_or_else(|e| panic!("parse DATABASE_URL: {e}")),
        NoTls,
    );
    let pool = Pool::builder()
        .max_size(10)
        .build(manager)
        .unwrap_or_else(|e| panic!("build connection pool: {e}"));

    // Apply migrations on boot so Supabase projects are always schema-current.
    let store = PgSocialStore::new(pool.clone());
    let migrations_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap_or_else(|| panic!("crates/social-server has no parent directory"))
        .join("social/migrations");
    store
        .apply_migrations(&migrations_dir)
        .unwrap_or_else(|e| panic!("apply migrations: {e}"));

    let state = Arc::new(AppState {
        pool,
        registry: ProviderRegistry::default_multi_platform(),
        config: ServerConfig {
            service_shared_secret,
            social_firing_enabled,
            supabase_url,
            supabase_service_key,
            storage_bucket,
            oauth_redirect_base,
            google_client_id,
            google_client_secret,
            token_key_id,
            token_key_hex,
            youtube_force_private,
            artifact_base_dir,
            desktop_auth_token,
            desktop_user_id,
        },
    });

    let app = Router::new()
        .route("/health", get(health_handler))
        .route("/providers", get(providers_handler))
        .route("/artifacts/upload-url", post(artifacts_upload_url_handler))
        .route("/oauth/begin/{provider}", post(oauth_begin_handler))
        .route("/oauth/callback/{provider}", get(oauth_callback_handler))
        // Phase 5 user-facing routes (desktop dev-bearer auth).
        .route("/social/accounts", get(user_routes::accounts_handler))
        .route(
            "/social/oauth/start/{provider}",
            post(user_routes::oauth_start_handler),
        )
        .route(
            "/social/accounts/{account_id}/disconnect",
            post(user_routes::disconnect_handler),
        )
        .route(
            "/social/accounts/{account_id}/audit",
            get(user_routes::account_audit_handler),
        )
        .route(
            "/social/targets/bind",
            post(user_routes::bind_target_handler),
        )
        .route(
            "/social/targets/validate",
            post(user_routes::validate_target_handler),
        )
        .route(
            "/social/targets/schedule",
            post(user_routes::schedule_target_handler),
        )
        .route("/social/jobs/{job_id}", get(user_routes::job_handler))
        .route(
            "/social/jobs/{job_id}/cancel",
            post(user_routes::cancel_job_handler),
        )
        .route(
            "/social/jobs/{job_id}/retry",
            post(user_routes::retry_job_handler),
        )
        .route(
            "/social/jobs/{job_id}/upload-url",
            post(user_routes::upload_url_handler),
        )
        .route(
            "/social/jobs/{job_id}/upload-complete",
            post(user_routes::upload_complete_handler),
        )
        .route("/internal/tick", post(internal_tick_handler))
        .route(
            "/internal/cron/poll-processing",
            post(internal_poll_processing_handler),
        )
        .route(
            "/internal/cron/refresh-tokens",
            post(internal_refresh_tokens_handler),
        )
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(&bind_addr)
        .await
        .unwrap_or_else(|e| panic!("bind {bind_addr}: {e}"));
    info!("listening on {bind_addr}");
    axum::serve(listener, app)
        .await
        .unwrap_or_else(|e| panic!("serve: {e}"));
}

/// Maximum YouTube Data API uploads per day per project (hard Google quota).
const YOUTUBE_DAILY_QUOTA: usize = 100;

/// The refresh sweep refreshes any token expiring within this window (15 min),
/// chosen larger than the cron interval so no due upload finds a dead token.
const TOKEN_REFRESH_SWEEP_SKEW_SECS: i64 = 900;

fn env_required(key: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| panic!("required env var {key} not set"))
}

/// Clone an `Aead256Key` by re-parsing its hex representation.
/// `Aead256Key` is not `Clone`; this helper is used to move a copy into a closure.
pub(crate) fn aead_key_clone(key: &Aead256Key) -> Aead256Key {
    // Encode the 32 raw bytes back to hex and re-parse.
    // We use the key_id "k" as a placeholder — the resolver only uses the bytes.
    let bytes: &[u8] = key.key_material();
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    Aead256Key::from_hex(key.key_id(), &hex)
        .unwrap_or_else(|_| panic!("aead_key_clone: key material is not 32 bytes"))
}

pub(crate) fn aead_key_from_state(
    config: &ServerConfig,
) -> Result<Aead256Key, (StatusCode, Json<serde_json::Value>)> {
    if config.token_key_hex.is_empty() {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({"error": "token encryption not configured"})),
        ));
    }
    Aead256Key::from_hex(&config.token_key_id, &config.token_key_hex).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("bad AEAD key: {e}")})),
        )
    })
}

pub(crate) fn bearer_auth(headers: &HeaderMap, secret: &str) -> bool {
    let auth = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("");
    constant_time_eq(auth.as_bytes(), secret.as_bytes())
}

// ── Handlers ──────────────────────────────────────────────────────────────────

async fn health_handler() -> Json<serde_json::Value> {
    Json(serde_json::json!({"status": "ok", "service": "awidat-social-server"}))
}

async fn providers_handler(State(state): State<SharedState>) -> Json<serde_json::Value> {
    let providers = SocialApi::providers(&state.registry);
    Json(serde_json::json!({"providers": providers}))
}

// ── OAuth begin ───────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct OAuthBeginRequest {
    owner_id: String,
    owner_kind: String,
    connection_id: String,
    state: String,
    return_to: String,
    created_at: i64,
    expires_at: i64,
}

#[derive(Serialize)]
struct OAuthBeginResponse {
    authorization_url: String,
    connection_id: String,
}

/// `POST /oauth/begin/{provider}` — build an OAuth authorization URL and
/// persist the pending `OAuthConnection` record.
///
/// Protected by the `SERVICE_SHARED_SECRET` bearer token.
async fn oauth_begin_handler(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Path(provider_str): Path<String>,
    Json(body): Json<OAuthBeginRequest>,
) -> Result<Json<OAuthBeginResponse>, (StatusCode, Json<serde_json::Value>)> {
    if !bearer_auth(&headers, &state.config.service_shared_secret) {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error": "unauthorized"})),
        ));
    }

    let provider = parse_provider(&provider_str)?;
    let client_id = provider_client_id(&state.config, &provider)?;
    let redirect_uri = redirect_uri(&state.config, &provider);
    let config = OAuthProviderConfig {
        client_id,
        redirect_uri,
    };
    let owner = parse_owner(&body.owner_kind, &body.owner_id)?;

    let pool = state.pool.clone();
    let connection_id = body.connection_id.clone();
    let state_str = body.state.clone();
    let return_to = body.return_to.clone();
    let created_at = body.created_at;
    let expires_at = body.expires_at;

    let result = tokio::task::spawn_blocking(move || {
        let mut store = PgSocialStore::new(pool);
        SocialAccountService::start_oauth(
            &mut store,
            connection_id,
            owner,
            provider,
            &config,
            &state_str,
            return_to,
            created_at,
            expires_at,
        )
    })
    .await
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("join error: {e}")})),
        )
    })?
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
    })?;

    Ok(Json(OAuthBeginResponse {
        authorization_url: result.authorization_url,
        connection_id: result.connection.id,
    }))
}

// ── OAuth callback ────────────────────────────────────────────────────────────

/// OAuth callback query parameters.
///
/// SECURITY: only the standard OAuth fields (`code`, `state`) plus our
/// `connection_id` handle are accepted from the client. Owner identity,
/// account id, display name, and `now` are all derived server-side from the
/// stored `OAuthConnection` and the provider response — never trusted from the
/// query string. Accepting owner/account from the client would be an IDOR /
/// account-takeover vector.
#[derive(Deserialize)]
struct OAuthCallbackQuery {
    code: String,
    state: String,
    connection_id: String,
}

/// `GET /oauth/callback/{provider}` — exchange the code, store encrypted tokens.
///
/// The desktop app redirects here after the provider grants access. This
/// handler performs the server-side code exchange (keeping `client_secret`
/// off the desktop) and stores tokens encrypted with ChaCha20-Poly1305.
async fn oauth_callback_handler(
    State(state): State<SharedState>,
    Path(provider_str): Path<String>,
    Query(q): Query<OAuthCallbackQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let provider = parse_provider(&provider_str)?;
    let key = aead_key_from_state(&state.config)?;
    let redirect_uri = redirect_uri(&state.config, &provider);

    // Exchange the authorization code for tokens (async, hits provider API).
    let output = match &provider {
        Provider::YouTube => {
            if state.config.google_client_id.is_empty()
                || state.config.google_client_secret.is_empty()
            {
                return Err((
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(serde_json::json!({"error": "Google OAuth not configured"})),
                ));
            }
            let exchange = GoogleOAuthExchange::new(GoogleOAuthExchangeConfig {
                client_id: state.config.google_client_id.clone(),
                client_secret: state.config.google_client_secret.clone(),
            });
            exchange
                .exchange(TokenExchangeInput {
                    provider: provider.clone(),
                    code: q.code.clone(),
                    redirect_uri: redirect_uri.clone(),
                })
                .await
                .map_err(|e| {
                    (
                        StatusCode::BAD_GATEWAY,
                        Json(serde_json::json!({"error": e.to_string()})),
                    )
                })?
        }
        Provider::TikTok | Provider::Instagram => {
            return Err((
                StatusCode::NOT_IMPLEMENTED,
                Json(serde_json::json!({"error": "provider not yet supported in Phase 2"})),
            ));
        }
    };

    let token_response = output.token_response;
    let access_token = output.access_token;
    let refresh_token = output.refresh_token;
    // SECURITY: server-authoritative timestamp; never trust a client-supplied clock.
    let now = now_secs();
    let connection_id = q.connection_id.clone();
    let raw_state = q.state.clone();
    let provider_account_id = token_response.provider_account_id.clone();
    let pool = state.pool.clone();
    let provider_for_blocking = provider.clone();

    let bundle = ProviderTokenBundle::from_oauth_response(provider.clone(), token_response, now)
        .map_err(|e| {
            (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({"error": format!("token bundle: {e:?}")})),
            )
        })?;

    let account = tokio::task::spawn_blocking(move || {
        let mut store = PgSocialStore::new(pool);

        // SECURITY: derive owner from the stored connection, validated by the
        // unguessable `state` handle — not from the query string. A forged
        // callback can't produce a `state` matching another owner's connection.
        let connection = store
            .oauth_connection(&connection_id)
            .map_err(|e| format!("connection lookup: {e}"))?;
        let owner = connection.owner;

        // SECURITY: account id + display name are server-derived from the
        // provider's own account id, not client input.
        let account_id = format!(
            "{}:{provider_account_id}",
            provider_slug(&provider_for_blocking)
        );
        let display_name = provider_account_id.clone();
        let account = build_connected_account(
            account_id,
            owner,
            provider_for_blocking,
            provider_account_id,
            display_name,
            now,
        );

        SocialAccountService::complete_oauth(
            &mut store,
            &key,
            CompleteOAuthInput {
                oauth_connection_id: connection_id,
                raw_state,
                connected_account: account,
                token_bundle: bundle,
                access_token,
                refresh_token,
                now,
            },
        )
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("join error: {e}")})),
        )
    })?
    .map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": e })),
        )
    })?;

    info!(account_id = %account.id, provider = ?account.provider, "OAuth complete");
    Ok(Json(serde_json::json!({
        "status": "ok",
        "account_id": account.id,
        "provider_account_id": account.provider_account_id,
    })))
}

// ── Artifact upload URL ───────────────────────────────────────────────────────

#[derive(Deserialize)]
struct ArtifactUploadUrlRequest {
    object_path: String,
    expires_in_secs: Option<u32>,
}

#[derive(Serialize)]
struct ArtifactUploadUrlResponse {
    upload_url: String,
    artifact_ref: String,
}

fn is_safe_object_path(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.contains("..")
        && path
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'/'))
}

/// `POST /artifacts/upload-url` (D4)
///
/// Returns a Supabase Storage signed PUT URL and the object key.
/// Protected by `SERVICE_SHARED_SECRET` bearer token.
async fn artifacts_upload_url_handler(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(body): Json<ArtifactUploadUrlRequest>,
) -> Result<Json<ArtifactUploadUrlResponse>, (StatusCode, Json<serde_json::Value>)> {
    if !bearer_auth(&headers, &state.config.service_shared_secret) {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error": "unauthorized"})),
        ));
    }

    if !is_safe_object_path(&body.object_path) {
        return Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({"error": "invalid object_path"})),
        ));
    }

    if state.config.supabase_url.is_empty() || state.config.supabase_service_key.is_empty() {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({"error": "storage not configured"})),
        ));
    }

    let expires_in = body.expires_in_secs.unwrap_or(3600);
    let bucket = &state.config.storage_bucket;
    let object_path = &body.object_path;

    let api_url = format!(
        "{}/storage/v1/object/sign/{bucket}/{object_path}",
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
        .json(&serde_json::json!({ "expiresIn": expires_in }))
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
        let body = resp.text().await.unwrap_or_default();
        return Err((
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({"error": format!("storage API {status}: {body}")})),
        ));
    }

    let json: serde_json::Value = resp.json().await.map_err(|e| {
        (
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({"error": e.to_string()})),
        )
    })?;

    let signed_url = json["signedURL"].as_str().unwrap_or("").to_string();
    let artifact_ref = format!("supabase-storage://{bucket}/{object_path}");

    Ok(Json(ArtifactUploadUrlResponse {
        upload_url: signed_url,
        artifact_ref,
    }))
}

// ── Internal tick ─────────────────────────────────────────────────────────────

/// `POST /internal/tick` — cron trigger from Supabase `pg_net`.
///
/// Protected by the `SERVICE_SHARED_SECRET` bearer token.
/// Code-guarded by `SOCIAL_FIRING_ENABLED=false` (G10): when disabled, logs
/// the tick but performs no job execution.
async fn internal_tick_handler(
    State(state): State<SharedState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if !bearer_auth(&headers, &state.config.service_shared_secret) {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error": "unauthorized"})),
        ));
    }

    if !state.config.social_firing_enabled {
        info!("internal/tick received but SOCIAL_FIRING_ENABLED=false — skipping");
        return Ok(Json(
            serde_json::json!({"status": "noop", "reason": "firing disabled"}),
        ));
    }

    // Resolve AEAD key — needed for token decryption in the resolver.
    let aead_key = aead_key_from_state(&state.config)?;
    let pool = state.pool.clone();
    let force_private = state.config.youtube_force_private;
    let artifact_base_dir = state.config.artifact_base_dir.clone();

    let claimed_count = tokio::task::spawn_blocking(move || {
        let mut store = PgSocialStore::new(pool.clone());
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| format!("clock error: {e}"))?
            .as_secs() as i64;

        // Quota gate: enforce 100 YouTube uploads/day/project before claiming.
        let today_count = store
            .youtube_upload_quota_today(now)
            .map_err(|e| format!("quota check: {e}"))?;
        let youtube_quota_remaining = YOUTUBE_DAILY_QUOTA.saturating_sub(today_count);

        let claimed = store
            .claim_due_publish_jobs(now, 10)
            .map_err(|e| format!("claim: {e}"))?;

        let count = claimed.len();
        let mut youtube_used = 0usize;

        for job in claimed {
            use awidat_social::model::Provider;
            match &job.provider {
                Provider::YouTube => {
                    if youtube_used >= youtube_quota_remaining {
                        tracing::warn!(job_id = %job.id, "YouTube daily quota reached, leaving job Scheduled");
                        continue;
                    }
                    let resolver = ServerAccessTokenResolver::new(pool.clone(), aead_key_clone(&aead_key), now);
                    let artifact_source =
                        artifact_source::FileArtifactSource::new(artifact_base_dir.clone());
                    let yt_config = YouTubeClientConfig { force_private, ..Default::default() };
                    let client = LiveYouTubeUploadClient::new(resolver, artifact_source, yt_config);
                    let adapter = YouTubeUploadAdapter::new(client);
                    if let Err(e) = SocialApi::execute_claimed_upload_job(
                        &mut store,
                        &adapter,
                        ExecuteUploadRequest {
                            job_id: job.id.clone(),
                            title: String::new(),
                            description: None,
                            tags: Vec::new(),
                            thumbnail_ref: None,
                            privacy: None,
                            now,
                        },
                    ) {
                        tracing::warn!(job_id = %job.id, "YouTube execute failed: {e}");
                    } else {
                        youtube_used += 1;
                        let _ = store.increment_youtube_quota(now);
                    }
                }
                Provider::TikTok | Provider::Instagram => {
                    // TODO(phase-6): wire TikTok/Instagram adapters.
                    tracing::info!(job_id = %job.id, provider = ?job.provider, "provider not yet live — skipping");
                }
            }
        }
        Ok::<usize, String>(count)
    })
    .await
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("join error: {e}")})),
        )
    })?
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e})),
        )
    })?;

    info!(claimed_count, "tick processed");
    Ok(Json(
        serde_json::json!({"status": "ok", "claimed": claimed_count}),
    ))
}

/// `POST /internal/cron/poll-processing` — advance `Processing` jobs.
///
/// YouTube's resumable upload returns `processing=true` until the video is
/// transcoded; this sweep polls the status API and moves jobs to
/// `Published`/`Failed`. Protected by `SERVICE_SHARED_SECRET`, code-guarded by
/// `SOCIAL_FIRING_ENABLED`.
async fn internal_poll_processing_handler(
    State(state): State<SharedState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if !bearer_auth(&headers, &state.config.service_shared_secret) {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error": "unauthorized"})),
        ));
    }
    if !state.config.social_firing_enabled {
        return Ok(Json(
            serde_json::json!({"status": "noop", "reason": "firing disabled"}),
        ));
    }

    let aead_key = aead_key_from_state(&state.config)?;
    let pool = state.pool.clone();

    let polled = tokio::task::spawn_blocking(move || {
        let mut store = PgSocialStore::new(pool.clone());
        let now = now_secs();
        let jobs = store
            .processing_publish_jobs(25)
            .map_err(|e| format!("processing query: {e}"))?;
        let mut advanced = 0usize;
        for job in jobs {
            use awidat_social::model::Provider;
            match &job.provider {
                Provider::YouTube => {
                    let resolver = ServerAccessTokenResolver::new(
                        pool.clone(),
                        aead_key_clone(&aead_key),
                        now,
                    );
                    let client = LiveYouTubeStatusClient::new(resolver);
                    let adapter = YouTubeStatusAdapter::new(client);
                    match SocialApi::poll_upload_status(&mut store, &adapter, &job.id, now) {
                        Ok(_) => advanced += 1,
                        Err(e) => {
                            tracing::warn!(job_id = %job.id, "poll status failed: {e}");
                        }
                    }
                }
                Provider::TikTok | Provider::Instagram => {}
            }
        }
        Ok::<usize, String>(advanced)
    })
    .await
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("join error: {e}")})),
        )
    })?
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e })),
        )
    })?;

    Ok(Json(serde_json::json!({"status": "ok", "polled": polled})))
}

/// `POST /internal/cron/refresh-tokens` — pro-actively refresh near-expiry tokens.
///
/// Runs independently of firing so a due post never finds a dead access token.
/// On `invalid_grant` (revoked / expired refresh token) the account is flipped
/// to `NeedsReauth`. Protected by `SERVICE_SHARED_SECRET`.
async fn internal_refresh_tokens_handler(
    State(state): State<SharedState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if !bearer_auth(&headers, &state.config.service_shared_secret) {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error": "unauthorized"})),
        ));
    }
    if state.config.google_client_id.is_empty() || state.config.google_client_secret.is_empty() {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({"error": "Google OAuth not configured"})),
        ));
    }

    let aead_key = aead_key_from_state(&state.config)?;
    let pool = state.pool.clone();
    let client_id = state.config.google_client_id.clone();
    let client_secret = state.config.google_client_secret.clone();

    let summary = tokio::task::spawn_blocking(move || {
        let mut store = PgSocialStore::new(pool);
        let now = now_secs();
        // Refresh anything expiring within the sweep skew window.
        let deadline = now.saturating_add(TOKEN_REFRESH_SWEEP_SKEW_SECS);
        let due = store
            .token_secrets_due_refresh(deadline)
            .map_err(|e| format!("due query: {e}"))?;

        let refresher =
            token_refresher::ServerTokenRefresher::new(client_id, client_secret, aead_key);

        let mut refreshed = 0usize;
        let mut needs_reauth = 0usize;
        let mut failed = 0usize;
        for secret in due {
            let account_id = secret.connected_account_id.clone();
            match refresher.refresh(&account_id, &secret, now) {
                Ok(fresh) => {
                    if store.save_token_secret(fresh).is_ok() {
                        refreshed += 1;
                    } else {
                        failed += 1;
                    }
                }
                Err(TokenRefreshError::InvalidGrant(_)) => {
                    if let Ok(mut account) = store.connected_account(&account_id) {
                        account.status = awidat_social::model::ConnectedAccountStatus::NeedsReauth;
                        account.updated_at = now;
                        let _ = store.save_connected_account(account);
                    }
                    needs_reauth += 1;
                }
                Err(TokenRefreshError::Transient(msg)) => {
                    tracing::warn!(account_id, "token refresh transient error: {msg}");
                    failed += 1;
                }
            }
        }
        Ok::<(usize, usize, usize), String>((refreshed, needs_reauth, failed))
    })
    .await
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("join error: {e}")})),
        )
    })?
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e })),
        )
    })?;

    let (refreshed, needs_reauth, failed) = summary;
    Ok(Json(serde_json::json!({
        "status": "ok",
        "refreshed": refreshed,
        "needs_reauth": needs_reauth,
        "failed": failed,
    })))
}

// ── Helpers ───────────────────────────────────────────────────────────────────

pub(crate) fn parse_provider(s: &str) -> Result<Provider, (StatusCode, Json<serde_json::Value>)> {
    match s {
        "youtube" => Ok(Provider::YouTube),
        "tiktok" => Ok(Provider::TikTok),
        "instagram" => Ok(Provider::Instagram),
        _ => Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": format!("unknown provider: {s}")})),
        )),
    }
}

fn parse_owner(kind: &str, id: &str) -> Result<OwnerRef, (StatusCode, Json<serde_json::Value>)> {
    match kind {
        "user" => Ok(OwnerRef::User(id.to_string())),
        "workspace" => Ok(OwnerRef::Workspace(id.to_string())),
        _ => Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({"error": format!("unknown owner_kind: {kind}")})),
        )),
    }
}

pub(crate) fn provider_client_id(
    config: &ServerConfig,
    provider: &Provider,
) -> Result<String, (StatusCode, Json<serde_json::Value>)> {
    match provider {
        Provider::YouTube => {
            if config.google_client_id.is_empty() {
                Err((
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(serde_json::json!({"error": "Google OAuth not configured"})),
                ))
            } else {
                Ok(config.google_client_id.clone())
            }
        }
        Provider::TikTok | Provider::Instagram => Err((
            StatusCode::NOT_IMPLEMENTED,
            Json(serde_json::json!({"error": "provider not yet supported"})),
        )),
    }
}

pub(crate) fn provider_slug(provider: &Provider) -> &'static str {
    match provider {
        Provider::YouTube => "youtube",
        Provider::TikTok => "tiktok",
        Provider::Instagram => "instagram",
    }
}

pub(crate) fn redirect_uri(config: &ServerConfig, provider: &Provider) -> String {
    format!(
        "{}/oauth/callback/{}",
        config.oauth_redirect_base,
        provider_slug(provider)
    )
}

/// Server-authoritative current Unix time in seconds.
///
/// Used everywhere a timestamp influences a security decision (token expiry,
/// OAuth completion) so a client can never supply its own clock.
pub(crate) fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn build_connected_account(
    id: String,
    owner: OwnerRef,
    provider: Provider,
    provider_account_id: String,
    display_name: String,
    now: i64,
) -> ConnectedAccount {
    use awidat_social::model::{
        AccountEligibility, AccountKind, ConnectedAccountStatus, ProviderCapabilities,
    };
    ConnectedAccount {
        id,
        owner,
        provider,
        provider_account_id,
        display_name,
        handle: None,
        avatar_url: None,
        account_kind: AccountKind::Channel,
        status: ConnectedAccountStatus::Connected,
        scopes: Vec::new(),
        capabilities: ProviderCapabilities::default(),
        eligibility: AccountEligibility::eligible(),
        last_verified_at: None,
        created_at: now,
        updated_at: now,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use axum::http::HeaderMap;

    fn headers_with_auth(value: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert("authorization", value.parse().unwrap());
        h
    }

    #[test]
    fn bearer_auth_accepts_matching_secret() {
        let h = headers_with_auth("Bearer s3cret");
        assert!(bearer_auth(&h, "s3cret"));
    }

    #[test]
    fn bearer_auth_rejects_wrong_secret() {
        let h = headers_with_auth("Bearer wrong");
        assert!(!bearer_auth(&h, "s3cret"));
    }

    #[test]
    fn bearer_auth_rejects_missing_header() {
        assert!(!bearer_auth(&HeaderMap::new(), "s3cret"));
    }

    #[test]
    fn bearer_auth_rejects_missing_bearer_prefix() {
        let h = headers_with_auth("s3cret");
        assert!(
            !bearer_auth(&h, "s3cret"),
            "raw token without Bearer prefix"
        );
    }

    #[test]
    fn provider_slug_round_trips() {
        assert_eq!(provider_slug(&Provider::YouTube), "youtube");
        assert_eq!(provider_slug(&Provider::TikTok), "tiktok");
        assert_eq!(provider_slug(&Provider::Instagram), "instagram");
    }

    #[test]
    fn redirect_uri_is_built_from_base_and_slug() {
        let config = ServerConfig {
            service_shared_secret: String::new(),
            social_firing_enabled: false,
            supabase_url: String::new(),
            supabase_service_key: String::new(),
            storage_bucket: String::new(),
            oauth_redirect_base: "https://app.example".into(),
            google_client_id: String::new(),
            google_client_secret: String::new(),
            token_key_id: String::new(),
            token_key_hex: String::new(),
            youtube_force_private: true,
            artifact_base_dir: String::new(),
            desktop_auth_token: String::new(),
            desktop_user_id: "desktop-user".into(),
        };
        assert_eq!(
            redirect_uri(&config, &Provider::YouTube),
            "https://app.example/oauth/callback/youtube"
        );
    }
}
