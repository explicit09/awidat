//! `poll_generated_media_job` — poll the provider for a generated-media
//! job, download any completed output, persist registry updates, and
//! return the resolved state. Synchronous within one tool call:
//!
//! - For `provider = "openrouter"`: GET the status URL, and on
//!   `completed` download the first video output to
//!   `<project>/raw/generated/openrouter/<job_id>.mp4`, then update the
//!   on-disk registry via `apply_status_to_record` + `Registry::upsert`.
//! - For `provider = "mock"`: return the persisted record as-is (the
//!   `mock` provider in `start_generated_media_job` already lands the
//!   record in its final state).
//!
//! The earlier MCP port deliberately gutted polling and downloading,
//! deferring to a future out-of-process worker. That worker never
//! shipped and the agent was stuck looking at `queued` records forever
//! — three OpenRouter jobs sat completed on the provider side at
//! ~$0.23/job while Awidat believed they were still in flight. This
//! restores the working sync flow.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::awidat_mcp::context::McpToolCtx;
use crate::generated_media::openrouter::{
    OpenRouterVideoConfig, apply_status_to_record, download_completed_video,
    local_video_output_path, poll_video_job,
};
use crate::generated_media::registry::{GeneratedMediaRecord, GeneratedMediaState, Registry};

/// Arguments to `poll_generated_media_job`.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct PollGeneratedMediaJobArgs {
    /// Job id previously returned by `start_generated_media_job`.
    pub job_id: String,
    /// Block server-side until the job reaches a terminal state
    /// (succeeded/failed/cancelled) or this many seconds elapse.
    /// Default 0 (single-shot poll, returns whatever state codex sees
    /// right now — same behavior as the legacy stub). Capped at 180s
    /// so a single tool call can't hang for more than 3 minutes.
    ///
    /// Why this exists: without it the agent ended up calling poll
    /// 20-30 times in a row while it waited for video gen, each call
    /// burning context for the same `queued` answer. One waiting call
    /// terminates in O(generation_time) instead.
    #[serde(default)]
    pub wait_until_terminal_s: Option<u32>,
}

/// Hard cap on wait_until_terminal_s. Server-side waits longer than
/// this risk timing out the codex tool call or pinning a worker;
/// agent can re-call to keep waiting if it really wants.
const MAX_WAIT_S: u32 = 180;

/// Server-side poll interval inside a wait_until_terminal loop. The
/// OpenRouter generation cycle is on the order of tens of seconds, so
/// polling every 5s is fast enough to feel responsive without
/// hammering the API.
const WAIT_POLL_INTERVAL_S: u64 = 5;

pub async fn run(args: PollGeneratedMediaJobArgs, ctx: McpToolCtx) -> Result<String, String> {
    if args.job_id.trim().is_empty() {
        return Err("poll_generated_media_job: job_id must not be empty.".into());
    }

    let wait_budget_s = args.wait_until_terminal_s.unwrap_or(0).min(MAX_WAIT_S);
    let deadline =
        std::time::Instant::now().checked_add(std::time::Duration::from_secs(wait_budget_s as u64));

    loop {
        let registry = Registry::load_or_default(&ctx.project_root)
            .map_err(|e| format!("poll_generated_media_job: load registry: {e}"))?;
        let mut record = registry
            .get(&args.job_id)
            .cloned()
            .ok_or_else(|| format!("poll_generated_media_job: job '{}' not found.", args.job_id))?;

        // Terminal states need no polling — return cached state. Same
        // for mock-provider records.
        let terminal = matches!(
            record.state,
            GeneratedMediaState::Succeeded
                | GeneratedMediaState::Failed
                | GeneratedMediaState::Cancelled
        );
        if terminal || record.provider == "mock" {
            return Ok(serialize_record(&record));
        }

        // Non-OpenRouter providers: nothing to actively poll, just
        // return whatever's persisted.
        if record.provider != "openrouter" {
            return Ok(serialize_record(&record));
        }

        let status_url = record.provider_status_url.clone().ok_or_else(|| {
            format!(
                "poll_generated_media_job: openrouter record '{}' has no provider_status_url.",
                args.job_id
            )
        })?;

        let config = OpenRouterVideoConfig::from_env(None)
            .map_err(|e| format!("poll_generated_media_job: openrouter config: {e}"))?;

        let status = poll_video_job(&config, &status_url)
            .await
            .map_err(|e| format!("poll_generated_media_job: openrouter poll: {e}"))?;

        let mut output_video_path: Option<String> = None;
        let succeeded = matches!(status.status.as_str(), "completed" | "succeeded");
        if succeeded {
            // Always re-attempt the download on `completed`: a prior
            // call may have updated state but failed mid-download.
            let project_relative = local_video_output_path(&record.job_id);
            let absolute = ctx.project_root.join(&project_relative);
            download_completed_video(&config, &status, &absolute)
                .await
                .map_err(|e| format!("poll_generated_media_job: openrouter download: {e}"))?;
            output_video_path = Some(project_relative);
        }

        apply_status_to_record(&mut record, &status, output_video_path)
            .map_err(|e| format!("poll_generated_media_job: apply status: {e}"))?;

        registry
            .upsert(&ctx.project_root, record.clone())
            .map_err(|e| format!("poll_generated_media_job: persist registry: {e}"))?;

        // Loop if the caller asked us to wait and we haven't blown
        // the budget yet. Otherwise return whatever we just observed.
        let still_in_flight = !matches!(
            record.state,
            GeneratedMediaState::Succeeded
                | GeneratedMediaState::Failed
                | GeneratedMediaState::Cancelled
        );
        let have_time = deadline.is_some_and(|d| std::time::Instant::now() < d);
        if still_in_flight && have_time {
            tokio::time::sleep(std::time::Duration::from_secs(WAIT_POLL_INTERVAL_S)).await;
            continue;
        }

        return Ok(serialize_record(&record));
    }
}

fn serialize_record(record: &GeneratedMediaRecord) -> String {
    serde_json::json!({
        "job_id": record.job_id,
        "state": record.state,
        "provider": record.provider,
        "model": record.model,
        "output_paths": record.output_paths,
        "cost_estimate_usd": record.cost_estimate_usd,
        "cost_actual_usd": record.cost_actual_usd,
        "uses_likeness": record.uses_likeness,
        "requires_disclosure": record.requires_disclosure,
        "failure_message": record.failure_message,
    })
    .to_string()
}

pub const DESCRIPTION: &str = "\
Poll a generated-media job. For OpenRouter video jobs this issues a \
GET against the provider status URL and, on `completed`, downloads \
the video into `<project>/raw/generated/openrouter/<job_id>.mp4` and \
persists the registry update. Returns the resolved state plus output \
paths.\
\n\nPrefer `wait_until_terminal_s: <seconds>` (max 180) when you want \
to BLOCK until the job is done — the tool re-polls server-side every \
5s until succeeded/failed/cancelled or the budget elapses. This \
replaces what would otherwise be 20-30 separate `poll` calls back \
to back, each one burning agent context for the same `queued` \
answer. Pattern: spawn jobs with start_generated_media_job, then call \
poll_generated_media_job(job_id=…, wait_until_terminal_s=120) once \
per job. Safe to retry: terminal states return cached records, and \
downloads are idempotent re-writes of the same file.";
