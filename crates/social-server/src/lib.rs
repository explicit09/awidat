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
//!   TIKTOK_CLIENT_KEY       — TikTok OAuth client key (server-only)
//!   TIKTOK_CLIENT_SECRET    — TikTok OAuth client secret (server-only)
//!   INSTAGRAM_CLIENT_ID     — Instagram OAuth app client ID (server-only)
//!   INSTAGRAM_CLIENT_SECRET — Instagram OAuth app secret (server-only)
//!   TWITTER_X_CLIENT_ID     — Twitter/X OAuth client ID (server-only)
//!   TWITTER_X_CLIENT_SECRET — Twitter/X OAuth client secret (server-only)
//!   SOCIAL_TOKEN_AEAD_KEY   — 64 hex chars = 32-byte ChaCha20-Poly1305 key (Phase 2)
//!   SOCIAL_TOKEN_KEY_ID     — key identifier stored alongside every token (Phase 2)
//!   OAUTH_REDIRECT_BASE     — base URL for OAuth redirect URIs, e.g. `https://montage-social.fly.dev`
//!   YOUTUBE_FORCE_PRIVATE   — "false" allows non-private uploads (default "true"; keep true pre-audit)
//!   ARTIFACT_BASE_DIR       — root dir for file:// artifact refs (default "/var/lib/montage-artifacts")
//!   SOCIAL_MIGRATIONS_DIR   — SQL migration dir (default: ../social/migrations from Cargo manifest)
//!   SOCIAL_DB_POOL_MAX_SIZE — max Postgres pool size (default "4")
//!   DESKTOP_AUTH_TOKEN      — (Phase 5) dev bearer for /social/* (fallback when no Supabase JWT)
//!   DESKTOP_USER_ID         — (Phase 5) fixed user id the dev bearer maps to (default "desktop-user")
//!   SUPABASE_JWT_SECRET     — (Phase 7) HS256 secret to verify Supabase Auth JWTs; server-only
//!   SOCIAL_ALLOWED_USER_IDS — optional comma-separated Supabase user ids allowed to use /social/*
//!
//! Provider endpoint overrides (all optional; default to the production hosts —
//! set only in hermetic tests to point live clients at a mock server):
//!   SOCIAL_YOUTUBE_UPLOAD_BASE    — YouTube resumable-upload endpoint
//!   SOCIAL_YOUTUBE_VIDEOS_BASE    — YouTube Data API videos (status) endpoint
//!   SOCIAL_TIKTOK_API_BASE        — TikTok API base
//!   SOCIAL_INSTAGRAM_GRAPH_BASE   — Instagram Graph API base
//!   SOCIAL_TWITTER_X_API_BASE     — Twitter/X API base
//!   SOCIAL_GOOGLE_TOKEN_ENDPOINT  — Google OAuth token endpoint

mod artifact_source;
pub mod store_handle;
mod supabase_jwt;
mod token_refresher;
mod token_resolver;
mod user_routes;

pub use store_handle::{PgPool, ServerStore, StoreHandle};

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
use montage_social::upload_adapter::{TikTokInteractionSettings, UploadAdapter, UploadPrivacy};
use montage_social::{
    account_service::{CompleteOAuthInput, SocialAccountService},
    api::{ExecuteUploadRequest, SocialApi},
    instagram_upload::{
        INSTAGRAM_GRAPH_BASE, InstagramStatusAdapter, InstagramUploadAdapter,
        LiveInstagramStatusClient, LiveInstagramUploadClient,
    },
    model::{
        ConnectedAccount, OwnerRef, Provider, PublishJob, PublishJobActorType, PublishJobEvent,
        PublishJobEventType, PublishJobStatus,
    },
    oauth_exchange::{
        GOOGLE_TOKEN_ENDPOINT, GoogleOAuthExchange, GoogleOAuthExchangeConfig, OAuthTokenExchange,
        PlatformOAuthExchange, PlatformOAuthExchangeConfig, TokenExchangeInput,
    },
    oauth_url::OAuthProviderConfig,
    pg_store::PgSocialStore,
    provider::ProviderRegistry,
    store::SocialStore,
    tiktok_upload::{
        LiveTikTokStatusClient, LiveTikTokUploadClient, TIKTOK_API_BASE, TikTokStatusAdapter,
        TikTokUploadAdapter,
    },
    token::{Aead256Key, LocalTokenKeyProvider},
    token_bundle::ProviderTokenBundle,
    token_refresh::{TokenRefreshError, TokenRefresher},
    twitter_x_upload::{
        LiveTwitterXStatusClient, LiveTwitterXUploadClient, TWITTER_X_API_BASE,
        TwitterXStatusAdapter, TwitterXUploadAdapter,
    },
    youtube_upload::{
        ArtifactSource, YOUTUBE_UPLOAD_BASE, YOUTUBE_VIDEOS_BASE, YouTubeClientConfig,
        YouTubeStatusAdapter, YouTubeUploadAdapter,
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
pub struct ServerConfig {
    pub service_shared_secret: String,
    pub social_firing_enabled: bool,
    pub supabase_url: String,
    pub supabase_service_key: String,
    pub storage_bucket: String,
    pub oauth_redirect_base: String,
    // Phase 2: Google OAuth credentials.
    pub google_client_id: String,
    pub google_client_secret: String,
    pub tiktok_client_key: String,
    pub tiktok_client_secret: String,
    pub instagram_client_id: String,
    pub instagram_client_secret: String,
    pub twitter_x_client_id: String,
    pub twitter_x_client_secret: String,
    // Phase 2: AEAD token encryption.
    pub token_key_id: String,
    pub token_key_hex: String,
    // Phase 3: YouTube upload config.
    // When true, forces all uploads to private regardless of job privacy setting.
    // Must be true until the YouTube TOS audit clears.
    pub youtube_force_private: bool,
    // TikTok unaudited/sandbox apps can only direct-post private videos.
    // Keep false until TikTok approves public posting for the app.
    pub tiktok_public_posting_enabled: bool,
    // Phase 3: artifact root. `file://` artifact refs are confined to this
    // directory (path-traversal defense). Phase 5 replaces local files with
    // Supabase Storage signed URLs.
    pub artifact_base_dir: String,
    // Phase 5: desktop client auth (pre-Phase-7 single-user dev bearer).
    // The desktop sends `Authorization: Bearer <desktop_auth_token>` to the
    // user-facing `/social/*` routes; it maps to the fixed `desktop_user_id`.
    // Phase 7 replaces this with real Supabase Auth.
    pub desktop_auth_token: String,
    pub desktop_user_id: String,
    // Phase 7: Supabase Auth. When set, the /social/* routes verify the bearer
    // as a Supabase JWT (HS256) and build a per-user actor with loaded workspace
    // roles. When empty, the routes fall back to the single-user dev bearer
    // above. Server-only; never shipped to the desktop.
    pub supabase_jwt_secret: String,
    // Optional product-level gate for limited-access social publishing.
    // Empty means auth + workspace roles decide access. When populated, /social/*
    // rejects authenticated users whose id is not listed here.
    pub social_allowed_user_ids: Vec<String>,
    // Provider endpoint bases. Default to the production hosts; env-overridable
    // (SOCIAL_*_BASE / SOCIAL_GOOGLE_TOKEN_ENDPOINT) so hermetic tests can point
    // the live clients at a mock server. Unset env == exact pre-seam behavior.
    pub youtube_upload_base: String,
    pub youtube_videos_base: String,
    pub tiktok_api_base: String,
    pub instagram_graph_base: String,
    pub twitter_x_api_base: String,
    pub google_token_endpoint: String,
}

impl Default for ServerConfig {
    /// Empty credentials/secrets, production provider endpoints, and the same
    /// fallback values `run()` uses when the corresponding env vars are unset.
    fn default() -> Self {
        Self {
            service_shared_secret: String::new(),
            social_firing_enabled: false,
            supabase_url: String::new(),
            supabase_service_key: String::new(),
            storage_bucket: "artifacts".into(),
            oauth_redirect_base: String::new(),
            google_client_id: String::new(),
            google_client_secret: String::new(),
            tiktok_client_key: String::new(),
            tiktok_client_secret: String::new(),
            instagram_client_id: String::new(),
            instagram_client_secret: String::new(),
            twitter_x_client_id: String::new(),
            twitter_x_client_secret: String::new(),
            token_key_id: "k1".into(),
            token_key_hex: String::new(),
            youtube_force_private: true,
            tiktok_public_posting_enabled: false,
            artifact_base_dir: "/var/lib/montage-artifacts".into(),
            desktop_auth_token: String::new(),
            desktop_user_id: "desktop-user".into(),
            supabase_jwt_secret: String::new(),
            social_allowed_user_ids: Vec::new(),
            youtube_upload_base: YOUTUBE_UPLOAD_BASE.into(),
            youtube_videos_base: YOUTUBE_VIDEOS_BASE.into(),
            tiktok_api_base: TIKTOK_API_BASE.into(),
            instagram_graph_base: INSTAGRAM_GRAPH_BASE.into(),
            twitter_x_api_base: TWITTER_X_API_BASE.into(),
            google_token_endpoint: GOOGLE_TOKEN_ENDPOINT.into(),
        }
    }
}

// ── App state ─────────────────────────────────────────────────────────────────

/// All routes share this state.
/// `spawn_blocking` moves a clone of the store handle so the sync domain layer
/// runs on the blocking thread pool without holding any async lock across awaits.
pub struct AppState {
    pub store: StoreHandle,
    pub registry: ProviderRegistry,
    pub config: ServerConfig,
}

#[derive(Clone)]
struct SocialOAuthCredentials {
    google_client_id: String,
    google_client_secret: String,
    tiktok_client_key: String,
    tiktok_client_secret: String,
    twitter_x_client_id: String,
    twitter_x_client_secret: String,
    // Token endpoints resolved from config so refreshers built inside blocking
    // closures never consult the environment.
    google_token_endpoint: String,
    tiktok_token_endpoint: String,
    twitter_x_token_endpoint: String,
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
            google_token_endpoint: config.google_token_endpoint.clone(),
            tiktok_token_endpoint: tiktok_token_endpoint(&config.tiktok_api_base),
            twitter_x_token_endpoint: twitter_x_token_endpoint(&config.twitter_x_api_base),
        }
    }
}

pub type SharedState = Arc<AppState>;

// ── Entry point ───────────────────────────────────────────────────────────────

/// Boot the production server: read env config, build the Postgres pool, apply
/// migrations, and serve the router. This is the whole body of `fn main()`.
pub async fn run() {
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
    let social_allowed_user_ids = social_allowed_user_ids();
    // Provider endpoint overrides — default to the production hosts.
    let youtube_upload_base = env_or("SOCIAL_YOUTUBE_UPLOAD_BASE", YOUTUBE_UPLOAD_BASE);
    let youtube_videos_base = env_or("SOCIAL_YOUTUBE_VIDEOS_BASE", YOUTUBE_VIDEOS_BASE);
    let tiktok_api_base = env_or("SOCIAL_TIKTOK_API_BASE", TIKTOK_API_BASE);
    let instagram_graph_base = env_or("SOCIAL_INSTAGRAM_GRAPH_BASE", INSTAGRAM_GRAPH_BASE);
    let twitter_x_api_base = env_or("SOCIAL_TWITTER_X_API_BASE", TWITTER_X_API_BASE);
    let google_token_endpoint = env_or("SOCIAL_GOOGLE_TOKEN_ENDPOINT", GOOGLE_TOKEN_ENDPOINT);

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
        store: StoreHandle::Pg(pool),
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
            social_allowed_user_ids,
            youtube_upload_base,
            youtube_videos_base,
            tiktok_api_base,
            instagram_graph_base,
            twitter_x_api_base,
            google_token_endpoint,
        },
    });

    let app = build_router(state);

    let listener = tokio::net::TcpListener::bind(&bind_addr)
        .await
        .unwrap_or_else(|e| panic!("bind {bind_addr}: {e}"));
    info!("listening on {bind_addr}");
    axum::serve(listener, app)
        .await
        .unwrap_or_else(|e| panic!("serve: {e}"));
}

/// Build the full production router over the given state. Public so hermetic
/// route-level tests can drive the exact same routing table.
pub fn build_router(state: SharedState) -> Router {
    Router::new()
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
        .with_state(state)
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

fn social_allowed_user_ids() -> Vec<String> {
    parse_social_allowed_user_ids(&std::env::var("SOCIAL_ALLOWED_USER_IDS").unwrap_or_default())
}

fn parse_social_allowed_user_ids(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect()
}

/// Maximum YouTube Data API uploads per day per project (hard Google quota).
const YOUTUBE_DAILY_QUOTA: usize = 100;

/// The refresh sweep refreshes any token expiring within this window (15 min),
/// chosen larger than the cron interval so no due upload finds a dead token.
const TOKEN_REFRESH_SWEEP_SKEW_SECS: i64 = 900;

fn env_required(key: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| panic!("required env var {key} not set"))
}

/// Read an env var, falling back to `default` when unset or empty.
fn env_or(key: &str, default: &str) -> String {
    std::env::var(key)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| default.to_string())
}

/// TikTok OAuth token endpoint derived from the (overridable) API base.
/// With the default base this is byte-identical to the pre-seam literal.
fn tiktok_token_endpoint(api_base: &str) -> String {
    format!("{}/v2/oauth/token/", api_base.trim_end_matches('/'))
}

/// Twitter/X OAuth token endpoint derived from the (overridable) API base.
fn twitter_x_token_endpoint(api_base: &str) -> String {
    format!("{}/2/oauth2/token", api_base.trim_end_matches('/'))
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
    // Fail closed on an empty secret: constant_time_eq("", "") is true,
    // so a SERVICE_SHARED_SECRET set to "" would otherwise turn every
    // /internal/* route into an unauthenticated endpoint.
    if secret.is_empty() {
        return false;
    }
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

    let store_handle = state.store.clone();
    let connection_id = body.connection_id.clone();
    let state_str = body.state.clone();
    let return_to = body.return_to.clone();
    let created_at = body.created_at;
    let expires_at = body.expires_at;

    let result = tokio::task::spawn_blocking(move || {
        let mut store = store_handle.open();
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
///
/// SECURITY (R26): `state` is validated — shape, then hash + status/expiry
/// against the stored `OAuthConnection` — BEFORE any provider round-trip. A
/// forged/replayed/expired `state` must never cause an authorization `code`
/// to be spent against the provider's token endpoint; validating state first
/// means a rejected callback makes zero provider requests.
async fn oauth_callback_handler(
    State(state): State<SharedState>,
    Path(provider_str): Path<String>,
    Query(q): Query<OAuthCallbackQuery>,
) -> Result<Response<Body>, (StatusCode, Json<serde_json::Value>)> {
    let provider = parse_provider(&provider_str)?;
    let key = aead_key_from_state(&state.config)?;
    let redirect_uri = redirect_uri(&state.config, &provider);

    // SECURITY: server-authoritative timestamp; never trust a client-supplied clock.
    let now = now_secs();
    let raw_state = q.state.clone();
    // Recover the connection id embedded in `state` at oauth-start
    // (`<connection_id>~<random>`). `validate_callback` below re-validates the
    // full `raw_state` against the stored connection's hash, so a forged state
    // can't match — splitting here only locates which connection to check.
    let connection_id = raw_state
        .split_once('~')
        .map(|(id, _)| id.to_string())
        .ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "malformed oauth state"})),
            )
        })?;

    // Validate `state` against the started connection BEFORE touching the
    // provider — an invalid/expired/forged/replayed state must reject here
    // with zero provider traffic (no code exchange spent).
    let store_handle_for_validate = state.store.clone();
    let connection_id_for_validate = connection_id.clone();
    let raw_state_for_validate = raw_state.clone();
    tokio::task::spawn_blocking(move || {
        let store = store_handle_for_validate.open();
        let connection = store
            .oauth_connection(&connection_id_for_validate)
            .map_err(|e| format!("connection lookup: {e}"))?;
        connection
            .validate_callback(&raw_state_for_validate, now)
            .map_err(|e| format!("{e:?}"))
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
                token_endpoint: state.config.google_token_endpoint.clone(),
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
                token_endpoint: token_endpoint(&state.config, &provider),
                profile_endpoint: profile_endpoint(&state.config, &provider),
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
    // `now`, `raw_state`, and `connection_id` were already derived above (state
    // validated before the provider exchange); reused here for the same
    // `complete_oauth` re-validation contract.
    let provider_account_id = token_response.provider_account_id.clone();
    let store_handle = state.store.clone();
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
        let mut store = store_handle.open();

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

    let store_handle = state.store.clone();
    let artifact_base_dir = state.config.artifact_base_dir.clone();
    let supabase_url = state.config.supabase_url.clone();
    let supabase_service_key = state.config.supabase_service_key.clone();

    let body = tokio::task::spawn_blocking(move || {
        let store = store_handle.open();
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
    let store_handle = state.store.clone();
    let force_private = state.config.youtube_force_private;
    let tiktok_public_posting_enabled = state.config.tiktok_public_posting_enabled;
    let artifact_base_dir = state.config.artifact_base_dir.clone();
    let supabase_url = state.config.supabase_url.clone();
    let supabase_service_key = state.config.supabase_service_key.clone();
    let oauth_credentials = SocialOAuthCredentials::from_config(&state.config);
    let youtube_upload_base = state.config.youtube_upload_base.clone();
    let tiktok_api_base = state.config.tiktok_api_base.clone();
    let instagram_graph_base = state.config.instagram_graph_base.clone();
    let twitter_x_api_base = state.config.twitter_x_api_base.clone();

    let claimed_count = tokio::task::spawn_blocking(move || {
        let mut store = store_handle.open();
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
                        if let Err(e) = restore_youtube_quota_blocked_job(&mut store, job, now) {
                            tracing::warn!("failed to restore quota-blocked YouTube job: {e}");
                        }
                        continue;
                    }
                    let resolver = ServerAccessTokenResolver::new(store_handle.clone(), aead_key_clone(&aead_key), now);
                    let artifact_source = artifact_source::FileArtifactSource::new(artifact_base_dir.clone())
                        .with_storage_resolver(artifact_source::SupabaseStorageResolver::new(
                            supabase_url.clone(),
                            supabase_service_key.clone(),
                            3600,
                        ));
                    let yt_config = YouTubeClientConfig {
                        force_private,
                        upload_base: youtube_upload_base.clone(),
                        ..Default::default()
                    };
                    let client = LiveYouTubeUploadClient::new(resolver, artifact_source, yt_config);
                    let adapter = YouTubeUploadAdapter::new(client);
                    let Some(refresher) = token_refresher_for_provider(
                        &oauth_credentials,
                        &Provider::YouTube,
                        aead_key_clone(&aead_key),
                    ) else {
                        // R27: the job is already claimed (Uploading, attempt
                        // bumped) at this point — a bare `continue` here used
                        // to strand it with no requeue and no event. Restore
                        // it the same way the quota-exhausted arm does, and
                        // never touched the provider, so no quota unit is
                        // consumed (see the quota-increment comment below).
                        tracing::warn!(job_id = %job.id, "YouTube token refresh unavailable: OAuth not configured");
                        if let Err(e) =
                            restore_youtube_refresher_unconfigured_job(&mut store, job, now)
                        {
                            tracing::warn!(
                                "failed to restore refresher-unconfigured YouTube job: {e}"
                            );
                        }
                        continue;
                    };
                    let upload = upload_request_for_job(&store, &job, now);
                    let tracked_adapter = TrackedUploadAdapter::new(&adapter);
                    let execute_result = SocialApi::execute_claimed_upload_job_with_refresher(
                        &mut store,
                        &tracked_adapter,
                        &refresher,
                        upload,
                        TOKEN_REFRESH_SWEEP_SKEW_SECS,
                    );
                    if let Err(e) = &execute_result {
                        tracing::warn!(job_id = %job.id, "YouTube execute failed: {e}");
                    }
                    // R28: quota counts actual upload attempts that reached the
                    // provider, not every claim. `execute_claimed_upload_job_with_refresher`
                    // returns `Ok` both when the provider was genuinely
                    // contacted (success, requires-action, media-constraint
                    // failure, or a retryable 5xx requeue) AND when a purely
                    // local pre-provider failure short-circuits with an `Ok`
                    // job transition (e.g. an exhausted/invalid refresh token
                    // flips the account to NeedsReauth without ever calling
                    // the adapter) — so the `Result` alone can't be trusted
                    // here. `tracked_adapter.dispatched()` is the ground truth:
                    // it is only ever true if `adapter.upload()` — the actual
                    // provider round-trip — was invoked. Only then do we count
                    // the attempt and burn a quota unit; a 5xx still counts
                    // (it reached the provider), but a refresher-unconfigured
                    // or refresh-failure short-circuit never does.
                    if tracked_adapter.dispatched() {
                        youtube_used += 1;
                        let _ = store.increment_youtube_quota(now);
                    }
                }
                Provider::TikTok => {
                    let resolver = ServerAccessTokenResolver::new(
                        store_handle.clone(),
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
                    let client = LiveTikTokUploadClient::with_base(
                        resolver,
                        artifact_source,
                        tiktok_api_base.clone(),
                    );
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
                        store_handle.clone(),
                        aead_key_clone(&aead_key),
                        now,
                    );
                    let client = LiveInstagramUploadClient::with_base(
                        resolver,
                        instagram_graph_base.clone(),
                        instagram_account_id,
                    );
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
                        store_handle.clone(),
                        aead_key_clone(&aead_key),
                        now,
                    );
                    let client =
                        LiveTwitterXUploadClient::with_base(resolver, twitter_x_api_base.clone());
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
        description: if matches!(job.provider, Provider::TikTok | Provider::TwitterX) {
            None
        } else {
            description
        },
        tags: if matches!(job.provider, Provider::TikTok | Provider::TwitterX) {
            Vec::new()
        } else {
            string_array_field(&fields, "tags")
        },
        thumbnail_ref: if matches!(job.provider, Provider::TikTok | Provider::TwitterX) {
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
    let Some(job) = store
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
            restore_youtube_quota_blocked_job(store, job, now)?;
            return Err("youtube daily upload quota reached".into());
        }
    }

    Ok(Some(job))
}

/// R28 — wraps an [`UploadAdapter`] and records whether `upload()` was
/// actually invoked.
///
/// `UploadService::execute_claimed_job_full` returns `Ok(..)` both when the
/// provider was genuinely contacted (success, requires-action,
/// media-constraint-failure, or a retryable 5xx requeue — all call
/// `adapter.upload()` first) AND when a purely local, pre-provider failure
/// short-circuits with a job-level `Ok` (e.g. an unrefreshable/expired token
/// flips the account to `NeedsReauth` and returns `Ok` without ever touching
/// the provider). The `Result` alone can't distinguish these; the adapter
/// itself is the only place that unambiguously knows whether it was called.
/// The quota-increment site uses `dispatched()` after the call, so a job that
/// never reached the provider — regardless of which `Ok`/`Err` path it took —
/// never burns a quota unit; a job that did reach the provider (including a
/// 5xx that gets requeued) correctly does.
struct TrackedUploadAdapter<'a, A: UploadAdapter> {
    inner: &'a A,
    dispatched: std::cell::Cell<bool>,
}

impl<'a, A: UploadAdapter> TrackedUploadAdapter<'a, A> {
    fn new(inner: &'a A) -> Self {
        Self {
            inner,
            dispatched: std::cell::Cell::new(false),
        }
    }

    /// Whether `upload()` was invoked at least once through this wrapper.
    fn dispatched(&self) -> bool {
        self.dispatched.get()
    }
}

impl<A: UploadAdapter> UploadAdapter for TrackedUploadAdapter<'_, A> {
    fn provider(&self) -> Provider {
        self.inner.provider()
    }

    fn upload(
        &self,
        request: &montage_social::upload_adapter::UploadRequest,
    ) -> Result<
        montage_social::upload_adapter::UploadResult,
        montage_social::upload_adapter::UploadAdapterError,
    > {
        self.dispatched.set(true);
        self.inner.upload(request)
    }
}

fn restore_youtube_quota_blocked_job(
    store: &mut impl SocialStore,
    mut job: PublishJob,
    now: i64,
) -> Result<(), String> {
    job.status = PublishJobStatus::Scheduled;
    job.attempt_count = job.attempt_count.saturating_sub(1);
    job.updated_at = now;
    store
        .save_publish_job(job)
        .map_err(|e| format!("restore quota-blocked job: {e}"))
}

/// R27 — a job was already claimed (status `Uploading`, attempt bumped) when
/// the sweep discovered the YouTube token refresher is unconfigured (missing
/// Google OAuth creds). Mirrors [`restore_youtube_quota_blocked_job`]: refund
/// the attempt and restore to `Scheduled` so the job isn't stranded — but also
/// appends a `RetryQueued` event, since "refresher unconfigured" is an
/// operator-visible misconfiguration a bare log line would hide from the job's
/// audit trail.
fn restore_youtube_refresher_unconfigured_job(
    store: &mut impl SocialStore,
    mut job: PublishJob,
    now: i64,
) -> Result<(), String> {
    let job_id = job.id.clone();
    job.status = PublishJobStatus::Scheduled;
    job.attempt_count = job.attempt_count.saturating_sub(1);
    job.updated_at = now;
    store
        .save_publish_job(job)
        .map_err(|e| format!("restore refresher-unconfigured job: {e}"))?;

    let sequence = store
        .publish_job_events(&job_id)
        .map_err(|e| format!("load events for restore: {e}"))?
        .len()
        + 1;
    store
        .append_publish_job_event(PublishJobEvent::new(
            format!("event_{job_id}_retry_queued_{sequence}"),
            job_id,
            PublishJobEventType::RetryQueued,
            PublishJobActorType::System,
            "YouTube token refresher unavailable (OAuth not configured); job restored to Scheduled",
            serde_json::json!({"provider": "youtube", "reason": "refresher_unconfigured"}),
            now,
        ))
        .map_err(|e| format!("append refresher-unconfigured event: {e}"))
}

pub(crate) async fn fire_due_publish_job(state: SharedState, job_id: String) -> Result<(), String> {
    if !state.config.social_firing_enabled {
        return Err("social firing disabled".into());
    }

    let aead_key = aead_key_from_state(&state.config)
        .map_err(|(_, body)| body.0["error"].as_str().unwrap_or("key error").to_string())?;
    let store_handle = state.store.clone();
    let force_private = state.config.youtube_force_private;
    let tiktok_public_posting_enabled = state.config.tiktok_public_posting_enabled;
    let artifact_base_dir = state.config.artifact_base_dir.clone();
    let supabase_url = state.config.supabase_url.clone();
    let supabase_service_key = state.config.supabase_service_key.clone();
    let oauth_credentials = SocialOAuthCredentials::from_config(&state.config);
    let youtube_upload_base = state.config.youtube_upload_base.clone();
    let tiktok_api_base = state.config.tiktok_api_base.clone();
    let instagram_graph_base = state.config.instagram_graph_base.clone();
    let twitter_x_api_base = state.config.twitter_x_api_base.clone();

    tokio::task::spawn_blocking(move || {
        let mut store = store_handle.open();
        let now = now_secs();
        let Some(job) = claim_direct_fire_job(&mut store, &job_id, now)? else {
            return Ok(());
        };

        match &job.provider {
            Provider::YouTube => {
                let resolver = ServerAccessTokenResolver::new(
                    store_handle.clone(),
                    aead_key_clone(&aead_key),
                    now,
                );
                let artifact_source =
                    artifact_source::FileArtifactSource::new(artifact_base_dir.clone())
                        .with_storage_resolver(artifact_source::SupabaseStorageResolver::new(
                            supabase_url.clone(),
                            supabase_service_key.clone(),
                            3600,
                        ));
                let yt_config = YouTubeClientConfig {
                    force_private,
                    upload_base: youtube_upload_base.clone(),
                    ..Default::default()
                };
                let client = LiveYouTubeUploadClient::new(resolver, artifact_source, yt_config);
                let adapter = YouTubeUploadAdapter::new(client);
                let Some(refresher) = token_refresher_for_provider(
                    &oauth_credentials,
                    &Provider::YouTube,
                    aead_key_clone(&aead_key),
                ) else {
                    // R27 (direct-fire analog): the job was already claimed by
                    // `claim_direct_fire_job` above — erroring out here used to
                    // strand it in `Uploading`. Restore it (attempt refunded,
                    // RetryQueued/System event naming the cause) and THEN
                    // surface the error: the caller still learns the fire
                    // failed, but the job stays retryable instead of stuck.
                    if let Err(e) = restore_youtube_refresher_unconfigured_job(&mut store, job, now)
                    {
                        tracing::warn!("failed to restore refresher-unconfigured YouTube job: {e}");
                    }
                    return Err("youtube OAuth not configured".to_string());
                };
                let upload = upload_request_for_job(&store, &job, now);
                let tracked_adapter = TrackedUploadAdapter::new(&adapter);
                let execute_result = SocialApi::execute_claimed_upload_job_with_refresher(
                    &mut store,
                    &tracked_adapter,
                    &refresher,
                    upload,
                    TOKEN_REFRESH_SWEEP_SKEW_SECS,
                );
                // R28 (same contract as the /internal/tick sweep): quota counts
                // actual upload attempts that reached the provider. The execute
                // `Result` alone can't tell "provider was contacted" apart from
                // a purely local pre-provider short-circuit that also returns
                // Ok (e.g. an exhausted/invalid refresh token flipping the
                // account to NeedsReauth without calling the adapter), so
                // `tracked_adapter.dispatched()` — set from inside
                // `adapter.upload()` itself — is the signal. A 5xx that gets
                // requeued still counts (it reached the provider); a local
                // failure never does. Increment before propagating any error so
                // a dispatched-then-failed attempt (e.g. cancel race) is still
                // counted.
                if tracked_adapter.dispatched() {
                    let _ = store.increment_youtube_quota(now);
                }
                execute_result.map_err(|e| format!("youtube execute: {e}"))?;
            }
            Provider::TikTok => {
                let resolver = ServerAccessTokenResolver::new(
                    store_handle.clone(),
                    aead_key_clone(&aead_key),
                    now,
                );
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
                let client = LiveTikTokUploadClient::with_base(
                    resolver,
                    artifact_source,
                    tiktok_api_base.clone(),
                );
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
                let resolver = ServerAccessTokenResolver::new(
                    store_handle.clone(),
                    aead_key_clone(&aead_key),
                    now,
                );
                let client = LiveInstagramUploadClient::with_base(
                    resolver,
                    instagram_graph_base.clone(),
                    instagram_account_id,
                );
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
                let resolver = ServerAccessTokenResolver::new(
                    store_handle.clone(),
                    aead_key_clone(&aead_key),
                    now,
                );
                let client =
                    LiveTwitterXUploadClient::with_base(resolver, twitter_x_api_base.clone());
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
    let store_handle = state.store.clone();
    let youtube_videos_base = state.config.youtube_videos_base.clone();
    let tiktok_api_base = state.config.tiktok_api_base.clone();
    let instagram_graph_base = state.config.instagram_graph_base.clone();
    let twitter_x_api_base = state.config.twitter_x_api_base.clone();

    tokio::task::spawn_blocking(move || {
        let mut store = store_handle.open();
        let now = now_secs();
        let job = store
            .publish_job(&job_id)
            .map_err(|e| format!("job lookup: {e}"))?;
        if job.status != montage_social::model::PublishJobStatus::Processing {
            return Ok(());
        }
        match &job.provider {
            Provider::YouTube => {
                let resolver = ServerAccessTokenResolver::new(
                    store_handle.clone(),
                    aead_key_clone(&aead_key),
                    now,
                );
                let client = LiveYouTubeStatusClient::with_base(resolver, youtube_videos_base);
                let adapter = YouTubeStatusAdapter::new(client);
                SocialApi::poll_upload_status(&mut store, &adapter, &job.id, now)
                    .map_err(|e| format!("youtube poll status: {e}"))?;
            }
            Provider::TikTok => {
                let resolver = ServerAccessTokenResolver::new(
                    store_handle.clone(),
                    aead_key_clone(&aead_key),
                    now,
                );
                let client = LiveTikTokStatusClient::with_base(resolver, tiktok_api_base);
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
                let resolver = ServerAccessTokenResolver::new(
                    store_handle.clone(),
                    aead_key_clone(&aead_key),
                    now,
                );
                let client = LiveInstagramStatusClient::with_base(
                    resolver,
                    instagram_graph_base,
                    account_id,
                );
                let adapter = InstagramStatusAdapter::new(client);
                SocialApi::poll_upload_status(&mut store, &adapter, &job.id, now)
                    .map_err(|e| format!("instagram poll status: {e}"))?;
            }
            Provider::TwitterX => {
                let resolver = ServerAccessTokenResolver::new(
                    store_handle.clone(),
                    aead_key_clone(&aead_key),
                    now,
                );
                let client = LiveTwitterXStatusClient::with_base(resolver, twitter_x_api_base);
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
    let store_handle = state.store.clone();
    let youtube_videos_base = state.config.youtube_videos_base.clone();
    let tiktok_api_base = state.config.tiktok_api_base.clone();
    let instagram_graph_base = state.config.instagram_graph_base.clone();
    let twitter_x_api_base = state.config.twitter_x_api_base.clone();

    let polled = tokio::task::spawn_blocking(move || {
        let mut store = store_handle.open();
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
                        store_handle.clone(),
                        aead_key_clone(&aead_key),
                        now,
                    );
                    let client =
                        LiveYouTubeStatusClient::with_base(resolver, youtube_videos_base.clone());
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
                        store_handle.clone(),
                        aead_key_clone(&aead_key),
                        now,
                    );
                    let client =
                        LiveTikTokStatusClient::with_base(resolver, tiktok_api_base.clone());
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
                        store_handle.clone(),
                        aead_key_clone(&aead_key),
                        now,
                    );
                    let client = LiveInstagramStatusClient::with_base(
                        resolver,
                        instagram_graph_base.clone(),
                        instagram_account_id,
                    );
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
                        store_handle.clone(),
                        aead_key_clone(&aead_key),
                        now,
                    );
                    let client =
                        LiveTwitterXStatusClient::with_base(resolver, twitter_x_api_base.clone());
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
    let store_handle = state.store.clone();
    let oauth_credentials = SocialOAuthCredentials::from_config(&state.config);

    let summary = tokio::task::spawn_blocking(move || {
        let mut store = store_handle.open();
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

/// Per-provider OAuth token endpoint. TikTok and Twitter/X derive from the
/// (overridable) API bases; with the default bases every value is byte-identical
/// to the pre-seam hardcoded literals.
fn token_endpoint(config: &ServerConfig, provider: &Provider) -> String {
    match provider {
        Provider::YouTube => config.google_token_endpoint.clone(),
        Provider::TikTok => tiktok_token_endpoint(&config.tiktok_api_base),
        Provider::Instagram => "https://api.instagram.com/oauth/access_token".to_string(),
        Provider::TwitterX => twitter_x_token_endpoint(&config.twitter_x_api_base),
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
                    credentials.google_token_endpoint.clone(),
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
                    credentials.tiktok_token_endpoint.clone(),
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
                    credentials.twitter_x_token_endpoint.clone(),
                    key,
                ))
            }
        }
    }
}

/// Per-provider profile endpoint used by the platform OAuth exchange. TikTok
/// and Twitter/X derive from the (overridable) API bases; defaults are
/// byte-identical to the pre-seam literals.
fn profile_endpoint(config: &ServerConfig, provider: &Provider) -> Option<String> {
    match provider {
        Provider::YouTube => Some("https://www.googleapis.com/youtube/v3/channels".to_string()),
        Provider::TikTok => Some(format!(
            "{}/v2/user/info/",
            config.tiktok_api_base.trim_end_matches('/')
        )),
        Provider::Instagram => None,
        Provider::TwitterX => Some(format!(
            "{}/2/users/me?user.fields=username,name",
            config.twitter_x_api_base.trim_end_matches('/')
        )),
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
    fn bearer_auth_fails_closed_on_empty_secret() {
        let no_auth = HeaderMap::new();
        assert!(!bearer_auth(&no_auth, ""));
        let empty_bearer = headers_with_auth("Bearer ");
        assert!(!bearer_auth(&empty_bearer, ""));
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
    fn social_allowed_user_ids_parser_trims_and_ignores_empty_segments() {
        assert_eq!(
            parse_social_allowed_user_ids(" user_1, ,cohost_1,, "),
            vec!["user_1".to_string(), "cohost_1".to_string()]
        );
    }

    #[test]
    fn redirect_uri_is_built_from_base_and_slug() {
        let config = ServerConfig {
            oauth_redirect_base: "https://app.example".into(),
            ..ServerConfig::default()
        };
        assert_eq!(
            redirect_uri(&config, &Provider::YouTube),
            "https://app.example/oauth/callback/youtube"
        );
    }

    #[test]
    fn provider_client_id_reads_config_for_each_major_platform() {
        let config = ServerConfig {
            oauth_redirect_base: "https://app.example".into(),
            google_client_id: "google-client".into(),
            tiktok_client_key: "tiktok-key".into(),
            instagram_client_id: "instagram-client".into(),
            twitter_x_client_id: "twitter-client".into(),
            ..ServerConfig::default()
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
        let config = ServerConfig::default();
        assert_eq!(
            token_endpoint(&config, &Provider::Instagram),
            "https://api.instagram.com/oauth/access_token"
        );
        assert_eq!(profile_endpoint(&config, &Provider::Instagram), None);
    }

    /// The base-URL seam must be behavior-preserving: with a default config the
    /// derived endpoints are byte-identical to the pre-seam hardcoded literals.
    #[test]
    fn default_endpoints_match_pre_seam_production_literals() {
        let config = ServerConfig::default();
        assert_eq!(
            token_endpoint(&config, &Provider::YouTube),
            "https://oauth2.googleapis.com/token"
        );
        assert_eq!(
            token_endpoint(&config, &Provider::TikTok),
            "https://open.tiktokapis.com/v2/oauth/token/"
        );
        assert_eq!(
            token_endpoint(&config, &Provider::TwitterX),
            "https://api.x.com/2/oauth2/token"
        );
        assert_eq!(
            profile_endpoint(&config, &Provider::YouTube).as_deref(),
            Some("https://www.googleapis.com/youtube/v3/channels")
        );
        assert_eq!(
            profile_endpoint(&config, &Provider::TikTok).as_deref(),
            Some("https://open.tiktokapis.com/v2/user/info/")
        );
        assert_eq!(
            profile_endpoint(&config, &Provider::TwitterX).as_deref(),
            Some("https://api.x.com/2/users/me?user.fields=username,name")
        );
        assert_eq!(
            config.youtube_upload_base,
            "https://www.googleapis.com/upload/youtube/v3/videos"
        );
        assert_eq!(
            config.youtube_videos_base,
            "https://www.googleapis.com/youtube/v3/videos"
        );
        assert_eq!(
            config.instagram_graph_base,
            "https://graph.instagram.com/v24.0"
        );
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
    fn bulk_tick_restores_due_youtube_job_when_quota_is_exhausted_after_claim() {
        use montage_social::{
            model::PublishJob,
            store::{InMemorySocialStore, SocialStore},
        };

        let now = 1_000;
        let mut store = InMemorySocialStore::default();
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

        let claimed = store.claim_due_publish_jobs(now, 10).unwrap();
        assert_eq!(claimed.len(), 1);
        let claimed_job = claimed.into_iter().next().unwrap();
        restore_youtube_quota_blocked_job(&mut store, claimed_job, now).unwrap();

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
    fn upload_request_for_job_drops_stale_tiktok_unused_fields() {
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
                "title": "Launch TikTok caption",
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
        assert_eq!(request.title, "Launch TikTok caption");
        assert_eq!(request.description, None);
        assert!(request.tags.is_empty());
        assert_eq!(request.thumbnail_ref, None);
        assert_eq!(request.privacy, Some(UploadPrivacy::Private));
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
