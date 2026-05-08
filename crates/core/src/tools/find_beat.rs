//! `find_beat` tool — query the editorial-moments sidecar by kind /
//! speaker / score.
//!
//! Where `find_moment` is grep over the transcript, `find_beat` is
//! filter-and-sort over the *typed editorial decisions* the
//! editorial-moments indexer produced. The agent stops asking
//! "where is the funny part" and starts asking "find every punchline
//! above 0.7" or "find every CTA from speaker A."
//!
//! Read-only, deterministic, cheap. The Cody-on-SCIP pattern: a
//! precise structural index, not embedding similarity.

use async_trait::async_trait;
use awidat_index::walk_indexer;
use serde::Deserialize;

use crate::FunctionCallError;
use crate::anthropic::Tool as ToolSchema;
use crate::tool::{ToolContext, ToolHandler, ToolInvocation, ToolOutput};

/// The `find_beat` tool.
pub struct FindBeatTool;

#[derive(Debug, Deserialize)]
struct FindBeatArgs {
    /// Filter by moment kind. Omit to return every kind.
    #[serde(default)]
    kind: Option<String>,
    /// Filter by speaker (whisper speaker_id). Omit for any.
    #[serde(default)]
    speaker: Option<String>,
    /// Minimum editorial score. Default 0.5.
    #[serde(default)]
    min_score: Option<f64>,
    /// Filter to one asset.
    #[serde(default)]
    asset_id: Option<String>,
    /// Max results. Default 10, hard cap 50.
    #[serde(default)]
    limit: Option<usize>,
}

#[async_trait]
impl ToolHandler for FindBeatTool {
    fn name(&self) -> &'static str {
        "find_beat"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "find_beat".into(),
            description: DESCRIPTION.into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "kind": {
                        "type": "string",
                        "enum": ["hook", "story", "punchline", "setup", "question",
                                 "answer", "cta", "emotional_peak", "dead_air",
                                 "tangent", "explanation"],
                        "description": "Filter to one beat kind. Omit for any."
                    },
                    "speaker": {
                        "type": "string",
                        "description": "Filter by whisper speaker_id (when diarization ran)."
                    },
                    "min_score": {
                        "type": "number",
                        "minimum": 0.0,
                        "maximum": 1.0,
                        "description": "Editorial-importance floor. Default 0.5."
                    },
                    "asset_id": {
                        "type": "string",
                        "description": "Restrict to one asset's moments."
                    },
                    "limit": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 50,
                        "description": "Max results. Default 10."
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
        let args: FindBeatArgs = serde_json::from_value(invocation.args).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "find_beat: invalid args ({e}). All fields optional."
            ))
        })?;
        let kind_filter = args.kind.as_deref().map(str::to_lowercase);
        let speaker_filter = args.speaker.as_deref();
        let min_score = args.min_score.unwrap_or(0.5);
        let limit = args.limit.unwrap_or(10).min(50);

        let walker = walk_indexer(&ctx.project_root, "editorial-moments").map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "find_beat: editorial-moments sidecars not readable ({e}). \
                 Run `awidat index --indexer editorial-moments <project>` and retry."
            ))
        })?;

        let mut all_hits = Vec::<serde_json::Value>::new();
        let mut more = false;
        'outer: for (asset_id, sidecar) in walker {
            if let Some(filter) = &args.asset_id
                && filter != &asset_id
            {
                continue;
            }
            let moments = sidecar
                .pointer("/data/moments")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            for m in moments {
                if let Some(k) = &kind_filter
                    && m.get("kind").and_then(|x| x.as_str()) != Some(k.as_str())
                {
                    continue;
                }
                if let Some(s) = speaker_filter
                    && m.get("speaker").and_then(|x| x.as_str()) != Some(s)
                {
                    continue;
                }
                let score = m.get("score").and_then(|x| x.as_f64()).unwrap_or(0.0);
                if score < min_score {
                    continue;
                }
                if all_hits.len() >= limit {
                    more = true;
                    break 'outer;
                }
                let mut row = serde_json::json!({
                    "asset_id": asset_id,
                    "moment_id": m.get("moment_id"),
                    "kind": m.get("kind"),
                    "start_s": m.get("start_s"),
                    "end_s": m.get("end_s"),
                    "score": score,
                    "speaker": m.get("speaker"),
                    "energy": m.get("energy"),
                    "broll_need": m.get("broll_need"),
                    "dependencies": m.get("dependencies"),
                    "note": m.get("note"),
                });
                // cut_in_suggestion is a free-form string the agent
                // uses as an anchor candidate; surface it.
                if let Some(c) = m.get("cut_in_suggestion") {
                    row["cut_in_suggestion"] = c.clone();
                }
                all_hits.push(row);
            }
        }
        // Sort highest-score first so the model sees the most
        // editorially-important beats at the top of the list.
        all_hits.sort_by(|a, b| {
            b.get("score")
                .and_then(|x| x.as_f64())
                .unwrap_or(0.0)
                .partial_cmp(&a.get("score").and_then(|x| x.as_f64()).unwrap_or(0.0))
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let body = serde_json::json!({
            "results": all_hits,
            "more_available": more,
        });
        Ok(ToolOutput::text(body.to_string()))
    }
}

const DESCRIPTION: &str = "\
Query the editorial-moments index by beat kind, speaker, and minimum \
editorial score. Returns beats sorted by score (highest first) with \
moment_id, time range, kind, speaker, energy, b-roll need, \
dependencies, and the model's note for why each is editorially \
interesting.\
\n\n\
Examples:\
\n  find_beat(kind='hook', min_score=0.7) → strongest opening beats\
\n  find_beat(kind='punchline')           → every punchline\
\n  find_beat(speaker='host')             → beats from one speaker\
\n  find_beat(min_score=0.8)              → top editorial decisions\
\n\n\
Kinds: hook, story, punchline, setup, question, answer, cta, \
emotional_peak, dead_air, tangent, explanation.\
\n\n\
The editorial-moments index is produced by `awidat index --indexer \
editorial-moments`; if find_beat returns empty, the index hasn't run \
yet. Each beat's `dependencies` field tells you which other beats it \
needs as setup — keep those when cutting standalone.\
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
            name: "find_beat".into(),
            args,
        }
    }

    fn write_sidecar(root: &std::path::Path, asset: &str, moments: Vec<serde_json::Value>) {
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

    #[tokio::test]
    async fn no_index_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        let out = FindBeatTool
            .handle(invoke(serde_json::json!({})), ctx_at(dir.path()))
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&out.content).unwrap();
        assert!(v["results"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn filters_by_kind_and_score() {
        let dir = tempfile::tempdir().unwrap();
        write_sidecar(
            dir.path(),
            "raw/ep.mp4",
            vec![
                serde_json::json!({
                    "moment_id": "m_0_1", "kind": "hook", "start_s": 0.0, "end_s": 5.0,
                    "score": 0.9, "speaker": null, "energy": "high", "broll_need": "high",
                    "dependencies": [], "note": "strong open"
                }),
                serde_json::json!({
                    "moment_id": "m_0_2", "kind": "tangent", "start_s": 5.0, "end_s": 10.0,
                    "score": 0.3, "speaker": null, "energy": "low", "broll_need": "none",
                    "dependencies": [], "note": "off-topic"
                }),
                serde_json::json!({
                    "moment_id": "m_0_3", "kind": "hook", "start_s": 10.0, "end_s": 15.0,
                    "score": 0.55, "speaker": null, "energy": "medium", "broll_need": "medium",
                    "dependencies": [], "note": "secondary hook"
                }),
            ],
        );
        // Only the score-0.9 hook should pass kind=hook + min_score=0.7.
        let out = FindBeatTool
            .handle(
                invoke(serde_json::json!({"kind": "hook", "min_score": 0.7})),
                ctx_at(dir.path()),
            )
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&out.content).unwrap();
        let results = v["results"].as_array().unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0]["moment_id"], "m_0_1");
    }

    #[tokio::test]
    async fn results_sorted_by_score_desc() {
        let dir = tempfile::tempdir().unwrap();
        write_sidecar(
            dir.path(),
            "raw/ep.mp4",
            vec![
                serde_json::json!({
                    "moment_id": "low",  "kind": "hook", "start_s": 0.0, "end_s": 5.0,
                    "score": 0.55, "energy": "medium", "broll_need": "none", "dependencies": []
                }),
                serde_json::json!({
                    "moment_id": "high", "kind": "hook", "start_s": 5.0, "end_s": 10.0,
                    "score": 0.95, "energy": "high", "broll_need": "high", "dependencies": []
                }),
            ],
        );
        let out = FindBeatTool
            .handle(invoke(serde_json::json!({})), ctx_at(dir.path()))
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&out.content).unwrap();
        let results = v["results"].as_array().unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(
            results[0]["moment_id"], "high",
            "highest score must come first"
        );
    }
}
