//! `poll_generated_media_job` — read a generated-media registry record
//! from disk and return the job state plus output metadata. Ported from
//! `crates/core/src/tools/poll_generated_media_job.rs` to the
//! in-process MCP server.
//!
//! In the old harness the tool ran inside an async loop and, for
//! OpenRouter-backed jobs, would poll the provider, download completed
//! outputs, and write the result back into the registry. The MCP port
//! drops that side-effectful path: it returns whatever state the
//! on-disk registry currently holds. Re-polling OpenRouter while a job
//! is in flight belongs to a future out-of-process job worker; for now
//! the agent inspects the persisted record and re-submits a job if it's
//! stuck.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::awidat_mcp::context::McpToolCtx;
use crate::generated_media::registry::Registry;

/// Arguments to `poll_generated_media_job`.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct PollGeneratedMediaJobArgs {
    /// Job id previously returned by `start_generated_media_job`.
    pub job_id: String,
}

pub fn run(args: PollGeneratedMediaJobArgs, ctx: McpToolCtx) -> Result<String, String> {
    if args.job_id.trim().is_empty() {
        return Err("poll_generated_media_job: job_id must not be empty.".into());
    }

    let registry = Registry::load_or_default(&ctx.project_root)
        .map_err(|e| format!("poll_generated_media_job: {e}"))?;
    let record = registry.get(&args.job_id).cloned().ok_or_else(|| {
        format!(
            "poll_generated_media_job: job '{}' not found.",
            args.job_id
        )
    })?;

    Ok(serde_json::json!({
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
    .to_string())
}

pub const DESCRIPTION: &str = "\
Read a generated-media registry record and return job state plus output \
metadata. Read-only: reads the on-disk registry under \
`<project>/.awidat/generated-media/registry.json` and returns whatever \
state was persisted by `start_generated_media_job`. NOTE: the \
in-process MCP server does NOT poll OpenRouter or download completed \
outputs from this call — if the record is still in flight, re-run \
`start_generated_media_job` from a desktop session that has the \
out-of-process worker wired up.";
