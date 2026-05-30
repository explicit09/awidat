//! `find_moment` tool — search the index for moments matching a query.
//!
//! Per `PLAN.md` §6.1 + survey of SWE-agent's `find_file`:
//! **paths/ranges only, no embedded thumbnails.** The model calls
//! `view_frame` / `read_index` after `find_moment` exactly the way
//! SWE-agent calls `open` after `find_file`. This is a hard rule.
//!
//! Implementation (#159): BM25 ranking over whisper transcript
//! segments, ported from codex's `tool_search.rs:8-11` use of the
//! `bm25` crate. Strict superset of the prior case-insensitive
//! substring path — substring queries still match (BM25 ranks them
//! highest), and semantically-related queries that don't share a
//! substring now find their target ("battery problems" → "Note 7
//! battery exploded"). Results are ordered by score (best first).
//! Continue's `FullTextSearchCodebaseIndex.ts` validated the same
//! pattern.

use async_trait::async_trait;
use awidat_index::walk_indexer;
use bm25::{Document, Language, SearchEngineBuilder};
use serde::Deserialize;

use crate::FunctionCallError;
use crate::tool::{ToolContext, ToolHandler, ToolInvocation, ToolOutput};
use crate::tool_schema::Tool as ToolSchema;

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
        find_moment_schema(DESCRIPTION)
    }

    fn schema_for_family(&self, family: crate::tool::ModelFamily) -> ToolSchema {
        // Haiku is our compaction model + has the smallest context;
        // a terser description leaves more room for actual query +
        // result tokens. Sonnet/Opus get the full version. Per #154
        // (cline `variants/<model>/overrides.ts` pattern): only the
        // description text varies, not the args schema.
        match family {
            crate::tool::ModelFamily::Haiku => find_moment_schema(DESCRIPTION_HAIKU),
            _ => find_moment_schema(DESCRIPTION),
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
        let query = args.query.trim();
        if query.is_empty() {
            return Err(FunctionCallError::RespondToModel(
                "find_moment: `query` was empty (or just whitespace). Pass a \
                 non-empty query to search transcript text for. \
                 Example: find_moment(query=\"battery fire\")."
                    .into(),
            ));
        }
        let limit = args.limit.unwrap_or(DEFAULT_LIMIT).min(100);

        // Walk the index once, collecting (asset, segment-fields) pairs
        // into a flat document corpus. BM25's index is fast to build at
        // this scale (real-video runs hit ~5K segments per hour); we
        // rebuild per-call rather than caching, since whisper sidecars
        // change often during a session and a stale cache would surface
        // out-of-date hits. Codex `tool_search.rs:28-37` mirrors this
        // build-per-call pattern.
        let walker = walk_indexer(&ctx.project_root, "whisper")
            .map_err(|e| FunctionCallError::RespondToModel(e.to_string()))?;

        let mut docs: Vec<SegmentDoc> = Vec::new();
        for (asset_id, sidecar) in walker {
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
                if text.is_empty() {
                    continue;
                }
                docs.push(SegmentDoc {
                    asset_id: asset_id.clone(),
                    start_s: seg.get("start_s").or_else(|| seg.get("start")).cloned(),
                    end_s: seg.get("end_s").or_else(|| seg.get("end")).cloned(),
                    speaker_id: seg.get("speaker_id").cloned(),
                    text: text.to_string(),
                });
            }
        }

        if docs.is_empty() {
            let body = serde_json::json!({
                "query": args.query,
                "results": [],
                "more_available": false,
            });
            return Ok(ToolOutput::text(body.to_string()));
        }

        let bm25_docs: Vec<Document<usize>> = docs
            .iter()
            .enumerate()
            .map(|(idx, d)| Document::new(idx, d.text.clone()))
            .collect();
        let engine =
            SearchEngineBuilder::<usize>::with_documents(Language::English, bm25_docs).build();

        // Ask BM25 for `limit + 1` so we can detect more_available.
        let raw = engine.search(query, limit + 1);
        let more_available = raw.len() > limit;
        let scored: Vec<_> = raw.into_iter().take(limit).collect();

        let hits: Vec<serde_json::Value> = scored
            .into_iter()
            .map(|res| {
                let d = &docs[res.document.id];
                let snippet = if d.text.len() > SNIPPET_CAP {
                    format!("{}…", &d.text[..SNIPPET_CAP])
                } else {
                    d.text.clone()
                };
                serde_json::json!({
                    "asset_id": d.asset_id,
                    "start_s": d.start_s,
                    "end_s": d.end_s,
                    "speaker_id": d.speaker_id,
                    "snippet": snippet,
                    "score": (res.score * 1000.0).round() / 1000.0,
                })
            })
            .collect();

        let body = serde_json::json!({
            "query": args.query,
            "results": hits,
            "more_available": more_available,
        });
        Ok(ToolOutput::text(body.to_string()))
    }
}

struct SegmentDoc {
    asset_id: String,
    start_s: Option<serde_json::Value>,
    end_s: Option<serde_json::Value>,
    speaker_id: Option<serde_json::Value>,
    text: String,
}

const DESCRIPTION: &str = "\
BM25-ranked search across whisper transcript segments. Tokenizes the \
query and ranks every segment by relevance — exact-substring queries \
still match (they rank highest), and semantically-related queries \
that share no substring (e.g. `battery problems` → `Note 7 battery \
exploded`) now find their target. Returns asset_id + start/end \
timestamps + speaker_id + a 200-char snippet + a relevance score per \
match. **Returns paths/ranges only, no audio or video.** Follow up \
with `view_frame` for visuals or `read_index` for fuller context. \
Default limit 25, hard cap 100. Results are ordered by score (best \
first).\
\n\nNote: English stopwords (`the`, `and`, `is`, …) are filtered out \
of the query before ranking. A query of just stopwords returns 0 \
hits — use content words (`battery`, `kitchen`, `Samsung`).\
";

/// Terser variant for Haiku per #154. Same args schema, different
/// description. Haiku tolerates fewer tokens of preamble + benefits
/// from a tight rule statement. The stopword note is kept (one-line
/// version) — this edge case is worth naming despite the extra tokens.
const DESCRIPTION_HAIKU: &str = "\
BM25 search of whisper transcript segments. Returns asset_id, \
start/end timestamps, speaker_id, snippet, score. Paths/ranges only \
— no audio or video. Default 25 results, max 100, ordered by score. \
English stopwords (`the`, `and`, …) are filtered — use content words.\
";

fn find_moment_schema(description: &str) -> ToolSchema {
    ToolSchema {
        name: "find_moment".into(),
        description: description.into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search query for transcript segments."
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
            .handle(
                invoke(serde_json::json!({"query": "stripe"})),
                ctx_at(dir.path()),
            )
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
            .handle(
                invoke(serde_json::json!({"query": "  "})),
                ctx_at(dir.path()),
            )
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
        write_sidecar(
            dir.path(),
            "raw/a.mp4",
            vec![serde_json::json!({"text":"hello","start_s":0,"end_s":1})],
        );
        write_sidecar(
            dir.path(),
            "raw/b.mp4",
            vec![serde_json::json!({"text":"hello","start_s":0,"end_s":1})],
        );
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
            .handle(
                invoke(serde_json::json!({"query": "anything"})),
                ctx_at(dir.path()),
            )
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&out.content).unwrap();
        assert!(v["results"].as_array().unwrap().is_empty());
        assert_eq!(v["more_available"], false);
    }

    /// Regression: BM25 returns hits for queries whose tokens overlap
    /// the corpus even if no contiguous substring match exists. The
    /// old substring impl returned nothing for these — that's the
    /// upgrade #159 ships.
    #[tokio::test]
    async fn bm25_matches_token_overlap_without_exact_substring() {
        let dir = tempfile::tempdir().unwrap();
        write_sidecar(
            dir.path(),
            "raw/ep.mp4",
            vec![
                serde_json::json!({"text": "Samsung's Note 7 battery exploded during charging", "start_s": 10.0, "end_s": 14.0}),
                serde_json::json!({"text": "we went to the kitchen for tea", "start_s": 20.0, "end_s": 22.0}),
            ],
        );
        // Query has no contiguous substring in either segment, but
        // shares the token "battery" with the first.
        let out = FindMomentTool
            .handle(
                invoke(serde_json::json!({"query": "battery problems"})),
                ctx_at(dir.path()),
            )
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&out.content).unwrap();
        let results = v["results"].as_array().unwrap();
        assert!(!results.is_empty(), "BM25 must score the battery segment");
        assert_eq!(results[0]["asset_id"], "raw/ep.mp4");
        assert!(results[0]["snippet"].as_str().unwrap().contains("battery"));
    }

    /// #154: Haiku gets a terser description; Opus/Sonnet get the
    /// canonical one. The args schema is unchanged across families
    /// (the contract from cline's variants pattern).
    #[test]
    fn schema_for_family_picks_haiku_variant() {
        let opus = FindMomentTool.schema_for_family(crate::tool::ModelFamily::Opus);
        let sonnet = FindMomentTool.schema_for_family(crate::tool::ModelFamily::Sonnet);
        let haiku = FindMomentTool.schema_for_family(crate::tool::ModelFamily::Haiku);

        // Opus/Sonnet share the canonical description; Haiku differs.
        assert_eq!(opus.description, sonnet.description);
        assert_ne!(haiku.description, opus.description);
        // Haiku's variant must be shorter (the whole point).
        assert!(haiku.description.len() < opus.description.len());
        // Args schema invariant — same shape across all families.
        assert_eq!(opus.input_schema, sonnet.input_schema);
        assert_eq!(opus.input_schema, haiku.input_schema);
    }

    /// Results carry a numeric `score` field. Used downstream by the
    /// model to decide which hits to inspect first.
    #[tokio::test]
    async fn results_carry_score_field() {
        let dir = tempfile::tempdir().unwrap();
        write_sidecar(
            dir.path(),
            "raw/ep.mp4",
            vec![
                serde_json::json!({"text": "battery exploded", "start_s": 0, "end_s": 1}),
                serde_json::json!({"text": "the battery problem with the device", "start_s": 1, "end_s": 2}),
            ],
        );
        let out = FindMomentTool
            .handle(
                invoke(serde_json::json!({"query": "battery"})),
                ctx_at(dir.path()),
            )
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&out.content).unwrap();
        let results = v["results"].as_array().unwrap();
        assert_eq!(results.len(), 2);
        assert!(results[0]["score"].as_f64().unwrap() > 0.0);
    }
}
