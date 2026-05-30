//! `shot_summary` tool — compact roll-up of an asset's visual structure.
//!
//! Reads `scenedetect` (for raw shot count + average length) and `shot`
//! (for type + motion histograms). Output is one paragraph the agent
//! can quote into a plan: "this episode has 14 shots over 30 minutes,
//! 70% medium close-ups + 20% wide cutaways, mostly static camera."
//!
//! No filtering, no editorial decisions — just the descriptive
//! statistics that let the agent decide whether broll_candidates is
//! worth calling, whether the camera moves enough to need
//! stabilization, etc.

use async_trait::async_trait;
use awidat_index::walk_indexer;
use serde::Deserialize;

use crate::FunctionCallError;
use crate::tool::{ToolContext, ToolHandler, ToolInvocation, ToolOutput};
use crate::tool_schema::Tool as ToolSchema;

/// The `shot_summary` tool.
pub struct ShotSummaryTool;

#[derive(Debug, Deserialize)]
struct Args {
    /// Restrict to one asset id; otherwise summarize each asset.
    #[serde(default)]
    asset_id: Option<String>,
}

#[async_trait]
impl ToolHandler for ShotSummaryTool {
    fn name(&self) -> &'static str {
        "shot_summary"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "shot_summary".into(),
            description: DESCRIPTION.into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "asset_id": {
                        "type": "string",
                        "description": "Restrict to one asset's summary."
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
            FunctionCallError::RespondToModel(format!("shot_summary: invalid args ({e})."))
        })?;

        let walker = walk_indexer(&ctx.project_root, "shot").map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "shot_summary: shot sidecars not readable ({e}). \
                 Run `awidat index --indexer shot <project>` and retry."
            ))
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

        Ok(ToolOutput::text(
            serde_json::json!({
                "summaries": summaries,
            })
            .to_string(),
        ))
    }
}

const DESCRIPTION: &str = "\
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
            sandbox_mode: crate::tool::SandboxMode::Default,
            mcp_host: crate::mcp_host::McpHost::new(awidat_mcp::ClientInfo {
                name: "test".into(),
                version: "0.0.0".into(),
            }),
            skills: std::sync::Arc::new(crate::skills::SkillRegistry::default()),
            subagent_return: None,
        }
    }

    fn invoke(args: serde_json::Value) -> ToolInvocation {
        ToolInvocation {
            call_id: "c1".into(),
            name: "shot_summary".into(),
            args,
        }
    }

    fn write_shot(root: &std::path::Path, asset: &str, shots: Vec<serde_json::Value>) {
        let p = root.join("index/shot").join(format!("{asset}.json"));
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(
            &p,
            serde_json::to_vec_pretty(&serde_json::json!({
                "indexer": "shot", "asset_id": asset,
                "data": {"shots": shots}
            }))
            .unwrap(),
        )
        .unwrap();
    }

    #[tokio::test]
    async fn produces_per_asset_histograms() {
        let dir = tempfile::tempdir().unwrap();
        write_shot(
            dir.path(),
            "raw/ep.mp4",
            vec![
                serde_json::json!({"start_s": 0.0,  "end_s": 10.0, "type": "wide",     "motion": "static"}),
                serde_json::json!({"start_s": 10.0, "end_s": 15.0, "type": "close-up", "motion": "static"}),
                serde_json::json!({"start_s": 15.0, "end_s": 20.0, "type": "close-up", "motion": "slow-pan"}),
            ],
        );
        let out = ShotSummaryTool
            .handle(invoke(serde_json::json!({})), ctx_at(dir.path()))
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&out.content).unwrap();
        let s = &v["summaries"][0];
        assert_eq!(s["shot_count"], 3);
        assert_eq!(s["total_shot_seconds"], 20.0);
        assert!((s["mean_shot_seconds"].as_f64().unwrap() - 20.0 / 3.0).abs() < 1e-9);
        assert_eq!(s["by_type"]["close-up"], 2);
        assert_eq!(s["by_type"]["wide"], 1);
        assert_eq!(s["by_motion"]["static"], 2);
    }
}
