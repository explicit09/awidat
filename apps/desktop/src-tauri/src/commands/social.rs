//! Server-backed social publishing Tauri commands.
//!
//! Phase 5: each command is a thin translation to the `montage-social-server`
//! over HTTPS via [`SocialClient`]. The desktop holds **no secrets** — no
//! token-encryption key, no `client_secret`, no local OAuth listener, no local
//! provider upload. OAuth is initiated by opening the system browser to the
//! *server's* auth-start URL; the provider redirects to the *server* callback,
//! so the desktop never sees the `code` and simply re-polls `social_accounts`
//! to discover the newly-connected account. Firing scheduled jobs is the
//! server's job (pg_cron); the desktop only schedules + polls.
//!
use montage_social::api::{AccountSummary, OAuthStartResponse};
use montage_social::api::{
    BindTargetRequest, PublishJobResponse, RescheduleJobRequest, ScheduleTargetRequest,
    UpdateTargetRequest, ValidateTargetRequest, ValidateTargetResponse,
};
use montage_social::model::{AccountUsageAudit, CampaignVariantTarget, Provider};
use tauri::State;

use crate::social_client::SocialClient;
use crate::state::MontageState;

/// Clones the initialized social client out of state, or returns a stable error
/// string when the server URL was never configured (`MONTAGE_SOCIAL_SERVER_URL`
/// unset at startup). `SocialClient` is cheap to clone (the inner
/// `reqwest::Client` is an `Arc`), so we clone-and-drop the lock rather than
/// hold it across the network round-trip.
async fn client(state: &State<'_, MontageState>) -> Result<SocialClient, String> {
    state
        .social_client
        .lock()
        .await
        .clone()
        .ok_or_else(|| "social client not initialized".to_string())
}

#[tauri::command]
pub async fn social_accounts(
    state: State<'_, MontageState>,
) -> Result<Vec<AccountSummary>, String> {
    client(&state).await?.accounts().await
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuthStartArgs {
    pub provider: Provider,
    /// Optional path the server should bounce back to after the callback.
    #[serde(default)]
    pub return_to: Option<String>,
}

#[tauri::command]
pub async fn social_oauth_start(
    state: State<'_, MontageState>,
    args: OAuthStartArgs,
) -> Result<OAuthStartResponse, String> {
    client(&state)
        .await?
        .oauth_start(&args.provider, args.return_to.unwrap_or_default())
        .await
}

#[tauri::command]
pub async fn social_disconnect_account(
    state: State<'_, MontageState>,
    account_id: String,
) -> Result<AccountSummary, String> {
    client(&state).await?.disconnect_account(&account_id).await
}

// --- Publish routes ---------------------------------------------------------

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BindArgs {
    pub target_id: String,
    pub campaign_id: String,
    pub variant_id: String,
    pub connected_account_id: String,
    pub platform_fields: serde_json::Value,
    pub scheduled_for: i64,
    pub now: i64,
}

#[tauri::command]
pub async fn social_bind_target(
    state: State<'_, MontageState>,
    args: BindArgs,
) -> Result<CampaignVariantTarget, String> {
    client(&state)
        .await?
        .bind_target(&BindTargetRequest {
            target_id: args.target_id,
            campaign_id: args.campaign_id,
            variant_id: args.variant_id,
            connected_account_id: args.connected_account_id,
            platform_fields: args.platform_fields,
            scheduled_for: args.scheduled_for,
            now: args.now,
        })
        .await
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateTargetArgs {
    pub target_id: String,
    pub platform_fields: serde_json::Value,
    pub scheduled_for: i64,
    pub now: i64,
}

#[tauri::command]
pub async fn social_update_target(
    state: State<'_, MontageState>,
    args: UpdateTargetArgs,
) -> Result<CampaignVariantTarget, String> {
    client(&state)
        .await?
        .update_target(&UpdateTargetRequest {
            target_id: args.target_id,
            platform_fields: args.platform_fields,
            scheduled_for: args.scheduled_for,
            now: args.now,
        })
        .await
}

#[tauri::command]
pub async fn social_validate_target(
    state: State<'_, MontageState>,
    target_id: String,
    now: i64,
) -> Result<ValidateTargetResponse, String> {
    client(&state)
        .await?
        .validate_target(&ValidateTargetRequest { target_id, now })
        .await
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleArgs {
    pub target_id: String,
    pub job_id: String,
    pub artifact_ref: String,
    /// Echoed onto the job for the audit trail. The server's authed actor is the
    /// source of truth for authorization; this is metadata only.
    #[serde(default)]
    pub created_by: String,
    pub now: i64,
}

#[tauri::command]
pub async fn social_schedule_target(
    state: State<'_, MontageState>,
    args: ScheduleArgs,
) -> Result<PublishJobResponse, String> {
    client(&state)
        .await?
        .schedule_target(&ScheduleTargetRequest {
            target_id: args.target_id,
            job_id: args.job_id,
            artifact_ref: args.artifact_ref,
            created_by: args.created_by,
            now: args.now,
        })
        .await
}

#[tauri::command]
pub async fn social_publish_job(
    state: State<'_, MontageState>,
    job_id: String,
) -> Result<PublishJobResponse, String> {
    client(&state).await?.publish_job(&job_id).await
}

#[tauri::command]
pub async fn social_cancel_job(
    state: State<'_, MontageState>,
    job_id: String,
) -> Result<PublishJobResponse, String> {
    client(&state).await?.cancel_job(&job_id).await
}

#[tauri::command]
pub async fn social_retry_job(
    state: State<'_, MontageState>,
    job_id: String,
) -> Result<PublishJobResponse, String> {
    client(&state).await?.retry_job(&job_id).await
}

#[tauri::command]
pub async fn social_fire_due_job(
    state: State<'_, MontageState>,
    job_id: String,
) -> Result<PublishJobResponse, String> {
    client(&state).await?.fire_due_job(&job_id).await
}

#[tauri::command]
pub async fn social_poll_publish_job(
    state: State<'_, MontageState>,
    job_id: String,
) -> Result<PublishJobResponse, String> {
    client(&state).await?.poll_publish_job(&job_id).await
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RescheduleArgs {
    pub scheduled_for: i64,
}

#[tauri::command]
pub async fn social_reschedule_job(
    state: State<'_, MontageState>,
    job_id: String,
    args: RescheduleArgs,
) -> Result<PublishJobResponse, String> {
    client(&state)
        .await?
        .reschedule_job(
            &job_id,
            &RescheduleJobRequest {
                scheduled_for: args.scheduled_for,
                // The server overwrites this with its own clock.
                now: 0,
            },
        )
        .await
}

#[tauri::command]
pub async fn social_account_audit(
    state: State<'_, MontageState>,
    account_id: String,
) -> Result<AccountUsageAudit, String> {
    client(&state).await?.account_audit(&account_id).await
}

// --- Upload-to-server -------------------------------------------------------

/// Stage a rendered artifact for a job on server-side storage.
///
/// Three-step handshake: ask the server for a signed upload URL, stream the file
/// to it from disk, then tell the server the bytes are staged so it records the
/// storage ref on the job. The provider upload itself stays entirely
/// server-side (pg_cron fires it). Returns the refreshed job.
#[tauri::command]
pub async fn social_upload_artifact(
    state: State<'_, MontageState>,
    job_id: String,
    file_path: String,
) -> Result<PublishJobResponse, String> {
    let client = client(&state).await?;
    let upload = client.request_upload_url(&job_id).await?;
    if upload.direct {
        // Reserved: the signed-URL path always returns direct=false today. A
        // server-proxied multipart fallback lands here when storage policy
        // requires it.
        return Err("direct multipart upload not supported by this client".to_string());
    }
    client
        .put_file(&upload.url, std::path::Path::new(&file_path))
        .await?;
    // No storage_ref echoed back — the server derives it from (bucket, job_id).
    client.complete_upload(&job_id).await
}
