//! `shot_summary` — compact roll-up of an asset's visual structure.
//! Ported from `crates/core/src/tools/shot_summary.rs` to the
//! in-process MCP server. Returns per-asset shot counts, mean shot
//! length, and histograms over shot type / motion.

use montage_index::walk_indexer;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::montage_mcp::context::McpToolCtx;

/// Arguments to `shot_summary`.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct ShotSummaryArgs {
    /// Restrict to one asset id; otherwise summarize each asset.
    #[serde(default)]
    pub asset_id: Option<String>,
}

/// Run `shot_summary` against the project resolved from
/// [`McpToolCtx`]. Returns the JSON body as `Ok(String)`; sidecar
/// loading errors return `Err(String)`.
pub fn run(args: ShotSummaryArgs, ctx: McpToolCtx) -> Result<String, String> {
    let walker = walk_indexer(&ctx.project_root, "shot").map_err(|e| {
        format!(
            "shot_summary: shot sidecars not readable ({e}). \
             Run `montage index --indexer shot <project>` and retry."
        )
    })?;

    let mut summaries: Vec<serde_json::Value> = Vec::new();
    for (asset_id, sidecar) in walker {
        if let Some(filter) = &args.asset_id
            && filter != &asset_id
        {
            continue;
        }
        let Some(shots) = sidecar
            .pointer("/data/shots")
            .and_then(|v| v.as_array())
            .cloned()
        else {
            continue;
        };
        let n_shots = shots.len();
        let total_duration_s: f64 = shots
            .iter()
            .map(|s| {
                let st = s.get("start_s").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let en = s.get("end_s").and_then(|v| v.as_f64()).unwrap_or(0.0);
                (en - st).max(0.0)
            })
            .sum();
        let mean_shot_s = if n_shots > 0 {
            total_duration_s / n_shots as f64
        } else {
            0.0
        };

        // Histograms.
        let mut by_type: std::collections::BTreeMap<String, u32> = Default::default();
        let mut by_motion: std::collections::BTreeMap<String, u32> = Default::default();
        for s in &shots {
            let t = s
                .get("type")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            *by_type.entry(t).or_insert(0) += 1;
            let m = s
                .get("motion")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            *by_motion.entry(m).or_insert(0) += 1;
        }

        summaries.push(serde_json::json!({
            "asset_id": asset_id,
            "shot_count": n_shots,
            "total_shot_seconds": total_duration_s,
            "mean_shot_seconds": mean_shot_s,
            "by_type": by_type,
            "by_motion": by_motion,
        }));
    }

    Ok(serde_json::json!({
        "summaries": summaries,
    })
    .to_string())
}

pub const DESCRIPTION: &str = "\
Compact descriptive summary of an episode's visual structure: shot \
count, mean shot length, histograms over shot type \
(close-up/medium/wide/no-face) and motion (static/slow-pan/handheld/\
fast-cut). Reads the `shot` indexer's sidecar.\
\n\n\
Use this to orient yourself to a new asset — the answer to 'what \
does this video look like, structurally?' before deciding whether \
to call broll_candidates, find_speaker_oncam, or just inspect a few \
clips.\
";
