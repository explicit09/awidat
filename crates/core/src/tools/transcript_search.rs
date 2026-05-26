//! First-class transcript search over whisper sidecars.

use async_trait::async_trait;
use awidat_index::walk_indexer;
use serde::{Deserialize, Serialize};

use crate::FunctionCallError;
use crate::tool_schema::Tool as ToolSchema;
use crate::tool::{ToolContext, ToolHandler, ToolInvocation, ToolOutput};

const DEFAULT_LIMIT: usize = 25;
const MAX_LIMIT: usize = 100;

/// Search whisper transcript sidecars.
pub struct TranscriptSearchTool;

#[derive(Debug, Deserialize)]
struct TranscriptSearchArgs {
    query: String,
    #[serde(default)]
    asset_id: Option<String>,
    #[serde(default)]
    speaker: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Debug, Serialize)]
struct TranscriptSearchResult {
    asset_id: String,
    start_s: Option<f64>,
    end_s: Option<f64>,
    speaker: Option<String>,
    score: usize,
    text: String,
}

#[async_trait]
impl ToolHandler for TranscriptSearchTool {
    fn name(&self) -> &'static str {
        "transcript_search"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "transcript_search".into(),
            description: "Search whisper transcript segments across project media, optionally filtering by asset or speaker.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["query"],
                "properties": {
                    "query": {"type": "string"},
                    "asset_id": {"type": "string"},
                    "speaker": {"type": "string"},
                    "limit": {"type": "integer", "minimum": 1, "maximum": MAX_LIMIT}
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
        let args: TranscriptSearchArgs = serde_json::from_value(invocation.args).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "transcript_search: invalid args ({e}). Expected query and optional asset_id/speaker/limit."
            ))
        })?;
        let query = args.query.trim().to_ascii_lowercase();
        if query.is_empty() {
            return Err(FunctionCallError::RespondToModel(
                "transcript_search: query must not be empty".into(),
            ));
        }
        let limit = args.limit.unwrap_or(DEFAULT_LIMIT).min(MAX_LIMIT);
        let speaker_filter = args
            .speaker
            .as_ref()
            .map(|speaker| speaker.to_ascii_lowercase());
        let mut results = Vec::new();
        let walker = walk_indexer(&ctx.project_root, "whisper")
            .map_err(|e| FunctionCallError::RespondToModel(e.to_string()))?;
        for (asset_id, sidecar) in walker {
            if args
                .asset_id
                .as_ref()
                .is_some_and(|filter| filter != &asset_id)
            {
                continue;
            }
            let Some(segments) = sidecar
                .pointer("/data/segments")
                .and_then(|value| value.as_array())
            else {
                continue;
            };
            for segment in segments {
                let text = segment
                    .get("text")
                    .and_then(|value| value.as_str())
                    .unwrap_or("");
                let haystack = text.to_ascii_lowercase();
                if !haystack.contains(&query) {
                    continue;
                }
                let speaker = segment
                    .get("speaker")
                    .or_else(|| segment.get("speaker_id"))
                    .and_then(|value| value.as_str())
                    .map(ToString::to_string);
                if let Some(filter) = &speaker_filter
                    && speaker
                        .as_ref()
                        .map(|speaker| speaker.to_ascii_lowercase())
                        .as_deref()
                        != Some(filter.as_str())
                {
                    continue;
                }
                results.push(TranscriptSearchResult {
                    asset_id: asset_id.clone(),
                    start_s: numeric_field(segment, &["start_s", "start"]),
                    end_s: numeric_field(segment, &["end_s", "end"]),
                    speaker,
                    score: haystack.matches(&query).count(),
                    text: truncate(text, 300),
                });
            }
        }
        results.sort_by(|a, b| {
            b.score
                .cmp(&a.score)
                .then_with(|| a.asset_id.cmp(&b.asset_id))
                .then_with(|| {
                    a.start_s
                        .partial_cmp(&b.start_s)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
        });
        let total = results.len();
        results.truncate(limit);
        Ok(ToolOutput::text(
            serde_json::json!({
                "query": args.query,
                "total_matches": total,
                "limit": limit,
                "results": results
            })
            .to_string(),
        ))
    }
}

fn numeric_field(segment: &serde_json::Value, names: &[&str]) -> Option<f64> {
    names
        .iter()
        .find_map(|name| segment.get(*name).and_then(|value| value.as_f64()))
}

fn truncate(text: &str, cap: usize) -> String {
    if text.len() <= cap {
        return text.to_string();
    }
    let mut end = cap;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

#[cfg(test)]
mod tests {
    use tokio::sync::broadcast;

    use super::*;
    use crate::tool::ToolInvocation;

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

    fn invocation(args: serde_json::Value) -> ToolInvocation {
        ToolInvocation {
            call_id: "c1".into(),
            name: "transcript_search".into(),
            args,
        }
    }

    fn write_sidecar(root: &std::path::Path, asset_id: &str, body: serde_json::Value) {
        let path = root
            .join("index")
            .join("whisper")
            .join(format!("{asset_id}.json"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, serde_json::to_vec(&body).unwrap()).unwrap();
    }

    #[test]
    fn truncate_preserves_short_text() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[tokio::test]
    async fn transcript_search_matches_query_and_filters_by_asset_and_speaker() {
        let dir = tempfile::tempdir().unwrap();
        write_sidecar(
            dir.path(),
            "raw/a.wav",
            serde_json::json!({
                "indexer": "whisper",
                "data": {
                    "segments": [
                        {"start_s": 1.0, "end_s": 2.0, "speaker": "S1", "text": "Clean quote for the intro"},
                        {"start_s": 3.0, "end_s": 4.0, "speaker": "S2", "text": "Another clean quote"}
                    ]
                }
            }),
        );
        write_sidecar(
            dir.path(),
            "raw/b.wav",
            serde_json::json!({
                "indexer": "whisper",
                "data": {
                    "segments": [
                        {"start": 5.0, "end": 6.0, "speaker_id": "S1", "text": "Clean alternate quote"}
                    ]
                }
            }),
        );

        let output = TranscriptSearchTool
            .handle(
                invocation(serde_json::json!({
                    "query": "clean",
                    "asset_id": "raw/a.wav",
                    "speaker": "s1",
                    "limit": 10
                })),
                ctx_at(dir.path()),
            )
            .await
            .unwrap();

        let body: serde_json::Value = serde_json::from_str(&output.content).unwrap();
        assert_eq!(body["total_matches"], 1);
        assert_eq!(body["results"][0]["asset_id"], "raw/a.wav");
        assert_eq!(body["results"][0]["speaker"], "S1");
        assert_eq!(body["results"][0]["start_s"], 1.0);
    }

    #[tokio::test]
    async fn transcript_search_caps_limit_and_reports_total_before_truncation() {
        let dir = tempfile::tempdir().unwrap();
        write_sidecar(
            dir.path(),
            "raw/a.wav",
            serde_json::json!({
                "indexer": "whisper",
                "data": {
                    "segments": [
                        {"start_s": 1.0, "end_s": 2.0, "text": "needle one"},
                        {"start_s": 2.0, "end_s": 3.0, "text": "needle two"}
                    ]
                }
            }),
        );

        let output = TranscriptSearchTool
            .handle(
                invocation(serde_json::json!({"query": "needle", "limit": 1})),
                ctx_at(dir.path()),
            )
            .await
            .unwrap();

        let body: serde_json::Value = serde_json::from_str(&output.content).unwrap();
        assert_eq!(body["total_matches"], 2);
        assert_eq!(body["results"].as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn transcript_search_rejects_empty_query() {
        let dir = tempfile::tempdir().unwrap();

        let err = TranscriptSearchTool
            .handle(
                invocation(serde_json::json!({"query": "  "})),
                ctx_at(dir.path()),
            )
            .await
            .unwrap_err();

        match err {
            FunctionCallError::RespondToModel(message) => {
                assert!(
                    message.contains("query must not be empty"),
                    "got: {message}"
                );
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }
}
