//! montage-social-server — Axum HTTP wrapper over the montage-social domain crate.
//!
//! Phase 1: skeleton with mock upload adapters, real PgSocialStore, and the
//! /internal/tick endpoint code-guarded by SOCIAL_FIRING_ENABLED=false.
//! Phase 2: server-side OAuth exchange (Google/YouTube), AEAD token storage,
//!          and the /oauth/callback/{provider} handler.
//! Phase 3: real YouTube resumable-upload adapter, status client, quota gate,
//!          and production AccessTokenResolver + ArtifactSource.
//! Phase 4: poll-processing + token-refresh cron routes, server TokenRefresher,
//!          and the pg_cron schedules (migration 0004) that drive all three.
//! Phase 5: user-facing /social/* routes (desktop dev bearer).
//! Phase 7: Supabase Auth — /social/* verify a Supabase JWT (HS256) when
//!          SUPABASE_JWT_SECRET is set, else fall back to the dev bearer.
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
//!   OAUTH_REDIRECT_BASE     — base URL for OAuth redirect URIs, e.g. "https://montage-social.fly.dev"
//!   YOUTUBE_FORCE_PRIVATE   — "false" allows non-private uploads (default "true"; keep true pre-audit)
//!   ARTIFACT_BASE_DIR       — root dir for file:// artifact refs (default "/var/lib/montage-artifacts")
//!   SOCIAL_MIGRATIONS_DIR   — SQL migration dir (default: ../social/migrations from Cargo manifest)
//!   SOCIAL_DB_POOL_MAX_SIZE — max Postgres pool size (default "4")
//!   DESKTOP_AUTH_TOKEN      — (Phase 5) dev bearer for /social/* (fallback when no Supabase JWT)
//!   DESKTOP_USER_ID         — (Phase 5) fixed user id the dev bearer maps to (default "desktop-user")
//!   SUPABASE_JWT_SECRET     — (Phase 7) HS256 secret to verify Supabase Auth JWTs; server-only

mod artifact_source;
mod supabase_jwt;
mod token_refresher;
mod token_resolver;
mod user_routes;

use axum::{
    Json, Router,
    body::Body,
    extract::{Path, Query, State},
    http::{HeaderMap, Response, StatusCode, header},
    response::{Html, IntoResponse},
    routing::{get, post},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use constant_time_eq::constant_time_eq;
use hmac::{Hmac, Mac};
use montage_social::upload_adapter::{TikTokInteractionSettings, UploadPrivacy};
use montage_social::{
    account_service::{CompleteOAuthInput, SocialAccountService},
    api::{ExecuteUploadRequest, SocialApi},
    instagram_upload::{
        InstagramStatusAdapter, InstagramUploadAdapter, LiveInstagramStatusClient,
        LiveInstagramUploadClient,
    },
    model::{
        ConnectedAccount, OwnerRef, Provider, PublishJob, PublishJobEventType, PublishJobStatus,
    },
    oauth_exchange::{
        GoogleOAuthExchange, GoogleOAuthExchangeConfig, OAuthTokenExchange, PlatformOAuthExchange,
        PlatformOAuthExchangeConfig, TokenExchangeInput,
    },
    oauth_url::OAuthProviderConfig,
    pg_store::PgSocialStore,
    provider::ProviderRegistry,
    store::SocialStore,
    tiktok_upload::{
        LiveTikTokStatusClient, LiveTikTokUploadClient, TikTokStatusAdapter, TikTokUploadAdapter,
    },
    token::{Aead256Key, LocalTokenKeyProvider},
    token_bundle::ProviderTokenBundle,
    token_refresh::{TokenRefreshError, TokenRefresher},
    twitter_x_upload::{
        LiveTwitterXStatusClient, LiveTwitterXUploadClient, TwitterXStatusAdapter,
        TwitterXUploadAdapter,
    },
    youtube_upload::{
        ArtifactSource, YouTubeClientConfig, YouTubeStatusAdapter, YouTubeUploadAdapter,
        live::{LiveYouTubeStatusClient, LiveYouTubeUploadClient},
    },
};
use r2d2::Pool;
use r2d2_postgres::PostgresConnectionManager;
use r2d2_postgres::postgres::NoTls;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
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
    tiktok_client_key: String,
    tiktok_client_secret: String,
    instagram_client_id: String,
    instagram_client_secret: String,
    twitter_x_client_id: String,
    twitter_x_client_secret: String,
    // Phase 2: AEAD token encryption.
    token_key_id: String,
    token_key_hex: String,
    // Phase 3: YouTube upload config.
    // When true, forces all uploads to private regardless of job privacy setting.
    // Must be true until the YouTube TOS audit clears.
    youtube_force_private: bool,
    // TikTok unaudited/sandbox apps can only direct-post private videos.
    // Keep false until TikTok approves public posting for the app.
    tiktok_public_posting_enabled: bool,
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
    // Phase 7: Supabase Auth. When set, the /social/* routes verify the bearer
    // as a Supabase JWT (HS256) and build a per-user actor with loaded workspace
    // roles. When empty, the routes fall back to the single-user dev bearer
    // above. Server-only; never shipped to the desktop.
    pub(crate) supabase_jwt_secret: String,
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

#[derive(Clone)]
struct SocialOAuthCredentials {
    google_client_id: String,
    google_client_secret: String,
    tiktok_client_key: String,
    tiktok_client_secret: String,
    twitter_x_client_id: String,
    twitter_x_client_secret: String,
}

impl SocialOAuthCredentials {
    fn from_config(config: &ServerConfig) -> Self {
        Self {
            google_client_id: config.google_client_id.clone(),
            google_client_secret: config.google_client_secret.clone(),
            tiktok_client_key: config.tiktok_client_key.clone(),
            tiktok_client_secret: config.tiktok_client_secret.clone(),
            twitter_x_client_id: config.twitter_x_client_id.clone(),
            twitter_x_client_secret: config.twitter_x_client_secret.clone(),
        }
    }
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
    let tiktok_client_key = std::env::var("TIKTOK_CLIENT_KEY").unwrap_or_default();
    let tiktok_client_secret = std::env::var("TIKTOK_CLIENT_SECRET").unwrap_or_default();
    let instagram_client_id = std::env::var("INSTAGRAM_CLIENT_ID").unwrap_or_default();
    let instagram_client_secret = std::env::var("INSTAGRAM_CLIENT_SECRET").unwrap_or_default();
    let twitter_x_client_id = std::env::var("TWITTER_X_CLIENT_ID").unwrap_or_default();
    let twitter_x_client_secret = std::env::var("TWITTER_X_CLIENT_SECRET").unwrap_or_default();
    let token_key_hex = std::env::var("SOCIAL_TOKEN_AEAD_KEY").unwrap_or_default();
    let token_key_id = std::env::var("SOCIAL_TOKEN_KEY_ID").unwrap_or_else(|_| "k1".into());
    // Default true: force private until the YouTube TOS audit clears.
    let youtube_force_private = std::env::var("YOUTUBE_FORCE_PRIVATE")
        .map(|v| v != "false")
        .unwrap_or(true);
    let tiktok_public_posting_enabled = std::env::var("TIKTOK_PUBLIC_POSTING_ENABLED")
        .map(|v| v == "true")
        .unwrap_or(false);
    let artifact_base_dir =
        std::env::var("ARTIFACT_BASE_DIR").unwrap_or_else(|_| "/var/lib/montage-artifacts".into());
    // Phase 5 desktop dev bearer (single-user until Phase 7 Supabase Auth).
    let desktop_auth_token = std::env::var("DESKTOP_AUTH_TOKEN").unwrap_or_default();
    let desktop_user_id =
        std::env::var("DESKTOP_USER_ID").unwrap_or_else(|_| "desktop-user".into());
    let supabase_jwt_secret = std::env::var("SUPABASE_JWT_SECRET").unwrap_or_default();

    info!(
        social_firing_enabled,
        youtube_force_private,
        tiktok_public_posting_enabled,
        "montage-social-server starting — firing enabled: {social_firing_enabled}, youtube_force_private: {youtube_force_private}, tiktok_public_posting_enabled: {tiktok_public_posting_enabled}"
    );

    let manager = PostgresConnectionManager::new(
        database_url
            .parse()
            .unwrap_or_else(|e| panic!("parse DATABASE_URL: {e}")),
        NoTls,
    );

    // Build the pool + apply migrations on the BLOCKING pool, never on the async
    // main thread. The sync `postgres` client calls `block_on` in its Drop impl
    // to close the connection; doing that on a tokio runtime thread panics with
    // "Cannot start a runtime from within a runtime". `spawn_blocking` gives it a
    // plain thread where that is legal. (All request handlers already follow this
    // sync-on-blocking-pool rule; boot must too.)
    //
    // Localhost-against-shared-DB safety: the cron/extension migrations (0002,
    // 0004) are infrastructure for the *deployed* environment — 0004 (re)activates
    // the pg_cron schedules. A localhost test server must NOT touch them, or boot
    // would re-point + re-enable the deployed cron at a placeholder URL. By
    // default we skip those whenever firing is disabled; local runners can also
    // force skipping while enabling their own in-process tick loop.
    let skip_infra = std::env::var("MONTAGE_SOCIAL_SKIP_INFRA_MIGRATIONS")
        .map(|v| v == "true")
        .unwrap_or(!social_firing_enabled);
    let db_pool_max_size = db_pool_max_size();
    let migrations_dir = migrations_dir();
    let pool = tokio::task::spawn_blocking(move || {
        let pool = Pool::builder()
            .max_size(db_pool_max_size)
            .build(manager)
            .unwrap_or_else(|e| panic!("build connection pool: {e}"));
        let store = PgSocialStore::new(pool.clone());
        store
            .apply_migrations_filtered(&migrations_dir, |filename| {
                skip_infra && (filename.contains("extensions") || filename.contains("cron"))
            })
            .unwrap_or_else(|e| panic!("apply migrations: {e}"));
        pool
    })
    .await
    .unwrap_or_else(|e| panic!("boot db setup join error: {e}"));

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
            tiktok_client_key,
            tiktok_client_secret,
            instagram_client_id,
            instagram_client_secret,
            twitter_x_client_id,
            twitter_x_client_secret,
            token_key_id,
            token_key_hex,
            youtube_force_private,
            tiktok_public_posting_enabled,
            artifact_base_dir,
            desktop_auth_token,
            desktop_user_id,
            supabase_jwt_secret,
        },
    });

    let app = Router::new()
        .route("/health", get(health_handler))
        .route("/providers", get(providers_handler))
        .route("/artifacts/upload-url", post(artifacts_upload_url_handler))
        .route("/public/artifacts/{filename}", get(public_artifact_handler))
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
            "/social/targets/update",
            post(user_routes::update_target_handler),
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
            "/social/jobs/{job_id}/fire",
            post(user_routes::fire_due_job_handler),
        )
        .route(
            "/social/jobs/{job_id}/poll",
            post(user_routes::poll_processing_job_handler),
        )
        .route(
            "/social/jobs/{job_id}/reschedule",
            post(user_routes::reschedule_job_handler),
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

fn migrations_dir() -> std::path::PathBuf {
    std::env::var_os("SOCIAL_MIGRATIONS_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .unwrap_or_else(|| panic!("crates/social-server has no parent directory"))
                .join("social/migrations")
        })
}

fn db_pool_max_size() -> u32 {
    std::env::var("SOCIAL_DB_POOL_MAX_SIZE")
        .ok()
        .and_then(|raw| raw.parse::<u32>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(4)
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
    Json(serde_json::json!({"status": "ok", "service": "montage-social-server"}))
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
    // The provider returns only `code` + `state`. We embed the connection id
    // into `state` at start (`<connection_id>~<random>`) and recover it here —
    // the provider has no knowledge of our connection_id, so it cannot be a
    // separate query param.
    state: String,
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
) -> Result<Response<Body>, (StatusCode, Json<serde_json::Value>)> {
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
                    code_verifier: None,
                })
                .await
                .map_err(|e| {
                    (
                        StatusCode::BAD_GATEWAY,
                        Json(serde_json::json!({"error": e.to_string()})),
                    )
                })?
        }
        Provider::TikTok | Provider::Instagram | Provider::TwitterX => {
            let client_id = provider_client_id(&state.config, &provider)?;
            let client_secret = provider_client_secret(&state.config, &provider)?;
            let exchange = PlatformOAuthExchange::new(PlatformOAuthExchangeConfig {
                provider: provider.clone(),
                client_id,
                client_secret,
                token_endpoint: token_endpoint(&provider).into(),
                profile_endpoint: profile_endpoint(&provider).map(ToOwned::to_owned),
            });
            exchange
                .exchange(TokenExchangeInput {
                    provider: provider.clone(),
                    code: q.code.clone(),
                    redirect_uri: redirect_uri.clone(),
                    code_verifier: if provider == Provider::TwitterX {
                        Some(q.state.clone())
                    } else {
                        None
                    },
                })
                .await
                .map_err(|e| {
                    (
                        StatusCode::BAD_GATEWAY,
                        Json(serde_json::json!({"error": e.to_string()})),
                    )
                })?
        }
    };

    let display_name = output.display_name;
    let token_response = output.token_response;
    let access_token = output.access_token;
    let refresh_token = output.refresh_token;
    // SECURITY: server-authoritative timestamp; never trust a client-supplied clock.
    let now = now_secs();
    let raw_state = q.state.clone();
    // Recover the connection id embedded in `state` at oauth-start
    // (`<connection_id>~<random>`). complete_oauth re-validates the full
    // `raw_state` against the stored connection's hash, so a forged state can't
    // match — splitting here only locates which connection to validate against.
    let connection_id = raw_state
        .split_once('~')
        .map(|(id, _)| id.to_string())
        .ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "malformed oauth state"})),
            )
        })?;
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

    // Granted scopes — used to derive the account's capabilities/eligibility.
    let granted_scopes = bundle.scopes.clone();

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
        // provider response, not client input.
        let account_id = format!(
            "{}:{provider_account_id}",
            provider_slug(&provider_for_blocking)
        );
        let display_name = display_name.unwrap_or_else(|| provider_account_id.clone());
        let account = build_connected_account(
            account_id,
            owner,
            provider_for_blocking,
            provider_account_id,
            display_name,
            &granted_scopes,
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
    Ok(Html(oauth_success_html(&account)).into_response())
}

fn oauth_success_html(account: &ConnectedAccount) -> String {
    let provider = html_escape(provider_slug(&account.provider));
    let account_name = html_escape(&account.display_name);
    let account_id = html_escape(&account.id);
    format!(
        r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Montage social account connected</title>
  <style>
    :root {{ color-scheme: dark; font-family: Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; }}
    body {{ margin: 0; min-height: 100vh; display: grid; place-items: center; background: #10131f; color: #f4f6fb; }}
    main {{ width: min(560px, calc(100vw - 32px)); border: 1px solid rgba(255,255,255,.16); border-radius: 12px; padding: 28px; background: #171b2b; box-shadow: 0 20px 80px rgba(0,0,0,.35); }}
    h1 {{ margin: 0 0 12px; font-size: 24px; line-height: 1.2; }}
    p {{ margin: 8px 0; color: #b9bfce; line-height: 1.5; }}
    strong {{ color: #fff; }}
    code {{ color: #a7f3d0; word-break: break-all; }}
  </style>
</head>
<body>
  <main>
    <h1>Connected to Montage</h1>
    <p><strong>{account_name}</strong> is connected for <strong>{provider}</strong>.</p>
    <p>You can close this tab and return to Montage.</p>
    <p><code>{account_id}</code></p>
  </main>
</body>
</html>"#
    )
}

fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
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

#[derive(Deserialize)]
struct PublicArtifactQuery {
    exp: i64,
    sig: String,
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

async fn public_artifact_handler(
    State(state): State<SharedState>,
    Path(filename): Path<String>,
    Query(query): Query<PublicArtifactQuery>,
) -> Result<Response<Body>, (StatusCode, Json<serde_json::Value>)> {
    let job_id = filename
        .strip_suffix(".mp4")
        .unwrap_or(&filename)
        .to_string();
    if !verify_public_artifact_signature(
        &state.config.service_shared_secret,
        &job_id,
        query.exp,
        &query.sig,
        now_secs(),
    ) {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error": "unauthorized"})),
        ));
    }

    let pool = state.pool.clone();
    let artifact_base_dir = state.config.artifact_base_dir.clone();
    let supabase_url = state.config.supabase_url.clone();
    let supabase_service_key = state.config.supabase_service_key.clone();

    let body = tokio::task::spawn_blocking(move || {
        let store = PgSocialStore::new(pool);
        let job = store
            .publish_job(&job_id)
            .map_err(|e| format!("publish job: {e}"))?;
        let artifact_source = artifact_source::FileArtifactSource::new(artifact_base_dir)
            .with_storage_resolver(artifact_source::SupabaseStorageResolver::new(
                supabase_url,
                supabase_service_key,
                3600,
            ));
        artifact_source
            .open(&job.artifact_ref)
            .map_err(|e| format!("artifact open: {e}"))
    })
    .await
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("artifact task: {e}")})),
        )
    })?
    .map_err(|e| (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": e}))))?;

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "video/mp4")
        .header(header::CONTENT_LENGTH, body.total_bytes.to_string())
        .header(header::CACHE_CONTROL, "private, no-store")
        .body(Body::from(body.data))
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e.to_string()})),
            )
        })
}

fn sign_public_artifact(signing_secret: &str, job_id: &str, expires_at: i64) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(signing_secret.as_bytes())
        .unwrap_or_else(|_| panic!("HMAC accepts keys of any size"));
    mac.update(public_artifact_payload(job_id, expires_at).as_bytes());
    URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
}

fn verify_public_artifact_signature(
    signing_secret: &str,
    job_id: &str,
    expires_at: i64,
    signature: &str,
    now: i64,
) -> bool {
    if expires_at < now || signing_secret.is_empty() {
        return false;
    }
    let Ok(provided) = URL_SAFE_NO_PAD.decode(signature) else {
        return false;
    };
    let expected = sign_public_artifact(signing_secret, job_id, expires_at);
    let Ok(expected) = URL_SAFE_NO_PAD.decode(expected) else {
        return false;
    };
    constant_time_eq(&provided, &expected)
}

fn public_artifact_payload(job_id: &str, expires_at: i64) -> String {
    format!("{job_id}.{expires_at}")
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
    let tiktok_public_posting_enabled = state.config.tiktok_public_posting_enabled;
    let artifact_base_dir = state.config.artifact_base_dir.clone();
    let supabase_url = state.config.supabase_url.clone();
    let supabase_service_key = state.config.supabase_service_key.clone();
    let oauth_credentials = SocialOAuthCredentials::from_config(&state.config);

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
            use montage_social::model::Provider;
            match &job.provider {
                Provider::YouTube => {
                    if youtube_used >= youtube_quota_remaining {
                        tracing::warn!(job_id = %job.id, "YouTube daily quota reached, leaving job Scheduled");
                        continue;
                    }
                    let resolver = ServerAccessTokenResolver::new(pool.clone(), aead_key_clone(&aead_key), now);
                    let artifact_source = artifact_source::FileArtifactSource::new(artifact_base_dir.clone())
                        .with_storage_resolver(artifact_source::SupabaseStorageResolver::new(
                            supabase_url.clone(),
                            supabase_service_key.clone(),
                            3600,
                        ));
                    let yt_config = YouTubeClientConfig { force_private, ..Default::default() };
                    let client = LiveYouTubeUploadClient::new(resolver, artifact_source, yt_config);
                    let adapter = YouTubeUploadAdapter::new(client);
                    let Some(refresher) = token_refresher_for_provider(
                        &oauth_credentials,
                        &Provider::YouTube,
                        aead_key_clone(&aead_key),
                    ) else {
                        tracing::warn!(job_id = %job.id, "YouTube token refresh unavailable: OAuth not configured");
                        continue;
                    };
                    let upload = upload_request_for_job(&store, &job, now);
                    if let Err(e) = SocialApi::execute_claimed_upload_job_with_refresher(
                        &mut store,
                        &adapter,
                        &refresher,
                        upload,
                        TOKEN_REFRESH_SWEEP_SKEW_SECS,
                    ) {
                        tracing::warn!(job_id = %job.id, "YouTube execute failed: {e}");
                    } else {
                        youtube_used += 1;
                        let _ = store.increment_youtube_quota(now);
                    }
                }
                Provider::TikTok => {
                    let resolver = ServerAccessTokenResolver::new(
                        pool.clone(),
                        aead_key_clone(&aead_key),
                        now,
                    );
                    let eligible_for_public = tiktok_public_posting_enabled
                        && store
                        .connected_account(&job.connected_account_id)
                        .map(|account| account.capabilities.public_posting)
                        .unwrap_or(false);
                    let artifact_source = artifact_source::FileArtifactSource::new(
                        artifact_base_dir.clone(),
                    )
                    .with_storage_resolver(artifact_source::SupabaseStorageResolver::new(
                        supabase_url.clone(),
                        supabase_service_key.clone(),
                        3600,
                    ));
                    let client = LiveTikTokUploadClient::new(resolver, artifact_source);
                    let adapter =
                        TikTokUploadAdapter::with_public_eligibility(client, eligible_for_public);
                    let upload = upload_request_for_job(&store, &job, now);
                    let Some(refresher) = token_refresher_for_provider(
                        &oauth_credentials,
                        &Provider::TikTok,
                        aead_key_clone(&aead_key),
                    ) else {
                        if let Err(e) =
                            SocialApi::execute_claimed_upload_job(&mut store, &adapter, upload)
                        {
                            tracing::warn!(job_id = %job.id, provider = ?job.provider, "TikTok execute failed: {e}");
                        }
                        continue;
                    };
                    if let Err(e) = SocialApi::execute_claimed_upload_job_with_refresher(
                        &mut store,
                        &adapter,
                        &refresher,
                        upload,
                        TOKEN_REFRESH_SWEEP_SKEW_SECS,
                    ) {
                        tracing::warn!(job_id = %job.id, provider = ?job.provider, "TikTok execute failed: {e}");
                    }
                }
                Provider::Instagram => {
                    use montage_social::upload_adapter::BlockedUploadAdapter;
                    let mut upload = upload_request_for_job(&store, &job, now);
                    let instagram_account_id = match store
                        .connected_account(&job.connected_account_id)
                        .map(|account| account.provider_account_id)
                        .ok()
                        .filter(|id| !id.trim().is_empty())
                    {
                        Some(id) => id,
                        None => {
                            let blocked = BlockedUploadAdapter::new(
                                Provider::Instagram,
                                "instagram_account_id_missing",
                            );
                            if let Err(e) =
                                SocialApi::execute_claimed_upload_job(&mut store, &blocked, upload)
                            {
                                tracing::warn!(job_id = %job.id, provider = ?job.provider, "Instagram account blocker execute failed: {e}");
                            }
                            continue;
                        }
                    };
                    let artifact_source = artifact_source::FileArtifactSource::new(
                        artifact_base_dir.clone(),
                    )
                    .with_storage_resolver(artifact_source::SupabaseStorageResolver::new(
                        supabase_url.clone(),
                        supabase_service_key.clone(),
                        3600,
                    ));
                    match artifact_source.provider_fetch_url(&job.artifact_ref) {
                        Ok(url) => upload.artifact_ref = Some(url),
                        Err(e) => {
                            tracing::warn!(job_id = %job.id, "Instagram artifact URL resolution failed: {e}");
                            let blocked = BlockedUploadAdapter::new(
                                Provider::Instagram,
                                "artifact_not_provider_fetchable",
                            );
                            if let Err(e) =
                                SocialApi::execute_claimed_upload_job(&mut store, &blocked, upload)
                            {
                                tracing::warn!(job_id = %job.id, provider = ?job.provider, "Instagram artifact blocker execute failed: {e}");
                            }
                            continue;
                        }
                    }
                    let resolver = ServerAccessTokenResolver::new(
                        pool.clone(),
                        aead_key_clone(&aead_key),
                        now,
                    );
                    let client = LiveInstagramUploadClient::new(resolver, instagram_account_id);
                    let adapter = InstagramUploadAdapter::new(client);
                    if let Err(e) =
                        SocialApi::execute_claimed_upload_job(&mut store, &adapter, upload)
                    {
                        tracing::warn!(job_id = %job.id, provider = ?job.provider, "Instagram execute failed: {e}");
                    }
                }
                Provider::TwitterX => {
                    use montage_social::upload_adapter::BlockedUploadAdapter;
                    let mut upload = upload_request_for_job(&store, &job, now);
                    let artifact_source = artifact_source::FileArtifactSource::new(
                        artifact_base_dir.clone(),
                    )
                    .with_storage_resolver(artifact_source::SupabaseStorageResolver::new(
                        supabase_url.clone(),
                        supabase_service_key.clone(),
                        3600,
                    ));
                    match artifact_source.provider_fetch_url(&job.artifact_ref) {
                        Ok(url) => upload.artifact_ref = Some(url),
                        Err(e) => {
                            tracing::warn!(job_id = %job.id, "Twitter/X artifact URL resolution failed: {e}");
                            let blocked = BlockedUploadAdapter::new(
                                Provider::TwitterX,
                                "artifact_not_provider_fetchable",
                            );
                            if let Err(e) =
                                SocialApi::execute_claimed_upload_job(&mut store, &blocked, upload)
                            {
                                tracing::warn!(job_id = %job.id, provider = ?job.provider, "Twitter/X artifact blocker execute failed: {e}");
                            }
                            continue;
                        }
                    }
                    let resolver = ServerAccessTokenResolver::new(
                        pool.clone(),
                        aead_key_clone(&aead_key),
                        now,
                    );
                    let client = LiveTwitterXUploadClient::new(resolver);
                    let adapter = TwitterXUploadAdapter::new(client);
                    let Some(refresher) = token_refresher_for_provider(
                        &oauth_credentials,
                        &Provider::TwitterX,
                        aead_key_clone(&aead_key),
                    ) else {
                        if let Err(e) =
                            SocialApi::execute_claimed_upload_job(&mut store, &adapter, upload)
                        {
                            tracing::warn!(job_id = %job.id, provider = ?job.provider, "Twitter/X execute failed: {e}");
                        }
                        continue;
                    };
                    if let Err(e) = SocialApi::execute_claimed_upload_job_with_refresher(
                        &mut store,
                        &adapter,
                        &refresher,
                        upload,
                        TOKEN_REFRESH_SWEEP_SKEW_SECS,
                    ) {
                        tracing::warn!(job_id = %job.id, provider = ?job.provider, "Twitter/X execute failed: {e}");
                    }
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

fn upload_request_for_job(
    store: &impl SocialStore,
    job: &PublishJob,
    now: i64,
) -> ExecuteUploadRequest {
    let fields = scheduled_target_platform_fields(store, job).unwrap_or_default();
    let platform_title = string_field(&fields, "title").filter(|title| !title.trim().is_empty());
    let title = platform_title
        .clone()
        .unwrap_or_else(|| job.variant_id.clone());
    let mut description = string_field(&fields, "description");
    if job.provider == Provider::Instagram
        && description
            .as_deref()
            .map(str::trim)
            .is_none_or(str::is_empty)
    {
        description = platform_title.map(|title| title.trim().to_string());
    }
    ExecuteUploadRequest {
        job_id: job.id.clone(),
        title,
        description: if job.provider == Provider::TwitterX {
            None
        } else {
            description
        },
        tags: if job.provider == Provider::TwitterX {
            Vec::new()
        } else {
            string_array_field(&fields, "tags")
        },
        thumbnail_ref: if job.provider == Provider::TwitterX {
            None
        } else {
            string_field(&fields, "thumbnailRef").or_else(|| string_field(&fields, "thumbnail_ref"))
        },
        artifact_ref: None,
        privacy: if job.provider == Provider::TwitterX {
            None
        } else {
            string_field(&fields, "privacy").map(|raw| parse_upload_privacy(&raw))
        },
        tiktok_interactions: tiktok_interaction_settings(&fields),
        now,
    }
}

fn claim_direct_fire_job(
    store: &mut impl SocialStore,
    job_id: &str,
    now: i64,
) -> Result<Option<PublishJob>, String> {
    let Some(mut job) = store
        .claim_due_publish_job(job_id, now)
        .map_err(|e| format!("claim due job: {e}"))?
    else {
        return Ok(None);
    };

    if job.provider == Provider::YouTube {
        let today_count = store
            .youtube_upload_quota_today(now)
            .map_err(|e| format!("quota check: {e}"))?;
        if today_count >= YOUTUBE_DAILY_QUOTA {
            job.status = PublishJobStatus::Scheduled;
            job.attempt_count = job.attempt_count.saturating_sub(1);
            job.updated_at = now;
            store
                .save_publish_job(job)
                .map_err(|e| format!("restore quota-blocked job: {e}"))?;
            return Err("youtube daily upload quota reached".into());
        }
    }

    Ok(Some(job))
}

pub(crate) async fn fire_due_publish_job(state: SharedState, job_id: String) -> Result<(), String> {
    if !state.config.social_firing_enabled {
        return Err("social firing disabled".into());
    }

    let aead_key = aead_key_from_state(&state.config)
        .map_err(|(_, body)| body.0["error"].as_str().unwrap_or("key error").to_string())?;
    let pool = state.pool.clone();
    let force_private = state.config.youtube_force_private;
    let tiktok_public_posting_enabled = state.config.tiktok_public_posting_enabled;
    let artifact_base_dir = state.config.artifact_base_dir.clone();
    let supabase_url = state.config.supabase_url.clone();
    let supabase_service_key = state.config.supabase_service_key.clone();
    let oauth_credentials = SocialOAuthCredentials::from_config(&state.config);

    tokio::task::spawn_blocking(move || {
        let mut store = PgSocialStore::new(pool.clone());
        let now = now_secs();
        let Some(job) = claim_direct_fire_job(&mut store, &job_id, now)? else {
            return Ok(());
        };

        match &job.provider {
            Provider::YouTube => {
                let resolver =
                    ServerAccessTokenResolver::new(pool.clone(), aead_key_clone(&aead_key), now);
                let artifact_source =
                    artifact_source::FileArtifactSource::new(artifact_base_dir.clone())
                        .with_storage_resolver(artifact_source::SupabaseStorageResolver::new(
                            supabase_url.clone(),
                            supabase_service_key.clone(),
                            3600,
                        ));
                let yt_config = YouTubeClientConfig {
                    force_private,
                    ..Default::default()
                };
                let client = LiveYouTubeUploadClient::new(resolver, artifact_source, yt_config);
                let adapter = YouTubeUploadAdapter::new(client);
                let refresher = token_refresher_for_provider(
                    &oauth_credentials,
                    &Provider::YouTube,
                    aead_key_clone(&aead_key),
                )
                .ok_or_else(|| "youtube OAuth not configured".to_string())?;
                let upload = upload_request_for_job(&store, &job, now);
                SocialApi::execute_claimed_upload_job_with_refresher(
                    &mut store,
                    &adapter,
                    &refresher,
                    upload,
                    TOKEN_REFRESH_SWEEP_SKEW_SECS,
                )
                .map_err(|e| format!("youtube execute: {e}"))?;
                let _ = store.increment_youtube_quota(now);
            }
            Provider::TikTok => {
                let resolver =
                    ServerAccessTokenResolver::new(pool.clone(), aead_key_clone(&aead_key), now);
                let eligible_for_public = tiktok_public_posting_enabled
                    && store
                        .connected_account(&job.connected_account_id)
                        .map(|account| account.capabilities.public_posting)
                        .unwrap_or(false);
                let artifact_source =
                    artifact_source::FileArtifactSource::new(artifact_base_dir.clone())
                        .with_storage_resolver(artifact_source::SupabaseStorageResolver::new(
                            supabase_url.clone(),
                            supabase_service_key.clone(),
                            3600,
                        ));
                let client = LiveTikTokUploadClient::new(resolver, artifact_source);
                let adapter =
                    TikTokUploadAdapter::with_public_eligibility(client, eligible_for_public);
                let upload = upload_request_for_job(&store, &job, now);
                if let Some(refresher) = token_refresher_for_provider(
                    &oauth_credentials,
                    &Provider::TikTok,
                    aead_key_clone(&aead_key),
                ) {
                    SocialApi::execute_claimed_upload_job_with_refresher(
                        &mut store,
                        &adapter,
                        &refresher,
                        upload,
                        TOKEN_REFRESH_SWEEP_SKEW_SECS,
                    )
                    .map_err(|e| format!("tiktok execute: {e}"))?;
                } else {
                    SocialApi::execute_claimed_upload_job(&mut store, &adapter, upload)
                        .map_err(|e| format!("tiktok execute: {e}"))?;
                }
            }
            Provider::Instagram => {
                use montage_social::upload_adapter::BlockedUploadAdapter;
                let mut upload = upload_request_for_job(&store, &job, now);
                let instagram_account_id = store
                    .connected_account(&job.connected_account_id)
                    .map(|account| account.provider_account_id)
                    .ok()
                    .filter(|id| !id.trim().is_empty())
                    .ok_or_else(|| "instagram account id missing".to_string())?;
                let artifact_source =
                    artifact_source::FileArtifactSource::new(artifact_base_dir.clone())
                        .with_storage_resolver(artifact_source::SupabaseStorageResolver::new(
                            supabase_url.clone(),
                            supabase_service_key.clone(),
                            3600,
                        ));
                match artifact_source.provider_fetch_url(&job.artifact_ref) {
                    Ok(url) => upload.artifact_ref = Some(url),
                    Err(_) => {
                        let blocked = BlockedUploadAdapter::new(
                            Provider::Instagram,
                            "artifact_not_provider_fetchable",
                        );
                        SocialApi::execute_claimed_upload_job(&mut store, &blocked, upload)
                            .map_err(|e| format!("instagram artifact blocker: {e}"))?;
                        return Ok(());
                    }
                }
                let resolver =
                    ServerAccessTokenResolver::new(pool.clone(), aead_key_clone(&aead_key), now);
                let client = LiveInstagramUploadClient::new(resolver, instagram_account_id);
                let adapter = InstagramUploadAdapter::new(client);
                SocialApi::execute_claimed_upload_job(&mut store, &adapter, upload)
                    .map_err(|e| format!("instagram execute: {e}"))?;
            }
            Provider::TwitterX => {
                use montage_social::upload_adapter::BlockedUploadAdapter;
                let mut upload = upload_request_for_job(&store, &job, now);
                let artifact_source =
                    artifact_source::FileArtifactSource::new(artifact_base_dir.clone())
                        .with_storage_resolver(artifact_source::SupabaseStorageResolver::new(
                            supabase_url.clone(),
                            supabase_service_key.clone(),
                            3600,
                        ));
                match artifact_source.provider_fetch_url(&job.artifact_ref) {
                    Ok(url) => upload.artifact_ref = Some(url),
                    Err(_) => {
                        let blocked = BlockedUploadAdapter::new(
                            Provider::TwitterX,
                            "artifact_not_provider_fetchable",
                        );
                        SocialApi::execute_claimed_upload_job(&mut store, &blocked, upload)
                            .map_err(|e| format!("twitter artifact blocker: {e}"))?;
                        return Ok(());
                    }
                }
                let resolver =
                    ServerAccessTokenResolver::new(pool.clone(), aead_key_clone(&aead_key), now);
                let client = LiveTwitterXUploadClient::new(resolver);
                let adapter = TwitterXUploadAdapter::new(client);
                if let Some(refresher) = token_refresher_for_provider(
                    &oauth_credentials,
                    &Provider::TwitterX,
                    aead_key_clone(&aead_key),
                ) {
                    SocialApi::execute_claimed_upload_job_with_refresher(
                        &mut store,
                        &adapter,
                        &refresher,
                        upload,
                        TOKEN_REFRESH_SWEEP_SKEW_SECS,
                    )
                    .map_err(|e| format!("twitter execute: {e}"))?;
                } else {
                    SocialApi::execute_claimed_upload_job(&mut store, &adapter, upload)
                        .map_err(|e| format!("twitter execute: {e}"))?;
                }
            }
        }
        Ok(())
    })
    .await
    .map_err(|e| format!("join error: {e}"))?
}

pub(crate) async fn poll_processing_publish_job(
    state: SharedState,
    job_id: String,
) -> Result<(), String> {
    if !state.config.social_firing_enabled {
        return Err("social firing disabled".into());
    }

    let aead_key = aead_key_from_state(&state.config)
        .map_err(|(_, body)| body.0["error"].as_str().unwrap_or("key error").to_string())?;
    let pool = state.pool.clone();

    tokio::task::spawn_blocking(move || {
        let mut store = PgSocialStore::new(pool.clone());
        let now = now_secs();
        let job = store
            .publish_job(&job_id)
            .map_err(|e| format!("job lookup: {e}"))?;
        if job.status != montage_social::model::PublishJobStatus::Processing {
            return Ok(());
        }
        match &job.provider {
            Provider::YouTube => {
                let resolver =
                    ServerAccessTokenResolver::new(pool.clone(), aead_key_clone(&aead_key), now);
                let client = LiveYouTubeStatusClient::new(resolver);
                let adapter = YouTubeStatusAdapter::new(client);
                SocialApi::poll_upload_status(&mut store, &adapter, &job.id, now)
                    .map_err(|e| format!("youtube poll status: {e}"))?;
            }
            Provider::TikTok => {
                let resolver =
                    ServerAccessTokenResolver::new(pool.clone(), aead_key_clone(&aead_key), now);
                let client = LiveTikTokStatusClient::new(resolver);
                let adapter = TikTokStatusAdapter::new(client);
                SocialApi::poll_upload_status(&mut store, &adapter, &job.id, now)
                    .map_err(|e| format!("tiktok poll status: {e}"))?;
            }
            Provider::Instagram => {
                let account_id = store
                    .connected_account(&job.connected_account_id)
                    .map(|account| account.provider_account_id)
                    .ok()
                    .filter(|id| !id.trim().is_empty())
                    .ok_or_else(|| "instagram account id missing".to_string())?;
                let resolver =
                    ServerAccessTokenResolver::new(pool.clone(), aead_key_clone(&aead_key), now);
                let client = LiveInstagramStatusClient::new(resolver, account_id);
                let adapter = InstagramStatusAdapter::new(client);
                SocialApi::poll_upload_status(&mut store, &adapter, &job.id, now)
                    .map_err(|e| format!("instagram poll status: {e}"))?;
            }
            Provider::TwitterX => {
                let resolver =
                    ServerAccessTokenResolver::new(pool.clone(), aead_key_clone(&aead_key), now);
                let client = LiveTwitterXStatusClient::new(resolver);
                let adapter = TwitterXStatusAdapter::new(client);
                SocialApi::poll_upload_status(&mut store, &adapter, &job.id, now)
                    .map_err(|e| format!("twitter poll status: {e}"))?;
            }
        }
        Ok(())
    })
    .await
    .map_err(|e| format!("join error: {e}"))?
}

fn scheduled_target_platform_fields(
    store: &impl SocialStore,
    job: &PublishJob,
) -> Option<serde_json::Value> {
    let target_id = store
        .publish_job_events(&job.id)
        .ok()?
        .into_iter()
        .find(|event| event.event_type == PublishJobEventType::Scheduled)?
        .metadata
        .get("target_id")?
        .as_str()?
        .to_string();
    store
        .campaign_variant_target(&target_id)
        .ok()
        .map(|target| target.platform_fields)
}

fn string_field(fields: &serde_json::Value, key: &str) -> Option<String> {
    fields
        .get(key)
        .and_then(|value| value.as_str())
        .map(str::to_string)
}

fn string_array_field(fields: &serde_json::Value, key: &str) -> Vec<String> {
    fields
        .get(key)
        .and_then(|value| value.as_array())
        .map(|values| {
            values
                .iter()
                .filter_map(|value| value.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn bool_field(fields: &serde_json::Value, camel_key: &str, snake_key: &str) -> bool {
    fields
        .get(camel_key)
        .or_else(|| fields.get(snake_key))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

fn tiktok_interaction_settings(fields: &serde_json::Value) -> TikTokInteractionSettings {
    TikTokInteractionSettings {
        disable_duet: bool_field(fields, "disableDuet", "disable_duet"),
        disable_comment: bool_field(fields, "disableComment", "disable_comment"),
        disable_stitch: bool_field(fields, "disableStitch", "disable_stitch"),
    }
}

fn parse_upload_privacy(raw: &str) -> UploadPrivacy {
    match raw {
        "public" => UploadPrivacy::Public,
        "unlisted" => UploadPrivacy::Unlisted,
        _ => UploadPrivacy::Private,
    }
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
            use montage_social::model::Provider;
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
                Provider::TikTok => {
                    let resolver = ServerAccessTokenResolver::new(
                        pool.clone(),
                        aead_key_clone(&aead_key),
                        now,
                    );
                    let client = LiveTikTokStatusClient::new(resolver);
                    let adapter = TikTokStatusAdapter::new(client);
                    match SocialApi::poll_upload_status(&mut store, &adapter, &job.id, now) {
                        Ok(_) => advanced += 1,
                        Err(e) => {
                            tracing::warn!(job_id = %job.id, "TikTok poll status failed: {e}");
                        }
                    }
                }
                Provider::Instagram => {
                    let instagram_account_id = match store
                        .connected_account(&job.connected_account_id)
                        .map(|account| account.provider_account_id)
                        .ok()
                        .filter(|id| !id.trim().is_empty())
                    {
                        Some(id) => id,
                        None => {
                            tracing::warn!(job_id = %job.id, "Instagram poll skipped: account id missing");
                            continue;
                        }
                    };
                    let resolver = ServerAccessTokenResolver::new(
                        pool.clone(),
                        aead_key_clone(&aead_key),
                        now,
                    );
                    let client = LiveInstagramStatusClient::new(resolver, instagram_account_id);
                    let adapter = InstagramStatusAdapter::new(client);
                    match SocialApi::poll_upload_status(&mut store, &adapter, &job.id, now) {
                        Ok(_) => advanced += 1,
                        Err(e) => {
                            tracing::warn!(job_id = %job.id, "Instagram poll status failed: {e}");
                        }
                    }
                }
                Provider::TwitterX => {
                    let resolver = ServerAccessTokenResolver::new(
                        pool.clone(),
                        aead_key_clone(&aead_key),
                        now,
                    );
                    let client = LiveTwitterXStatusClient::new(resolver);
                    let adapter = TwitterXStatusAdapter::new(client);
                    match SocialApi::poll_upload_status(&mut store, &adapter, &job.id, now) {
                        Ok(_) => advanced += 1,
                        Err(e) => {
                            tracing::warn!(job_id = %job.id, "Twitter/X poll status failed: {e}");
                        }
                    }
                }
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
    let aead_key = aead_key_from_state(&state.config)?;
    let pool = state.pool.clone();
    let oauth_credentials = SocialOAuthCredentials::from_config(&state.config);

    let summary = tokio::task::spawn_blocking(move || {
        let mut store = PgSocialStore::new(pool);
        let now = now_secs();
        // Refresh anything expiring within the sweep skew window.
        let deadline = now.saturating_add(TOKEN_REFRESH_SWEEP_SKEW_SECS);
        let due = store
            .token_secrets_due_refresh(deadline)
            .map_err(|e| format!("due query: {e}"))?;

        let mut refreshed = 0usize;
        let mut needs_reauth = 0usize;
        let mut failed = 0usize;
        for secret in due {
            let account_id = secret.connected_account_id.clone();
            let account = match store.connected_account(&account_id) {
                Ok(account) => account,
                Err(e) => {
                    tracing::warn!(account_id, "token refresh account lookup failed: {e}");
                    failed += 1;
                    continue;
                }
            };
            let Some(refresher) = token_refresher_for_provider(
                &oauth_credentials,
                &account.provider,
                aead_key_clone(&aead_key),
            ) else {
                tracing::warn!(
                    account_id,
                    provider = ?account.provider,
                    "token refresh unavailable for provider"
                );
                let mut account = account;
                account.status = montage_social::model::ConnectedAccountStatus::NeedsReauth;
                account.updated_at = now;
                let _ = store.save_connected_account(account);
                needs_reauth += 1;
                continue;
            };
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
                        account.status = montage_social::model::ConnectedAccountStatus::NeedsReauth;
                        account.updated_at = now;
                        let _ = store.save_connected_account(account);
                    }
                    needs_reauth += 1;
                }
                Err(TokenRefreshError::Unavailable(msg)) => {
                    tracing::warn!(account_id, "token refresh unavailable: {msg}");
                    if let Ok(mut account) = store.connected_account(&account_id) {
                        account.status = montage_social::model::ConnectedAccountStatus::NeedsReauth;
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
        "twitter_x" | "twitter" | "x" => Ok(Provider::TwitterX),
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
        Provider::TikTok => {
            if config.tiktok_client_key.is_empty() {
                Err((
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(serde_json::json!({"error": "TikTok OAuth not configured"})),
                ))
            } else {
                Ok(config.tiktok_client_key.clone())
            }
        }
        Provider::Instagram => {
            if config.instagram_client_id.is_empty() {
                Err((
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(serde_json::json!({"error": "Instagram OAuth not configured"})),
                ))
            } else {
                Ok(config.instagram_client_id.clone())
            }
        }
        Provider::TwitterX => {
            if config.twitter_x_client_id.is_empty() {
                Err((
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(serde_json::json!({"error": "Twitter/X OAuth not configured"})),
                ))
            } else {
                Ok(config.twitter_x_client_id.clone())
            }
        }
    }
}

pub(crate) fn provider_client_secret(
    config: &ServerConfig,
    provider: &Provider,
) -> Result<String, (StatusCode, Json<serde_json::Value>)> {
    match provider {
        Provider::YouTube => {
            if config.google_client_secret.is_empty() {
                Err((
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(serde_json::json!({"error": "Google OAuth not configured"})),
                ))
            } else {
                Ok(config.google_client_secret.clone())
            }
        }
        Provider::TikTok => {
            if config.tiktok_client_secret.is_empty() {
                Err((
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(serde_json::json!({"error": "TikTok OAuth not configured"})),
                ))
            } else {
                Ok(config.tiktok_client_secret.clone())
            }
        }
        Provider::Instagram => {
            if config.instagram_client_secret.is_empty() {
                Err((
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(serde_json::json!({"error": "Instagram OAuth not configured"})),
                ))
            } else {
                Ok(config.instagram_client_secret.clone())
            }
        }
        Provider::TwitterX => {
            if config.twitter_x_client_secret.is_empty() {
                Err((
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(serde_json::json!({"error": "Twitter/X OAuth not configured"})),
                ))
            } else {
                Ok(config.twitter_x_client_secret.clone())
            }
        }
    }
}

fn token_endpoint(provider: &Provider) -> &'static str {
    match provider {
        Provider::YouTube => "https://oauth2.googleapis.com/token",
        Provider::TikTok => "https://open.tiktokapis.com/v2/oauth/token/",
        Provider::Instagram => "https://api.instagram.com/oauth/access_token",
        Provider::TwitterX => "https://api.x.com/2/oauth2/token",
    }
}

fn token_refresher_for_provider(
    credentials: &SocialOAuthCredentials,
    provider: &Provider,
    key: montage_social::token::Aead256Key,
) -> Option<token_refresher::ServerTokenRefresher> {
    match provider {
        Provider::YouTube => {
            if credentials.google_client_id.is_empty()
                || credentials.google_client_secret.is_empty()
            {
                None
            } else {
                Some(token_refresher::ServerTokenRefresher::new(
                    credentials.google_client_id.clone(),
                    credentials.google_client_secret.clone(),
                    key,
                ))
            }
        }
        Provider::TikTok => {
            if credentials.tiktok_client_key.is_empty()
                || credentials.tiktok_client_secret.is_empty()
            {
                None
            } else {
                Some(token_refresher::ServerTokenRefresher::new_platform(
                    Provider::TikTok,
                    credentials.tiktok_client_key.clone(),
                    credentials.tiktok_client_secret.clone(),
                    token_endpoint(provider).to_string(),
                    key,
                ))
            }
        }
        Provider::Instagram => None,
        Provider::TwitterX => {
            if credentials.twitter_x_client_id.is_empty()
                || credentials.twitter_x_client_secret.is_empty()
            {
                None
            } else {
                Some(token_refresher::ServerTokenRefresher::new_platform(
                    Provider::TwitterX,
                    credentials.twitter_x_client_id.clone(),
                    credentials.twitter_x_client_secret.clone(),
                    token_endpoint(provider).to_string(),
                    key,
                ))
            }
        }
    }
}

fn profile_endpoint(provider: &Provider) -> Option<&'static str> {
    match provider {
        Provider::YouTube => Some("https://www.googleapis.com/youtube/v3/channels"),
        Provider::TikTok => Some("https://open.tiktokapis.com/v2/user/info/"),
        Provider::Instagram => None,
        Provider::TwitterX => Some("https://api.x.com/2/users/me?user.fields=username,name"),
    }
}

#[cfg(test)]
fn pending_live_client_reason(provider: &Provider) -> Option<&'static str> {
    match provider {
        Provider::YouTube | Provider::TikTok | Provider::Instagram | Provider::TwitterX => None,
    }
}

pub(crate) fn provider_slug(provider: &Provider) -> &'static str {
    match provider {
        Provider::YouTube => "youtube",
        Provider::TikTok => "tiktok",
        Provider::Instagram => "instagram",
        Provider::TwitterX => "twitter_x",
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
    scopes: &[String],
    now: i64,
) -> ConnectedAccount {
    use montage_social::eligibility::{
        has_instagram_content_publish_scope, instagram_eligibility, tiktok_eligibility,
        twitter_x_eligibility, youtube_eligibility,
    };
    use montage_social::model::ConnectedAccountStatus;

    // Derive capabilities + eligibility from the GRANTED scopes (not defaults),
    // so an account that holds youtube.upload is actually marked upload-capable.
    // Without this the account connects but shows upload_video=false and can't
    // publish. complete_oauth also persists account.scopes from the token bundle.
    let scope_refs: Vec<&str> = scopes.iter().map(String::as_str).collect();
    let report = match provider {
        Provider::YouTube => {
            youtube_eligibility(&provider_account_id, &display_name, None, &scope_refs)
        }
        Provider::TikTok => tiktok_eligibility(&provider_account_id, &display_name, &scope_refs),
        Provider::Instagram => instagram_eligibility(
            &provider_account_id,
            &display_name,
            true,
            has_instagram_content_publish_scope(&scope_refs),
        ),
        Provider::TwitterX => {
            twitter_x_eligibility(&provider_account_id, &display_name, None, &scope_refs)
        }
    };

    ConnectedAccount {
        id,
        owner,
        provider,
        provider_account_id,
        display_name,
        handle: report.profile.handle,
        avatar_url: None,
        account_kind: report.profile.account_kind,
        status: ConnectedAccountStatus::Connected,
        scopes: scopes.to_vec(),
        capabilities: report.capabilities,
        eligibility: report.eligibility,
        last_verified_at: Some(now),
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
    fn public_artifact_signature_rejects_tampering() {
        let sig = sign_public_artifact("server-secret", "job_123", 1_000);

        assert!(!verify_public_artifact_signature(
            "server-secret",
            "job_999",
            1_000,
            &sig,
            999
        ));
        assert!(!verify_public_artifact_signature(
            "wrong-secret",
            "job_123",
            1_000,
            &sig,
            999
        ));
    }

    #[test]
    fn provider_slug_round_trips() {
        assert_eq!(provider_slug(&Provider::YouTube), "youtube");
        assert_eq!(provider_slug(&Provider::TikTok), "tiktok");
        assert_eq!(provider_slug(&Provider::Instagram), "instagram");
        assert_eq!(provider_slug(&Provider::TwitterX), "twitter_x");
        assert_eq!(parse_provider("x").unwrap(), Provider::TwitterX);
        assert_eq!(parse_provider("twitter").unwrap(), Provider::TwitterX);
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
            tiktok_client_key: String::new(),
            tiktok_client_secret: String::new(),
            instagram_client_id: String::new(),
            instagram_client_secret: String::new(),
            twitter_x_client_id: String::new(),
            twitter_x_client_secret: String::new(),
            token_key_id: String::new(),
            token_key_hex: String::new(),
            youtube_force_private: true,
            tiktok_public_posting_enabled: false,
            artifact_base_dir: String::new(),
            desktop_auth_token: String::new(),
            desktop_user_id: "desktop-user".into(),
            supabase_jwt_secret: String::new(),
        };
        assert_eq!(
            redirect_uri(&config, &Provider::YouTube),
            "https://app.example/oauth/callback/youtube"
        );
    }

    #[test]
    fn provider_client_id_reads_config_for_each_major_platform() {
        let config = ServerConfig {
            service_shared_secret: String::new(),
            social_firing_enabled: false,
            supabase_url: String::new(),
            supabase_service_key: String::new(),
            storage_bucket: String::new(),
            oauth_redirect_base: "https://app.example".into(),
            google_client_id: "google-client".into(),
            google_client_secret: String::new(),
            tiktok_client_key: "tiktok-key".into(),
            tiktok_client_secret: String::new(),
            instagram_client_id: "instagram-client".into(),
            instagram_client_secret: String::new(),
            twitter_x_client_id: "twitter-client".into(),
            twitter_x_client_secret: String::new(),
            token_key_id: String::new(),
            token_key_hex: String::new(),
            youtube_force_private: true,
            tiktok_public_posting_enabled: false,
            artifact_base_dir: String::new(),
            desktop_auth_token: String::new(),
            desktop_user_id: "desktop-user".into(),
            supabase_jwt_secret: String::new(),
        };

        assert_eq!(
            provider_client_id(&config, &Provider::YouTube).unwrap(),
            "google-client"
        );
        assert_eq!(
            provider_client_id(&config, &Provider::TikTok).unwrap(),
            "tiktok-key"
        );
        assert_eq!(
            provider_client_id(&config, &Provider::Instagram).unwrap(),
            "instagram-client"
        );
        assert_eq!(
            provider_client_id(&config, &Provider::TwitterX).unwrap(),
            "twitter-client"
        );
    }

    #[test]
    fn instagram_oauth_uses_instagram_login_endpoints() {
        assert_eq!(
            token_endpoint(&Provider::Instagram),
            "https://api.instagram.com/oauth/access_token"
        );
        assert_eq!(profile_endpoint(&Provider::Instagram), None);
    }

    #[test]
    fn connected_account_builder_uses_provider_specific_eligibility() {
        let account = build_connected_account(
            "twitter_x:x_user_1".into(),
            OwnerRef::User("user_1".into()),
            Provider::TwitterX,
            "x_user_1".into(),
            "Creator".into(),
            &[
                "users.read".into(),
                "tweet.write".into(),
                "media.write".into(),
            ],
            100,
        );

        assert_eq!(account.provider, Provider::TwitterX);
        assert!(account.capabilities.upload_video);
        assert!(account.eligibility.eligible);
    }

    #[test]
    fn connected_instagram_account_accepts_business_publish_scope() {
        let account = build_connected_account(
            "instagram:ig_user_1".into(),
            OwnerRef::User("user_1".into()),
            Provider::Instagram,
            "ig_user_1".into(),
            "Creator".into(),
            &[
                "instagram_business_basic".into(),
                "instagram_business_content_publish".into(),
            ],
            100,
        );

        assert_eq!(account.provider, Provider::Instagram);
        assert!(account.capabilities.upload_video);
        assert!(account.capabilities.public_posting);
        assert!(account.eligibility.eligible);
    }

    #[test]
    fn live_clients_are_not_routed_to_pending_live_client_blocker() {
        assert_eq!(pending_live_client_reason(&Provider::TikTok), None);
        assert_eq!(pending_live_client_reason(&Provider::Instagram), None);
        assert_eq!(pending_live_client_reason(&Provider::TwitterX), None);
    }

    #[test]
    fn direct_fire_skips_quota_when_job_is_not_due() {
        use montage_social::{
            model::PublishJob,
            store::{InMemorySocialStore, SocialStore},
        };

        let now = 1_000;
        let mut store = InMemorySocialStore::default();
        for _ in 0..YOUTUBE_DAILY_QUOTA {
            store.increment_youtube_quota(now).unwrap();
        }
        let job = PublishJob::new(
            "job_not_due",
            "campaign_1",
            "variant_1",
            "acct_1",
            Provider::YouTube,
            "file:///tmp/render.mp4",
            now + 60,
            "desktop",
        )
        .schedule(now);
        store.save_publish_job(job).unwrap();

        let claimed = claim_direct_fire_job(&mut store, "job_not_due", now).unwrap();

        assert!(claimed.is_none());
        let persisted = store.publish_job("job_not_due").unwrap();
        assert_eq!(persisted.status, PublishJobStatus::Scheduled);
        assert_eq!(persisted.attempt_count, 0);
    }

    #[test]
    fn direct_fire_restores_due_youtube_job_when_quota_is_exhausted() {
        use montage_social::{
            model::PublishJob,
            store::{InMemorySocialStore, SocialStore},
        };

        let now = 1_000;
        let mut store = InMemorySocialStore::default();
        for _ in 0..YOUTUBE_DAILY_QUOTA {
            store.increment_youtube_quota(now).unwrap();
        }
        let job = PublishJob::new(
            "job_due",
            "campaign_1",
            "variant_1",
            "acct_1",
            Provider::YouTube,
            "file:///tmp/render.mp4",
            now,
            "desktop",
        )
        .schedule(now);
        store.save_publish_job(job).unwrap();

        let err = claim_direct_fire_job(&mut store, "job_due", now).unwrap_err();

        assert_eq!(err, "youtube daily upload quota reached");
        let persisted = store.publish_job("job_due").unwrap();
        assert_eq!(persisted.status, PublishJobStatus::Scheduled);
        assert_eq!(persisted.attempt_count, 0);
    }

    #[test]
    fn upload_request_for_job_uses_target_platform_fields() {
        use montage_social::{
            model::{
                CampaignVariantTarget, PublishJob, PublishJobActorType, PublishJobEvent,
                PublishJobEventType,
            },
            store::{InMemorySocialStore, SocialStore},
        };

        let mut store = InMemorySocialStore::default();
        let target = CampaignVariantTarget::new(
            "target_1",
            "campaign_1",
            "variant_1",
            "acct_1",
            Provider::YouTube,
            serde_json::json!({
                "title": "Launch clip",
                "description": "Rendered from Montage",
                "tags": ["montage", "launch"],
                "privacy": "unlisted"
            }),
            2_000,
            1_000,
        );
        store.save_campaign_variant_target(target).unwrap();

        let job = PublishJob::new(
            "job_1",
            "campaign_1",
            "variant_1",
            "acct_1",
            Provider::YouTube,
            "file:///tmp/render.mp4",
            2_000,
            "desktop",
        )
        .schedule(1_000);
        store.save_publish_job(job.clone()).unwrap();
        store
            .append_publish_job_event(PublishJobEvent::new(
                "event_job_1_scheduled",
                "job_1",
                PublishJobEventType::Scheduled,
                PublishJobActorType::User,
                "publish job scheduled",
                serde_json::json!({"target_id": "target_1"}),
                1_000,
            ))
            .unwrap();

        let request = upload_request_for_job(&store, &job, 1_001);
        assert_eq!(request.title, "Launch clip");
        assert_eq!(
            request.description.as_deref(),
            Some("Rendered from Montage")
        );
        assert_eq!(request.tags, vec!["montage", "launch"]);
        assert_eq!(request.privacy, Some(UploadPrivacy::Unlisted));
    }

    #[test]
    fn upload_request_for_job_drops_stale_twitter_x_only_fields() {
        use montage_social::{
            model::{
                CampaignVariantTarget, PublishJob, PublishJobActorType, PublishJobEvent,
                PublishJobEventType,
            },
            store::{InMemorySocialStore, SocialStore},
        };

        let mut store = InMemorySocialStore::default();
        let target = CampaignVariantTarget::new(
            "target_1",
            "campaign_1",
            "variant_1",
            "acct_1",
            Provider::TwitterX,
            serde_json::json!({
                "title": "Launch post text",
                "description": "stale generic description",
                "tags": ["stale", "ignored"],
                "thumbnailRef": "render://thumb_1",
                "privacy": "private"
            }),
            2_000,
            1_000,
        );
        store.save_campaign_variant_target(target).unwrap();

        let job = PublishJob::new(
            "job_1",
            "campaign_1",
            "variant_1",
            "acct_1",
            Provider::TwitterX,
            "file:///tmp/render.mp4",
            2_000,
            "desktop",
        )
        .schedule(1_000);
        store.save_publish_job(job.clone()).unwrap();
        store
            .append_publish_job_event(PublishJobEvent::new(
                "event_job_1_scheduled",
                "job_1",
                PublishJobEventType::Scheduled,
                PublishJobActorType::User,
                "publish job scheduled",
                serde_json::json!({"target_id": "target_1"}),
                1_000,
            ))
            .unwrap();

        let request = upload_request_for_job(&store, &job, 1_001);
        assert_eq!(request.title, "Launch post text");
        assert_eq!(request.description, None);
        assert!(request.tags.is_empty());
        assert_eq!(request.thumbnail_ref, None);
        assert_eq!(request.privacy, None);
    }

    #[test]
    fn upload_request_for_job_maps_instagram_title_to_caption_when_description_is_blank() {
        use montage_social::{
            model::{
                CampaignVariantTarget, PublishJob, PublishJobActorType, PublishJobEvent,
                PublishJobEventType,
            },
            store::{InMemorySocialStore, SocialStore},
        };

        let mut store = InMemorySocialStore::default();
        let target = CampaignVariantTarget::new(
            "target_1",
            "campaign_1",
            "variant_1",
            "acct_1",
            Provider::Instagram,
            serde_json::json!({
                "title": "Shared scheduler title",
                "description": "",
                "privacy": "private"
            }),
            2_000,
            1_000,
        );
        store.save_campaign_variant_target(target).unwrap();

        let job = PublishJob::new(
            "job_1",
            "campaign_1",
            "variant_1",
            "acct_1",
            Provider::Instagram,
            "file:///tmp/render.mp4",
            2_000,
            "desktop",
        )
        .schedule(1_000);
        store.save_publish_job(job.clone()).unwrap();
        store
            .append_publish_job_event(PublishJobEvent::new(
                "event_job_1_scheduled",
                "job_1",
                PublishJobEventType::Scheduled,
                PublishJobActorType::User,
                "publish job scheduled",
                serde_json::json!({"target_id": "target_1"}),
                1_000,
            ))
            .unwrap();

        let request = upload_request_for_job(&store, &job, 1_001);
        assert_eq!(request.title, "Shared scheduler title");
        assert_eq!(
            request.description.as_deref(),
            Some("Shared scheduler title")
        );
    }

    #[test]
    fn upload_request_for_job_uses_tiktok_interaction_fields() {
        use montage_social::{
            model::{
                CampaignVariantTarget, PublishJob, PublishJobActorType, PublishJobEvent,
                PublishJobEventType,
            },
            store::{InMemorySocialStore, SocialStore},
        };

        let mut store = InMemorySocialStore::default();
        let target = CampaignVariantTarget::new(
            "target_1",
            "campaign_1",
            "variant_1",
            "acct_1",
            Provider::TikTok,
            serde_json::json!({
                "title": "Launch clip",
                "privacy": "private",
                "disableDuet": true,
                "disableComment": true,
                "disableStitch": true
            }),
            2_000,
            1_000,
        );
        store.save_campaign_variant_target(target).unwrap();

        let job = PublishJob::new(
            "job_1",
            "campaign_1",
            "variant_1",
            "acct_1",
            Provider::TikTok,
            "file:///tmp/render.mp4",
            2_000,
            "desktop",
        )
        .schedule(1_000);
        store.save_publish_job(job.clone()).unwrap();
        store
            .append_publish_job_event(PublishJobEvent::new(
                "event_job_1_scheduled",
                "job_1",
                PublishJobEventType::Scheduled,
                PublishJobActorType::User,
                "publish job scheduled",
                serde_json::json!({"target_id": "target_1"}),
                1_000,
            ))
            .unwrap();

        let request = upload_request_for_job(&store, &job, 1_001);
        assert!(request.tiktok_interactions.disable_duet);
        assert!(request.tiktok_interactions.disable_comment);
        assert!(request.tiktok_interactions.disable_stitch);
    }
}
