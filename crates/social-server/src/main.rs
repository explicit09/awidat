//! awidat-social-server — Axum HTTP wrapper over the awidat-social domain crate.
//!
//! Phase 1: skeleton with mock upload adapters, real PgSocialStore, and the
//! /internal/tick endpoint code-guarded by SOCIAL_FIRING_ENABLED=false.
//! Phase 2: server-side OAuth exchange (Google/YouTube), AEAD token storage,
//!          and the /oauth/callback/{provider} handler.
//! Phases 3/4 replace the mock adapters with real provider clients.
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
    token::Aead256Key,
    token_bundle::ProviderTokenBundle,
    upload_adapter::MockUploadAdapter,
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
use tracing::info;

// ── Server config ─────────────────────────────────────────────────────────────

#[derive(Clone)]
struct ServerConfig {
    service_shared_secret: String,
    social_firing_enabled: bool,
    supabase_url: String,
    supabase_service_key: String,
    storage_bucket: String,
    oauth_redirect_base: String,
    // Phase 2: Google OAuth credentials.
    google_client_id: String,
    google_client_secret: String,
    // Phase 2: AEAD token encryption.
    token_key_id: String,
    token_key_hex: String,
}

// ── App state ─────────────────────────────────────────────────────────────────

/// All routes share this state.
/// `spawn_blocking` moves a clone of the pool so the sync domain layer runs
/// on the blocking thread pool without holding any async lock across awaits.
struct AppState {
    pool: Pool<PostgresConnectionManager<NoTls>>,
    registry: ProviderRegistry,
    config: ServerConfig,
}

type SharedState = Arc<AppState>;

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
        },
    });

    let app = Router::new()
        .route("/health", get(health_handler))
        .route("/providers", get(providers_handler))
        .route("/artifacts/upload-url", post(artifacts_upload_url_handler))
        .route("/oauth/begin/{provider}", post(oauth_begin_handler))
        .route("/oauth/callback/{provider}", get(oauth_callback_handler))
        .route("/internal/tick", post(internal_tick_handler))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(&bind_addr)
        .await
        .unwrap_or_else(|e| panic!("bind {bind_addr}: {e}"));
    info!("listening on {bind_addr}");
    axum::serve(listener, app)
        .await
        .unwrap_or_else(|e| panic!("serve: {e}"));
}

fn env_required(key: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| panic!("required env var {key} not set"))
}

fn aead_key_from_state(
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

fn bearer_auth(headers: &HeaderMap, secret: &str) -> bool {
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

#[derive(Deserialize)]
struct OAuthCallbackQuery {
    code: String,
    state: String,
    connection_id: String,
    owner_id: String,
    owner_kind: String,
    account_id: String,
    display_name: String,
    now: i64,
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
    let now = q.now;
    let owner = parse_owner(&q.owner_kind, &q.owner_id)?;
    let account_id = q.account_id.clone();
    let display_name = q.display_name.clone();
    let connection_id = q.connection_id.clone();
    let raw_state = q.state.clone();
    let provider_account_id = token_response.provider_account_id.clone();
    let pool = state.pool.clone();

    let bundle = ProviderTokenBundle::from_oauth_response(provider.clone(), token_response, now)
        .map_err(|e| {
            (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({"error": format!("token bundle: {e:?}")})),
            )
        })?;

    let account = build_connected_account(
        account_id.clone(),
        owner.clone(),
        provider.clone(),
        provider_account_id,
        display_name,
        now,
    );

    let account = tokio::task::spawn_blocking(move || {
        let mut store = PgSocialStore::new(pool);
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
            Json(serde_json::json!({"error": e.to_string()})),
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

    // Claim due jobs and execute each through the mock adapter (Phase 1).
    // Phases 3/4 replace the mock adapter with real provider adapters.
    let pool = state.pool.clone();
    let claimed_count = tokio::task::spawn_blocking(move || {
        let mut store = PgSocialStore::new(pool);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| format!("clock error: {e}"))?
            .as_secs() as i64;
        let claimed = store
            .claim_due_publish_jobs(now, 10)
            .map_err(|e| format!("claim: {e}"))?;
        let count = claimed.len();
        for job in claimed {
            let adapter = MockUploadAdapter::published(
                job.provider.clone(),
                "mock-post-id",
                "https://example.com/mock",
            );
            if let Err(e) = SocialApi::execute_claimed_upload_job(
                &mut store,
                &adapter,
                ExecuteUploadRequest {
                    job_id: job.id,
                    title: "Mock upload".into(),
                    description: None,
                    tags: Vec::new(),
                    thumbnail_ref: None,
                    privacy: None,
                    now,
                },
            ) {
                tracing::warn!("execute job failed: {e}");
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

// ── Helpers ───────────────────────────────────────────────────────────────────

fn parse_provider(s: &str) -> Result<Provider, (StatusCode, Json<serde_json::Value>)> {
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

fn provider_client_id(
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

fn redirect_uri(config: &ServerConfig, provider: &Provider) -> String {
    let slug = match provider {
        Provider::YouTube => "youtube",
        Provider::TikTok => "tiktok",
        Provider::Instagram => "instagram",
    };
    format!("{}/oauth/callback/{slug}", config.oauth_redirect_base)
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
