//! `transcript_search` — first-class search over whisper transcript
//! sidecars. Ported from `crates/core/src/tools/transcript_search.rs`
//! to the in-process MCP server.

use awidat_index::walk_indexer;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::awidat_mcp::context::McpToolCtx;

const DEFAULT_LIMIT: usize = 25;
const MAX_LIMIT: usize = 100;

/// Arguments to `transcript_search`.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct TranscriptSearchArgs {
    /// Substring to search for (case-insensitive).
    pub query: String,
    /// Restrict matches to a single asset id.
    #[serde(default)]
    pub asset_id: Option<String>,
    /// Restrict matches to a single speaker label.
    #[serde(default)]
    pub speaker: Option<String>,
    /// Max results to return. Default 25, hard cap 100.
    #[serde(default)]
    pub limit: Option<usize>,
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

/// Run `transcript_search` against the project resolved from
/// [`McpToolCtx`]. Returns the JSON body as `Ok(String)`; argument or
/// sidecar-walk errors return `Err(String)`.
pub fn run(args: TranscriptSearchArgs, ctx: McpToolCtx) -> Result<String, String> {
    let query = args.query.trim().to_ascii_lowercase();
    if query.is_empty() {
        return Err("transcript_search: query must not be empty".into());
    }
    let limit = args.limit.unwrap_or(DEFAULT_LIMIT).min(MAX_LIMIT);
    let speaker_filter = args
        .speaker
        .as_ref()
        .map(|speaker| speaker.to_ascii_lowercase());
    let mut results = Vec::new();
    let walker = walk_indexer(&ctx.project_root, "whisper").map_err(|e| e.to_string())?;
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
    Ok(serde_json::json!({
        "query": args.query,
        "total_matches": total,
        "limit": limit,
        "results": results
    })
    .to_string())
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

pub const DESCRIPTION: &str = "\
Search whisper transcript segments across project media, optionally \
filtering by asset or speaker. Returns matching segments ranked by \
match count, with start/end times, speaker, and a truncated text \
preview. Read-only.";
