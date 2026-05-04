//! `inspect_moment` tool — drill into one editorial beat.
//!
//! Where `find_beat` returns a list of candidates, `inspect_moment`
//! returns *one* beat fully expanded: the full record, ±10s of
//! transcript context around it, and any dependencies inlined.
//!
//! Read-only. The agent's typical flow:
//!   1. find_beat(kind='hook') → list of candidate hooks
//!   2. inspect_moment('m_0_1') → enough detail to decide whether to
//!      use it
//!   3. apply_edl(...) to commit the cut

use async_trait::async_trait;
use awidat_index::walk_indexer;
use serde::Deserialize;

use crate::FunctionCallError;
use crate::anthropic::Tool as ToolSchema;
use crate::tool::{ToolContext, ToolHandler, ToolInvocation, ToolOutput};

pub struct InspectMomentTool;

#[derive(Debug, Deserialize)]
struct InspectMomentArgs {
    moment_id: String,
    /// Surrounding transcript context (seconds). Default 10.0.
    #[serde(default)]
    context_s: Option<f64>,
}

#[async_trait]
impl ToolHandler for InspectMomentTool {
    fn name(&self) -> &'static str {
        "inspect_moment"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "inspect_moment".into(),
            description: DESCRIPTION.into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "moment_id": {
                        "type": "string",
                        "description": "The moment_id from find_beat."
                    },
                    "context_s": {
                        "type": "number",
                        "minimum": 0.0,
                        "description": "Seconds of surrounding transcript to include. Default 10."
                    }
                },
                "required": ["moment_id"]
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
        let args: InspectMomentArgs = serde_json::from_value(invocation.args).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "inspect_moment: invalid args ({e}). Required: {{ \"moment_id\": <str> }}."
            ))
        })?;
        let context_s = args.context_s.unwrap_or(10.0).max(0.0);

        // Walk every editorial-moments sidecar; first match wins.
        let walker = walk_indexer(&ctx.project_root, "editorial-moments").map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "inspect_moment: editorial-moments sidecars not readable ({e}). \
                 Run `awidat index --indexer editorial-moments <project>` and retry."
            ))
        })?;
        let mut hit: Option<(String, serde_json::Value, Vec<serde_json::Value>)> = None;
        for (asset_id, sidecar) in walker {
            let moments = sidecar
                .pointer("/data/moments")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            if let Some(found) = moments
                .iter()
                .find(|m| m.get("moment_id").and_then(|x| x.as_str()) == Some(args.moment_id.as_str()))
            {
                hit = Some((asset_id, found.clone(), moments));
                break;
            }
        }
        let Some((asset_id, moment, all_moments)) = hit else {
            return Err(FunctionCallError::RespondToModel(format!(
                "inspect_moment: moment_id {:?} not found in any editorial-moments sidecar. \
                 Run find_beat first to get valid moment_ids.",
                args.moment_id
            )));
        };

        let start_s = moment.get("start_s").and_then(|x| x.as_f64()).unwrap_or(0.0);
        let end_s = moment.get("end_s").and_then(|x| x.as_f64()).unwrap_or(0.0);

        // Pull surrounding transcript from the whisper sidecar for
        // this asset.
        let transcript_window = transcript_around(
            &ctx.project_root,
            &asset_id,
            (start_s - context_s).max(0.0),
            end_s + context_s,
        );

        // Inline any dependency moments (just their summaries).
        let deps: Vec<serde_json::Value> = moment
            .get("dependencies")
            .and_then(|x| x.as_array())
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|dep_id| {
                dep_id.as_str().and_then(|did| {
                    all_moments.iter().find(|m| {
                        m.get("moment_id").and_then(|x| x.as_str()) == Some(did)
                    })
                }).cloned()
            })
            .collect();

        let body = serde_json::json!({
            "asset_id": asset_id,
            "moment": moment,
            "context_s": context_s,
            "transcript_window": transcript_window,
            "dependencies_expanded": deps,
        });
        Ok(ToolOutput::text(body.to_string()))
    }
}

/// Pull transcript segments inside `[lo, hi]` from the asset's
/// whisper sidecar. Best-effort; returns empty if the sidecar isn't
/// present.
fn transcript_around(
    project_root: &std::path::Path,
    asset_id: &str,
    lo: f64,
    hi: f64,
) -> Vec<serde_json::Value> {
    let path = project_root
        .join("index")
        .join("whisper")
        .join(format!("{asset_id}.json"));
    let Ok(bytes) = std::fs::read(&path) else {
        return Vec::new();
    };
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return Vec::new();
    };
    let Some(segs) = v.pointer("/data/segments").and_then(|s| s.as_array()) else {
        return Vec::new();
    };
    segs.iter()
        .filter(|s| {
            let start = s.get("start_s").and_then(|x| x.as_f64()).unwrap_or(0.0);
            let end = s.get("end_s").and_then(|x| x.as_f64()).unwrap_or(0.0);
            // Overlap with [lo, hi].
            end >= lo && start <= hi
        })
        .cloned()
        .collect()
}

const DESCRIPTION: &str = "\
Drill into one editorial beat from the editorial-moments index. \
Returns: the full moment record, ±N seconds of surrounding transcript \
(default 10s), and any `dependencies` moments expanded inline so you \
can read their notes without a second tool call.\
\n\n\
Use after find_beat narrows to a candidate. The transcript window \
gives you the actual phrases to anchor against in apply_edl. The \
dependencies tell you what setup must stay intact when cutting this \
beat standalone.\
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
            name: "inspect_moment".into(),
            args,
        }
    }

    fn write_em_sidecar(root: &std::path::Path, asset: &str, moments: Vec<serde_json::Value>) {
        let p = root
            .join("index/editorial-moments")
            .join(format!("{asset}.json"));
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(
            &p,
            serde_json::to_vec_pretty(&serde_json::json!({
                "indexer": "editorial-moments",
                "asset_id": asset,
                "data": {"moments": moments}
            }))
            .unwrap(),
        )
        .unwrap();
    }

    fn write_whisper_sidecar(root: &std::path::Path, asset: &str, segments: Vec<serde_json::Value>) {
        let p = root.join("index/whisper").join(format!("{asset}.json"));
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(
            &p,
            serde_json::to_vec_pretty(&serde_json::json!({
                "indexer": "whisper",
                "asset_id": asset,
                "data": {"segments": segments}
            }))
            .unwrap(),
        )
        .unwrap();
    }

    #[tokio::test]
    async fn missing_id_is_respond_to_model() {
        let dir = tempfile::tempdir().unwrap();
        write_em_sidecar(dir.path(), "raw/ep.mp4", vec![]);
        let err = InspectMomentTool
            .handle(
                invoke(serde_json::json!({"moment_id": "missing"})),
                ctx_at(dir.path()),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, FunctionCallError::RespondToModel(msg) if msg.contains("not found")));
    }

    #[tokio::test]
    async fn returns_moment_plus_transcript_window() {
        let dir = tempfile::tempdir().unwrap();
        write_em_sidecar(
            dir.path(),
            "raw/ep.mp4",
            vec![serde_json::json!({
                "moment_id": "m_0_1", "kind": "hook",
                "start_s": 5.0, "end_s": 10.0, "score": 0.9,
                "energy": "high", "broll_need": "high", "dependencies": []
            })],
        );
        write_whisper_sidecar(
            dir.path(),
            "raw/ep.mp4",
            vec![
                serde_json::json!({"text": "off-topic chat", "start_s": 0.0, "end_s": 3.0}),
                serde_json::json!({"text": "the hook starts", "start_s": 5.0, "end_s": 8.0}),
                serde_json::json!({"text": "and ends here", "start_s": 8.0, "end_s": 10.0}),
                serde_json::json!({"text": "post-hook stuff", "start_s": 10.5, "end_s": 13.0}),
            ],
        );
        let out = InspectMomentTool
            .handle(
                invoke(serde_json::json!({"moment_id": "m_0_1", "context_s": 5.0})),
                ctx_at(dir.path()),
            )
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&out.content).unwrap();
        assert_eq!(v["moment"]["moment_id"], "m_0_1");
        let window = v["transcript_window"].as_array().unwrap();
        // [0..15] context window picks up all 4 segments.
        assert_eq!(window.len(), 4);
    }

    #[tokio::test]
    async fn dependencies_expanded_inline() {
        let dir = tempfile::tempdir().unwrap();
        write_em_sidecar(
            dir.path(),
            "raw/ep.mp4",
            vec![
                serde_json::json!({
                    "moment_id": "setup", "kind": "setup",
                    "start_s": 0.0, "end_s": 5.0, "score": 0.8,
                    "energy": "medium", "broll_need": "none", "dependencies": [],
                    "note": "the setup"
                }),
                serde_json::json!({
                    "moment_id": "punchline", "kind": "punchline",
                    "start_s": 6.0, "end_s": 10.0, "score": 0.95,
                    "energy": "high", "broll_need": "none",
                    "dependencies": ["setup"], "note": "the payoff"
                }),
            ],
        );
        let out = InspectMomentTool
            .handle(
                invoke(serde_json::json!({"moment_id": "punchline"})),
                ctx_at(dir.path()),
            )
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&out.content).unwrap();
        let deps = v["dependencies_expanded"].as_array().unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0]["moment_id"], "setup");
        assert_eq!(deps[0]["note"], "the setup");
    }
}
