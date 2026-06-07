//! `podcast_audio_polish` — forced audio finishing pass for podcasts.
//! Ported from `crates/core/src/tools/podcast_audio_polish.rs` to the
//! in-process MCP server.

use montage_proto::project::Project;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::montage_mcp::context::McpToolCtx;

/// Arguments to `podcast_audio_polish`.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct PodcastAudioPolishArgs {
    /// Integrated loudness target. Default -16 LUFS for stereo podcasts.
    #[serde(default)]
    pub target_lufs: Option<f64>,
}

/// Run `podcast_audio_polish` against the project resolved from
/// [`McpToolCtx`]. Returns the JSON body as `Ok(String)`;
/// project-read errors return `Err(String)`.
pub fn run(args: PodcastAudioPolishArgs, ctx: McpToolCtx) -> Result<String, String> {
    let target_lufs = args.target_lufs.unwrap_or(-16.0);
    let project = Project::read(&ctx.project_root)
        .map_err(|e| format!("podcast_audio_polish: failed to read project: {e}"))?;
    let finishing = crate::professional::derive_audio_finishing_state(&project.timeline);
    let mut issues = Vec::new();
    let mut recommendations = Vec::new();

    if finishing.buses.is_empty() {
        issues.push(serde_json::json!({
            "kind": "missing_audio_buses",
            "severity": "warning",
            "message": "No audio finishing buses were derived from the timeline."
        }));
    }
    if !finishing
        .chains
        .iter()
        .any(|chain| chain.id == "dialogue_cleanup")
    {
        recommendations
            .push("Add dialogue cleanup chain: noise reduction, de-esser, EQ, compression.");
    }
    if !finishing
        .chains
        .iter()
        .any(|chain| chain.id == "master_delivery")
    {
        recommendations.push("Add master delivery chain: limiter and loudness meter.");
    }

    if finishing.meters.is_empty() {
        issues.push(serde_json::json!({
            "kind": "missing_audio_measurements",
            "severity": "warning",
            "message": "No audio_measurements metadata found; loudness, true peak, noise, and clipping need a meter pass."
        }));
    }
    for meter in &finishing.meters {
        if meter.clipping {
            issues.push(serde_json::json!({
                "kind": "clipping",
                "severity": "error",
                "target": meter.target,
                "message": "Clipping detected; reduce gain or repair clipped dialogue before render."
            }));
        }
        if let Some(lufs) = meter.integrated_lufs
            && meter.target == "master"
            && (lufs - target_lufs).abs() > 2.0
        {
            issues.push(serde_json::json!({
                "kind": "loudness_out_of_range",
                "severity": "warning",
                "target": meter.target,
                "integrated_lufs": lufs,
                "target_lufs": target_lufs,
                "message": "Master loudness is outside the podcast target window."
            }));
        }
        if let Some(true_peak) = meter.true_peak_db
            && true_peak > -1.0
        {
            issues.push(serde_json::json!({
                "kind": "true_peak_too_hot",
                "severity": "warning",
                "target": meter.target,
                "true_peak_db": true_peak,
                "message": "True peak should stay at or below -1 dBTP for delivery safety."
            }));
        }
        if let Some(noise_floor) = meter.noise_floor_db
            && noise_floor > -45.0
        {
            issues.push(serde_json::json!({
                "kind": "high_noise_floor",
                "severity": "warning",
                "target": meter.target,
                "noise_floor_db": noise_floor,
                "message": "Noise floor is high enough to need noise reduction or room-tone work."
            }));
        }
    }

    recommendations.extend([
        "Balance speaker dialogue levels before final render.",
        "Use light compression and de-essing on dialogue; avoid over-processing.",
        "Set final loudness target via Set Loudness Target before render.",
    ]);
    let status = if issues.iter().any(|issue| issue["severity"] == "error") {
        "needs_fix"
    } else if issues.is_empty() {
        "ready"
    } else {
        "needs_review"
    };
    let body = serde_json::json!({
        "status": status,
        "summary_for_agent": format!("Audio polish status: {status}. {} issue(s), {} recommendation(s).", issues.len(), recommendations.len()),
        "target_lufs": target_lufs,
        "audio_finishing": finishing,
        "issues": issues,
        "recommendations": recommendations,
        "required_before_render": true,
    });
    serde_json::to_string(&body).map_err(|e| format!("podcast_audio_polish serialize: {e}"))
}

pub const DESCRIPTION: &str = "\
Check podcast audio finishing readiness: loudness, clipping, noise, buses, \
and recommended mix processors. Returns a status (ready / needs_review / \
needs_fix), issues by severity, the derived audio finishing state, and \
recommended mix-chain steps. Read-only; reports against current timeline \
metadata.\
";
