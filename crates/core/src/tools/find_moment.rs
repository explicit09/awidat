//! `find_moment` tool — search the index for moments matching a query.
//!
//! Per `PLAN.md` §6.1 + survey of SWE-agent's `find_file`:
//! **paths/ranges only, no embedded thumbnails.** The model calls
//! `view_frame` / `read_index` after `find_moment` exactly the way
//! SWE-agent calls `open` after `find_file`. This is a hard rule.
//!
//! Week-4 implementation: case-insensitive substring search across every
//! whisper transcript segment. v1.5 will swap in embedding similarity
//! (the `topic-mcp` Python indexer already produces sentence embeddings;
//! we'll join on those).

use async_trait::async_trait;
use awidat_index::{walk_indexer};
use serde::Deserialize;

use crate::FunctionCallError;
use crate::anthropic::Tool as ToolSchema;
use crate::tool::{ToolContext, ToolHandler, ToolInvocation, ToolOutput};

/// Hard cap on results returned. Codex's `list_dir` uses 25; matches.
const DEFAULT_LIMIT: usize = 25;

/// Per-result snippet length cap. SWE-agent §2.3: keep matches terse.
const SNIPPET_CAP: usize = 200;

/// The `find_moment` tool.
pub struct FindMomentTool;

#[derive(Debug, Deserialize)]
struct FindMomentArgs {
    query: String,
    /// Optional asset id filter; if set, only that asset is searched.
    #[serde(default)]
    asset_id: Option<String>,
    /// Max results (default 25, hard cap 100).
    #[serde(default)]
    limit: Option<usize>,
}

#[async_trait]
impl ToolHandler for FindMomentTool {
    fn name(&self) -> &'static str {
        "find_moment"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "find_moment".into(),
            description: DESCRIPTION.into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Substring (case-insensitive) to search for in transcript segments."
                    },
                    "asset_id": {
                        "type": "string",
                        "description": "Optional: restrict to one asset's transcript."
                    },
                    "limit": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 100,
                        "description": "Max results. Default 25, hard cap 100."
                    }
                },
                "required": ["query"]
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
        let args: FindMomentArgs = serde_json::from_value(invocation.args).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "find_moment: invalid args ({e}). Required: {{ \"query\": <str> }}."
            ))
        })?;
        let query = args.query.trim().to_lowercase();
        if query.is_empty() {
            return Err(FunctionCallError::RespondToModel(
                "find_moment: query is empty".into(),
            ));
        }
        let limit = args.limit.unwrap_or(DEFAULT_LIMIT).min(100);

        let mut hits = Vec::<serde_json::Value>::new();
        let mut more_available = false;

        let walker = walk_indexer(&ctx.project_root, "whisper").map_err(|e| {
            FunctionCallError::RespondToModel(e.to_string())
        })?;

        'outer: for (asset_id, sidecar) in walker {
            if let Some(filter) = &args.asset_id
                && filter != &asset_id
            {
                continue;
            }
            let segments = sidecar
                .pointer("/data/segments")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            for seg in segments {
                let text = seg.get("text").and_then(|v| v.as_str()).unwrap_or("");
                if text.to_lowercase().contains(&query) {
                    if hits.len() >= limit {
                        more_available = true;
                        break 'outer;
                    }
                    let snippet = if text.len() > SNIPPET_CAP {
                        format!("{}…", &text[..SNIPPET_CAP])
                    } else {
                        text.to_string()
                    };
                    hits.push(serde_json::json!({
                        "asset_id": asset_id,
                        "start_s": seg.get("start_s").or_else(|| seg.get("start")),
                        "end_s": seg.get("end_s").or_else(|| seg.get("end")),
                        "speaker_id": seg.get("speaker_id"),
                        "snippet": snippet,
                    }));
                }
            }
        }

        let body = serde_json::json!({
            "query": args.query,
            "results": hits,
            "more_available": more_available,
        });
        Ok(ToolOutput::text(body.to_string()))
    }
}

const DESCRIPTION: &str = "\
Search transcript segments for a substring (case-insensitive). Returns \
asset_id + start/end timestamps + speaker_id + a 200-char snippet per \
match. **Returns paths/ranges only, no audio or video.** Follow up with \
`view_frame` for visuals or `read_index` for fuller context. Default \
limit 25, hard cap 100.\
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
        }
    }

    fn invoke(args: serde_json::Value) -> ToolInvocation {
        ToolInvocation {
            call_id: "c1".into(),
            name: "find_moment".into(),
            args,
        }
    }

    fn write_sidecar(root: &std::path::Path, asset: &str, segments: Vec<serde_json::Value>) {
        let p = root.join("index/whisper").join(format!("{asset}.json"));
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(
            &p,
            serde_json::to_vec_pretty(&serde_json::json!({
                "indexer": "whisper", "asset_id": asset,
                "data": {"segments": segments}
            }))
            .unwrap(),
        )
        .unwrap();
    }

    #[tokio::test]
    async fn finds_substring_match() {
        let dir = tempfile::tempdir().unwrap();
        write_sidecar(
            dir.path(),
            "raw/ep.mp4",
            vec![
                serde_json::json!({"text": "and that's when she said the thing about Stripe", "start_s": 12.4, "end_s": 15.1, "speaker_id": "A"}),
                serde_json::json!({"text": "we went to the kitchen", "start_s": 20.0, "end_s": 22.0, "speaker_id": "B"}),
            ],
        );
        let out = FindMomentTool
            .handle(invoke(serde_json::json!({"query": "stripe"})), ctx_at(dir.path()))
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&out.content).unwrap();
        let results = v["results"].as_array().unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0]["asset_id"], "raw/ep.mp4");
        assert!(results[0]["snippet"].as_str().unwrap().contains("Stripe"));
        assert_eq!(results[0]["speaker_id"], "A");
    }

    #[tokio::test]
    async fn empty_query_is_respond_to_model() {
        let dir = tempfile::tempdir().unwrap();
        let err = FindMomentTool
            .handle(invoke(serde_json::json!({"query": "  "})), ctx_at(dir.path()))
            .await
            .unwrap_err();
        assert!(matches!(err, FunctionCallError::RespondToModel(msg) if msg.contains("empty")));
    }

    #[tokio::test]
    async fn limit_caps_results_and_marks_more_available() {
        let dir = tempfile::tempdir().unwrap();
        let segs: Vec<_> = (0..10)
            .map(|i| serde_json::json!({"text": format!("hello {i}"), "start_s": i, "end_s": i+1}))
            .collect();
        write_sidecar(dir.path(), "raw/ep.mp4", segs);
        let out = FindMomentTool
            .handle(
                invoke(serde_json::json!({"query": "hello", "limit": 3})),
                ctx_at(dir.path()),
            )
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&out.content).unwrap();
        assert_eq!(v["results"].as_array().unwrap().len(), 3);
        assert_eq!(v["more_available"], true);
    }

    #[tokio::test]
    async fn asset_id_filter_restricts() {
        let dir = tempfile::tempdir().unwrap();
        write_sidecar(dir.path(), "raw/a.mp4", vec![serde_json::json!({"text":"hello","start_s":0,"end_s":1})]);
        write_sidecar(dir.path(), "raw/b.mp4", vec![serde_json::json!({"text":"hello","start_s":0,"end_s":1})]);
        let out = FindMomentTool
            .handle(
                invoke(serde_json::json!({"query":"hello","asset_id":"raw/a.mp4"})),
                ctx_at(dir.path()),
            )
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&out.content).unwrap();
        let results = v["results"].as_array().unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0]["asset_id"], "raw/a.mp4");
    }

    #[tokio::test]
    async fn no_whisper_index_returns_empty_results() {
        let dir = tempfile::tempdir().unwrap();
        let out = FindMomentTool
            .handle(invoke(serde_json::json!({"query": "anything"})), ctx_at(dir.path()))
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&out.content).unwrap();
        assert!(v["results"].as_array().unwrap().is_empty());
        assert_eq!(v["more_available"], false);
    }
}
