//! `find_moment` — search the index for moments matching a query.
//! Ported from `crates/core/src/tools/find_moment.rs` to the in-process
//! MCP server.
//!
//! Per `PLAN.md` §6.1 + survey of SWE-agent's `find_file`:
//! **paths/ranges only, no embedded thumbnails.** The model calls
//! `view_frame` / `read_index` after `find_moment` exactly the way
//! SWE-agent calls `open` after `find_file`. This is a hard rule.
//!
//! Implementation (#159): BM25 ranking over whisper transcript
//! segments. Strict superset of the prior case-insensitive substring
//! path — substring queries still match (BM25 ranks them highest), and
//! semantically-related queries that don't share a substring now find
//! their target.

use awidat_index::walk_indexer;
use bm25::{Document, Language, SearchEngineBuilder};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::awidat_mcp::context::McpToolCtx;

/// Hard cap on results returned. Codex's `list_dir` uses 25; matches.
const DEFAULT_LIMIT: usize = 25;

/// Per-result snippet length cap. SWE-agent §2.3: keep matches terse.
const SNIPPET_CAP: usize = 200;

/// Arguments to `find_moment`.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct FindMomentArgs {
    /// Search query for transcript segments.
    pub query: String,
    /// Optional asset id filter; if set, only that asset is searched.
    #[serde(default)]
    pub asset_id: Option<String>,
    /// Max results (default 25, hard cap 100).
    #[serde(default)]
    pub limit: Option<usize>,
}

/// Run `find_moment` against the project resolved from [`McpToolCtx`].
/// Returns the JSON body as `Ok(String)`; invalid args / sidecar-read
/// errors return `Err(String)`.
pub fn run(args: FindMomentArgs, ctx: McpToolCtx) -> Result<String, String> {
    let query = args.query.trim();
    if query.is_empty() {
        return Err(
            "find_moment: `query` was empty (or just whitespace). Pass a \
             non-empty query to search transcript text for. \
             Example: find_moment(query=\"battery fire\")."
                .into(),
        );
    }
    let limit = args.limit.unwrap_or(DEFAULT_LIMIT).min(100);

    // Walk the index once, collecting (asset, segment-fields) pairs
    // into a flat document corpus. BM25's index is fast to build at
    // this scale; we rebuild per-call rather than caching, since
    // whisper sidecars change often during a session and a stale cache
    // would surface out-of-date hits.
    let walker = walk_indexer(&ctx.project_root, "whisper").map_err(|e| e.to_string())?;

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
        return Ok(body.to_string());
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
    Ok(body.to_string())
}

struct SegmentDoc {
    asset_id: String,
    start_s: Option<serde_json::Value>,
    end_s: Option<serde_json::Value>,
    speaker_id: Option<serde_json::Value>,
    text: String,
}

pub const DESCRIPTION: &str = "\
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
