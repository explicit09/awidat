//! `broll_candidates` tool — find shots usable as B-roll cutaways.
//!
//! B-roll has two editorial constraints:
//!  1. No talking head visible (or at least not the main speaker) —
//!     shot type is `wide` or `no-face`.
//!  2. Steady or slow camera — shot motion is `static` or `slow-pan`,
//!     not `handheld` or `fast-cut`.
//!  3. Frames are sharp and well-exposed (frame-quality threshold).
//!
//! Reads the `shot` sidecar and (optionally) `frame-quality` sidecar
//! to score each shot. Returns the surviving shots ranked by usable
//! duration descending.

use async_trait::async_trait;
use awidat_index::walk_indexer;
use serde::Deserialize;

use crate::FunctionCallError;
use crate::anthropic::Tool as ToolSchema;
use crate::tool::{ToolContext, ToolHandler, ToolInvocation, ToolOutput};

/// The `broll_candidates` tool.
pub struct BrollCandidatesTool;

#[derive(Debug, Deserialize)]
struct Args {
    /// Restrict to one asset id; otherwise scan all shot sidecars.
    #[serde(default)]
    asset_id: Option<String>,
    /// Minimum shot duration in seconds. Default 1.5s — anything
    /// shorter rarely reads as B-roll on a cut.
    #[serde(default)]
    min_duration_s: Option<f64>,
    /// Allowed shot types. Default ["no-face", "wide"].
    #[serde(default)]
    types: Option<Vec<String>>,
    /// Allowed motion buckets. Default ["static", "slow-pan"].
    #[serde(default)]
    motions: Option<Vec<String>>,
    /// If set, require the shot's frame-quality fraction_sharp to be
    /// above this threshold. Default 0.5 (half the sampled frames in
    /// the shot must clear the blur floor). Pass 0.0 to skip.
    #[serde(default)]
    min_sharp_fraction: Option<f64>,
    /// Max results. Default 25, cap 100.
    #[serde(default)]
    limit: Option<usize>,
}

#[async_trait]
impl ToolHandler for BrollCandidatesTool {
    fn name(&self) -> &'static str {
        "broll_candidates"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "broll_candidates".into(),
            description: DESCRIPTION.into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "asset_id": {
                        "type": "string",
                        "description": "Restrict to one asset's shot sidecar."
                    },
                    "min_duration_s": {
                        "type": "number",
                        "minimum": 0.0,
                        "description": "Minimum shot length. Default 1.5s."
                    },
                    "types": {
                        "type": "array",
                        "items": {"type": "string", "enum": [
                            "extreme-close-up", "close-up", "medium",
                            "wide", "no-face"
                        ]},
                        "description": "Allowed shot types. Default ['no-face', 'wide']."
                    },
                    "motions": {
                        "type": "array",
                        "items": {"type": "string", "enum": [
                            "static", "slow-pan", "handheld", "fast-cut"
                        ]},
                        "description": "Allowed motions. Default ['static', 'slow-pan']."
                    },
                    "min_sharp_fraction": {
                        "type": "number",
                        "minimum": 0.0,
                        "maximum": 1.0,
                        "description": "Threshold on frame-quality fraction_sharp. Default 0.5."
                    },
                    "limit": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 100,
                        "description": "Max results. Default 25."
                    }
                }
            }),
            ..Default::default()
        }
    }

    fn is_mutating(&self, _invocation: &ToolInvocation) -> bool {
        false
    }

    async fn handle(
        &self,
        invocation: ToolInvocation,
        ctx: ToolContext,
    ) -> Result<ToolOutput, FunctionCallError> {
        let args: Args = serde_json::from_value(invocation.args).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "broll_candidates: invalid args ({e}). All fields optional."
            ))
        })?;
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
            FunctionCallError::RespondToModel(format!(
                "broll_candidates: frame-quality sidecars not readable ({e}). \
                 Run `awidat index --indexer frame-quality <project>` and retry."
            ))
        })?;
        let fq_by_asset: std::collections::HashMap<String, serde_json::Value> =
            fq_walker.collect();

        let walker = walk_indexer(&ctx.project_root, "shot").map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "broll_candidates: shot sidecars not readable ({e}). \
                 Run `awidat index --indexer shot <project>` and retry."
            ))
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
                let start_s = shot
                    .get("start_s")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0);
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
            db.partial_cmp(&da).unwrap_or(std::cmp::Ordering::Equal).then_with(|| {
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

        Ok(ToolOutput::text(
            serde_json::json!({
                "results": results,
                "more_available": more,
                "filters": {
                    "min_duration_s": min_duration_s,
                    "types": types,
                    "motions": motions,
                    "min_sharp_fraction": min_sharp_fraction,
                }
            })
            .to_string(),
        ))
    }
}

const DESCRIPTION: &str = "\
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

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::broadcast;

    fn ctx_at(root: &std::path::Path) -> ToolContext {
        let (tx, _) = broadcast::channel(8);
        ToolContext {
            project_root: root.to_path_buf(),
            events_tx: tx,
            user_input_tx: None,
            job_manager: awidat_render::JobManager::new(),
            approval_tx: None,
            mcp_host: crate::mcp_host::McpHost::new(awidat_mcp::ClientInfo { name: "test".into(), version: "0.0.0".into() }),
            skills: std::sync::Arc::new(crate::skills::SkillRegistry::default()),
            subagent_return: None,
        }
    }

    fn invoke(args: serde_json::Value) -> ToolInvocation {
        ToolInvocation {
            call_id: "c1".into(),
            name: "broll_candidates".into(),
            args,
        }
    }

    fn write_shot(
        root: &std::path::Path,
        asset: &str,
        shots: Vec<serde_json::Value>,
    ) {
        let p = root.join("index/shot").join(format!("{asset}.json"));
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(
            &p,
            serde_json::to_vec_pretty(&serde_json::json!({
                "indexer": "shot",
                "asset_id": asset,
                "data": {"shots": shots}
            }))
            .unwrap(),
        )
        .unwrap();
    }

    #[tokio::test]
    async fn defaults_keep_only_wide_or_noface_steady_shots() {
        let dir = tempfile::tempdir().unwrap();
        write_shot(
            dir.path(),
            "raw/ep.mp4",
            vec![
                // Should pass: wide + static, 5s.
                serde_json::json!({"index": 0, "start_s": 0.0, "end_s": 5.0, "type": "wide", "motion": "static"}),
                // Should drop: close-up.
                serde_json::json!({"index": 1, "start_s": 5.0, "end_s": 10.0, "type": "close-up", "motion": "static"}),
                // Should drop: handheld.
                serde_json::json!({"index": 2, "start_s": 10.0, "end_s": 15.0, "type": "wide", "motion": "handheld"}),
                // Should pass: no-face + slow-pan, 4s.
                serde_json::json!({"index": 3, "start_s": 15.0, "end_s": 19.0, "type": "no-face", "motion": "slow-pan"}),
            ],
        );
        // No frame-quality sidecar → sharp filter is bypassed.
        let out = BrollCandidatesTool
            .handle(
                invoke(serde_json::json!({"min_sharp_fraction": 0.0})),
                ctx_at(dir.path()),
            )
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&out.content).unwrap();
        let r = v["results"].as_array().unwrap();
        assert_eq!(r.len(), 2);
        // Sorted by duration desc → 5s shot first.
        assert_eq!(r[0]["shot_index"], 0);
        assert_eq!(r[1]["shot_index"], 3);
    }

    #[tokio::test]
    async fn min_duration_filters_short_shots() {
        let dir = tempfile::tempdir().unwrap();
        write_shot(
            dir.path(),
            "raw/ep.mp4",
            vec![
                serde_json::json!({"index": 0, "start_s": 0.0, "end_s": 1.0, "type": "wide", "motion": "static"}),
            ],
        );
        let out = BrollCandidatesTool
            .handle(
                invoke(serde_json::json!({"min_duration_s": 2.0, "min_sharp_fraction": 0.0})),
                ctx_at(dir.path()),
            )
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&out.content).unwrap();
        assert!(v["results"].as_array().unwrap().is_empty());
    }
}
