//! awidat-social-server — Axum HTTP wrapper over the awidat-social domain crate.
//!
//! Phase 1: skeleton with mock upload adapters, real PgSocialStore, and the
//! /internal/tick endpoint code-guarded by SOCIAL_FIRING_ENABLED=false.
//! Phases 2/3/4 replace the mock adapters with real provider clients.
//!
//! Environment variables (all required at runtime):
//!   DATABASE_URL            — Supavisor session-pooler URL
//!   SERVICE_SHARED_SECRET   — bearer token that pg_net sends to /internal/tick
//!   BIND_ADDR               — e.g. "0.0.0.0:3000" (default "0.0.0.0:3000")
//!   SOCIAL_FIRING_ENABLED   — "true" enables real job execution (default "false")
//!   SUPABASE_URL            — Supabase project URL (for Storage signed URLs)
//!   SUPABASE_SERVICE_KEY    — service_role key (for Storage signed URL minting)
//!   STORAGE_BUCKET          — name of the Supabase Storage bucket for artifacts

use awidat_social::{
    api::{ExecuteUploadRequest, SocialApi},
    pg_store::PgSocialStore,
    provider::ProviderRegistry,
    store::SocialStore,
    upload_adapter::MockUploadAdapter,
};
use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::{get, post},
};
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
}

// ── App state ─────────────────────────────────────────────────────────────────

/// All routes share this state; methods that mutate the store use the Mutex.
/// `spawn_blocking` moves a clone of the pool; the Mutex is only for the axum
/// handler layer where we need `&mut impl SocialStore` (D1: sync domain, async shell).
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
        },
    });

    let app = Router::new()
        .route("/health", get(health_handler))
        .route("/providers", get(providers_handler))
        .route("/artifacts/upload-url", post(artifacts_upload_url_handler))
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

// ── Handlers ──────────────────────────────────────────────────────────────────

async fn health_handler() -> Json<serde_json::Value> {
    Json(serde_json::json!({"status": "ok", "service": "awidat-social-server"}))
}

async fn providers_handler(State(state): State<SharedState>) -> Json<serde_json::Value> {
    let providers = SocialApi::providers(&state.registry);
    Json(serde_json::json!({"providers": providers}))
}

/// `POST /artifacts/upload-url` (D4)
///
/// Returns a Supabase Storage signed PUT URL and the object key that becomes
/// `artifact_ref` in publish jobs.  Provider adapters in Phases 3/6 fetch the
/// artifact from the signed GET URL they request via this same endpoint.
///
/// Phase 1 implementation: builds the URL using the Supabase REST Storage API.
/// Callers must supply the object path they want to upload to.
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

async fn artifacts_upload_url_handler(
    State(state): State<SharedState>,
    Json(body): Json<ArtifactUploadUrlRequest>,
) -> Result<Json<ArtifactUploadUrlResponse>, (StatusCode, Json<serde_json::Value>)> {
    if state.config.supabase_url.is_empty() || state.config.supabase_service_key.is_empty() {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({"error": "storage not configured"})),
        ));
    }

    let expires_in = body.expires_in_secs.unwrap_or(3600);
    let bucket = &state.config.storage_bucket;
    let object_path = &body.object_path;

    // Call Supabase Storage REST API to get a signed upload URL.
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

/// `POST /internal/tick` — cron trigger from Supabase `pg_net`.
///
/// Protected by the `SERVICE_SHARED_SECRET` bearer token.
/// Code-guarded by `SOCIAL_FIRING_ENABLED=false` (G10): when disabled, logs
/// the tick but performs no job execution.  This prevents a stray cron
/// from driving jobs through mock adapters before Phases 2–4 are live.
async fn internal_tick_handler(
    State(state): State<SharedState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    // Authenticate the cron caller.
    let auth = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("");
    if auth != state.config.service_shared_secret {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error": "unauthorized"})),
        ));
    }

    // G10: code-guard, not discipline.
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
