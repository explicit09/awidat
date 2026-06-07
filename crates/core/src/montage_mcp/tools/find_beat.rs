//! `find_beat` — query the editorial-moments sidecar by kind /
//! speaker / score. Ported from `crates/core/src/tools/find_beat.rs`
//! to the in-process MCP server.
//!
//! Where `find_moment` is grep over the transcript, `find_beat` is
//! filter-and-sort over the *typed editorial decisions* the
//! editorial-moments indexer produced.

use montage_index::walk_indexer;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::montage_mcp::context::McpToolCtx;

/// Arguments to `find_beat`.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct FindBeatArgs {
    /// Filter by moment kind. Omit to return every kind.
    #[serde(default)]
    pub kind: Option<String>,
    /// Filter by speaker (whisper speaker_id). Omit for any.
    #[serde(default)]
    pub speaker: Option<String>,
    /// Minimum editorial score. Default 0.5.
    #[serde(default)]
    pub min_score: Option<f64>,
    /// Filter to one asset.
    #[serde(default)]
    pub asset_id: Option<String>,
    /// Max results. Default 10, hard cap 50.
    #[serde(default)]
    pub limit: Option<usize>,
}

/// Run `find_beat` against the project resolved from [`McpToolCtx`].
/// Returns the JSON body as `Ok(String)`; sidecar-walk errors return
/// `Err(String)`.
pub fn run(args: FindBeatArgs, ctx: McpToolCtx) -> Result<String, String> {
    let kind_filter = args.kind.as_deref().map(str::to_lowercase);
    let speaker_filter = args.speaker.as_deref();
    let min_score = args.min_score.unwrap_or(0.5);
    let limit = args.limit.unwrap_or(10).min(50);

    let walker = walk_indexer(&ctx.project_root, "editorial-moments").map_err(|e| {
        format!(
            "find_beat: editorial-moments sidecars not readable ({e}). \
             Run `montage index --indexer editorial-moments <project>` and retry."
        )
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
    Ok(body.to_string())
}

pub const DESCRIPTION: &str = "\
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
The editorial-moments index is produced by `montage index --indexer \
editorial-moments`; if find_beat returns empty, the index hasn't run \
yet. Each beat's `dependencies` field tells you which other beats it \
needs as setup — keep those when cutting standalone.\
";
