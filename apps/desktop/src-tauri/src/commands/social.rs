//! Server-backed social publishing Tauri commands.
//!
//! Phase 5: each command is a thin translation to the `awidat-social-server`
//! over HTTPS via [`SocialClient`]. The desktop holds **no secrets** — no
//! token-encryption key, no `client_secret`, no local OAuth listener, no local
//! provider upload. OAuth is initiated by opening the system browser to the
//! *server's* auth-start URL; the provider redirects to the *server* callback,
//! so the desktop never sees the `code` and simply re-polls `social_accounts`
//! to discover the newly-connected account. Firing scheduled jobs is the
//! server's job (pg_cron); the desktop only schedules + polls.
//!
//! `social_providers` stays static: it is pure registry data (provider slots +
//! capability summaries) carrying no secrets. It can move server-side later.

use awidat_social::api::{AccountSummary, OAuthStartResponse};
use awidat_social::api::{
    BindTargetRequest, ProviderSummary, PublishJobResponse, RescheduleJobRequest,
    ScheduleTargetRequest, SocialApi, ValidateTargetRequest, ValidateTargetResponse,
};
use awidat_social::model::{AccountUsageAudit, CampaignVariantTarget, Provider};
use awidat_social::provider::ProviderRegistry;
use tauri::State;

use crate::social_client::SocialClient;
use crate::state::AwidatState;

/// Clones the initialized social client out of state, or returns a stable error
/// string when the server URL was never configured (`AWIDAT_SOCIAL_SERVER_URL`
/// unset at startup). `SocialClient` is cheap to clone (the inner
/// `reqwest::Client` is an `Arc`), so we clone-and-drop the lock rather than
/// hold it across the network round-trip.
async fn client(state: &State<'_, AwidatState>) -> Result<SocialClient, String> {
    state
        .social_client
        .lock()
        .await
        .clone()
        .ok_or_else(|| "social client not initialized".to_string())
}

#[tauri::command]
pub async fn social_providers() -> Result<Vec<ProviderSummary>, String> {
    let registry = ProviderRegistry::default_multi_platform();
    Ok(SocialApi::providers(&registry))
}

#[tauri::command]
pub async fn social_accounts(state: State<'_, AwidatState>) -> Result<Vec<AccountSummary>, String> {
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
    state: State<'_, AwidatState>,
    args: OAuthStartArgs,
) -> Result<OAuthStartResponse, String> {
    client(&state)
        .await?
        .oauth_start(&args.provider, args.return_to.unwrap_or_default())
        .await
}

#[tauri::command]
pub async fn social_disconnect_account(
    state: State<'_, AwidatState>,
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
    state: State<'_, AwidatState>,
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

#[tauri::command]
pub async fn social_validate_target(
    state: State<'_, AwidatState>,
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
    state: State<'_, AwidatState>,
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
    state: State<'_, AwidatState>,
    job_id: String,
) -> Result<PublishJobResponse, String> {
    client(&state).await?.publish_job(&job_id).await
}

#[tauri::command]
pub async fn social_cancel_job(
    state: State<'_, AwidatState>,
    job_id: String,
) -> Result<PublishJobResponse, String> {
    client(&state).await?.cancel_job(&job_id).await
}

#[tauri::command]
pub async fn social_retry_job(
    state: State<'_, AwidatState>,
    job_id: String,
) -> Result<PublishJobResponse, String> {
    client(&state).await?.retry_job(&job_id).await
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RescheduleArgs {
    pub scheduled_for: i64,
}

#[tauri::command]
pub async fn social_reschedule_job(
    state: State<'_, AwidatState>,
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
    state: State<'_, AwidatState>,
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
    state: State<'_, AwidatState>,
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn providers_list_has_major_platform_slots() {
        let registry = ProviderRegistry::default_multi_platform();
        assert_eq!(SocialApi::providers(&registry).len(), 4);
    }

    #[test]
    fn providers_payload_carries_no_token_material() {
        let registry = ProviderRegistry::default_multi_platform();
        let providers = SocialApi::providers(&registry);
        let json = serde_json::to_string(&providers).expect("serialize providers");
        assert!(!json.contains("access_token"));
        assert!(!json.contains("refresh_token"));
        assert!(!json.contains("client_secret"));
    }
}
