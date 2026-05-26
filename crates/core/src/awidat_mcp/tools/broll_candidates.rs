//! `broll_candidates` — find shots usable as B-roll cutaways.
//! Ported from `crates/core/src/tools/broll_candidates.rs` to the
//! in-process MCP server.

use awidat_index::walk_indexer;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::awidat_mcp::context::McpToolCtx;

/// Arguments to `broll_candidates`.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct BrollCandidatesArgs {
    /// Restrict to one asset id; otherwise scan all shot sidecars.
    #[serde(default)]
    pub asset_id: Option<String>,
    /// Minimum shot duration in seconds. Default 1.5s.
    #[serde(default)]
    pub min_duration_s: Option<f64>,
    /// Allowed shot types. Default ["no-face", "wide"].
    #[serde(default)]
    pub types: Option<Vec<String>>,
    /// Allowed motion buckets. Default ["static", "slow-pan"].
    #[serde(default)]
    pub motions: Option<Vec<String>>,
    /// If set, require the shot's frame-quality fraction_sharp to be
    /// above this threshold. Default 0.5.
    #[serde(default)]
    pub min_sharp_fraction: Option<f64>,
    /// Max results. Default 25, cap 100.
    #[serde(default)]
    pub limit: Option<usize>,
}

/// Run `broll_candidates` against the project resolved from
/// [`McpToolCtx`]. Returns the JSON body as `Ok(String)`; sidecar-read
/// failures return `Err(String)`.
pub fn run(args: BrollCandidatesArgs, ctx: McpToolCtx) -> Result<String, String> {
    let min_duration_s = args.min_duration_s.unwrap_or(1.5);
    let types: Vec<String> = args
        .types
        .unwrap_or_else(|| vec!["no-face".into(), "wide".into()]);
    let motions: Vec<String> = args
        .motions
        .unwrap_or_else(|| vec!["static".into(), "slow-pan".into()]);
    let min_sharp_fraction = args.min_sharp_fraction.unwrap_or(0.5);
    let limit = args.limit.unwrap_or(25).min(100);

    // Pre-load frame-quality sidecars by asset for the sharp-filter.
    let fq_walker = walk_indexer(&ctx.project_root, "frame-quality").map_err(|e| {
        format!(
            "broll_candidates: frame-quality sidecars not readable ({e}). \
             Run `awidat index --indexer frame-quality <project>` and retry."
        )
    })?;
    let fq_by_asset: std::collections::HashMap<String, serde_json::Value> = fq_walker.collect();

    let walker = walk_indexer(&ctx.project_root, "shot").map_err(|e| {
        format!(
            "broll_candidates: shot sidecars not readable ({e}). \
             Run `awidat index --indexer shot <project>` and retry."
        )
    })?;

    let mut results: Vec<serde_json::Value> = Vec::new();
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

        // Per-asset frame-quality lookup: a vector of (t_s, blur,
        // is_sharp) sorted by t_s. We compute fraction_sharp for
        // each shot by counting per-second entries inside its
        // window.
        let fq_per_frame = fq_by_asset
            .get(&asset_id)
            .and_then(|d| d.pointer("/data/per_frame"))
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        for shot in shots {
            let start_s = shot.get("start_s").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let end_s = shot.get("end_s").and_then(|v| v.as_f64()).unwrap_or(0.0);
            if end_s - start_s < min_duration_s {
                continue;
            }
            let stype = shot.get("type").and_then(|v| v.as_str()).unwrap_or("");
            if !types.iter().any(|t| t == stype) {
                continue;
            }
            let smotion = shot.get("motion").and_then(|v| v.as_str()).unwrap_or("");
            if !motions.iter().any(|m| m == smotion) {
                continue;
            }

            // Frame-quality gate (skipped when min_sharp_fraction == 0
            // OR no fq sidecar exists for this asset).
            let sharp_fraction = if min_sharp_fraction <= 0.0 || fq_per_frame.is_empty() {
                1.0
            } else {
                let mut sharp_in_window = 0u32;
                let mut total_in_window = 0u32;
                for entry in &fq_per_frame {
                    let t = entry.get("t_s").and_then(|v| v.as_f64()).unwrap_or(0.0);
                    if t < start_s || t >= end_s {
                        continue;
                    }
                    total_in_window += 1;
                    if entry
                        .get("is_sharp")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false)
                    {
                        sharp_in_window += 1;
                    }
                }
                if total_in_window == 0 {
                    // No frame-quality samples covered this shot;
                    // give it the benefit of the doubt rather than
                    // silently dropping every shot for short assets.
                    1.0
                } else {
                    sharp_in_window as f64 / total_in_window as f64
                }
            };
            if sharp_fraction < min_sharp_fraction {
                continue;
            }

            results.push(serde_json::json!({
                "asset_id": asset_id,
                "shot_index": shot.get("index"),
                "start_s": start_s,
                "end_s": end_s,
                "duration_s": end_s - start_s,
                "type": stype,
                "motion": smotion,
                "sharp_fraction": sharp_fraction,
            }));
        }
    }

    // Rank by duration desc — longer cutaways are usually more
    // useful; ties broken by sharper.
    results.sort_by(|a, b| {
        let da = a.get("duration_s").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let db = b.get("duration_s").and_then(|v| v.as_f64()).unwrap_or(0.0);
        db.partial_cmp(&da)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                let sa = a
                    .get("sharp_fraction")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0);
                let sb = b
                    .get("sharp_fraction")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0);
                sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal)
            })
    });
    let more = results.len() > limit;
    results.truncate(limit);

    Ok(serde_json::json!({
        "results": results,
        "more_available": more,
        "filters": {
            "min_duration_s": min_duration_s,
            "types": types,
            "motions": motions,
            "min_sharp_fraction": min_sharp_fraction,
        }
    })
    .to_string())
}

pub const DESCRIPTION: &str = "\
Find shots usable as B-roll: no main face on screen, steady camera, \
sharp frames. Reads `shot` (mandatory) and `frame-quality` (optional). \
Returns shots ranked by duration descending.\
\n\n\
Defaults filter to types ['no-face', 'wide'] + motions ['static', \
'slow-pan'] + sharp_fraction >= 0.5. Override per call: e.g. \
broll_candidates(types=['wide'], min_duration_s=3) for sustained \
wide cutaways only.\
\n\n\
Use this when the user asks for cutaways, B-roll, transition \
material, or 'something to cut to' — i.e. anytime you need a frame \
that isn't a talking head.\
";
