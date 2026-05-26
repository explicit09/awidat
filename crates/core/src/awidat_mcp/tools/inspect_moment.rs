//! `inspect_moment` — drill into one editorial beat.
//! Ported in step 5 from `crates/core/src/tools/inspect_moment.rs`.
//!
//! Where `find_beat` returns a list of candidates, `inspect_moment`
//! returns *one* beat fully expanded: the full record, ±10s of
//! transcript context around it, and any dependencies inlined.

use awidat_index::walk_indexer;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::awidat_mcp::context::McpToolCtx;

#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct InspectMomentArgs {
    /// The moment_id from `find_beat`.
    pub moment_id: String,
    /// Surrounding transcript context (seconds). Default 10.0.
    #[serde(default)]
    pub context_s: Option<f64>,
}

pub fn run(args: InspectMomentArgs, ctx: McpToolCtx) -> Result<String, String> {
    let context_s = args.context_s.unwrap_or(10.0).max(0.0);

    // Walk every editorial-moments sidecar; first match wins.
    let walker = walk_indexer(&ctx.project_root, "editorial-moments").map_err(|e| {
        format!(
            "inspect_moment: editorial-moments sidecars not readable ({e}). \
             Run `awidat index --indexer editorial-moments <project>` and retry."
        )
    })?;
    let mut hit: Option<(String, serde_json::Value, Vec<serde_json::Value>)> = None;
    for (asset_id, sidecar) in walker {
        let moments = sidecar
            .pointer("/data/moments")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        if let Some(found) = moments.iter().find(|m| {
            m.get("moment_id").and_then(|x| x.as_str()) == Some(args.moment_id.as_str())
        }) {
            hit = Some((asset_id, found.clone(), moments));
            break;
        }
    }
    let Some((asset_id, moment, all_moments)) = hit else {
        return Err(format!(
            "inspect_moment: moment_id {:?} not found in any editorial-moments sidecar. \
             Run find_beat first to get valid moment_ids.",
            args.moment_id
        ));
    };

    let start_s = moment
        .get("start_s")
        .and_then(|x| x.as_f64())
        .unwrap_or(0.0);
    let end_s = moment.get("end_s").and_then(|x| x.as_f64()).unwrap_or(0.0);

    // Pull surrounding transcript from the whisper sidecar for this asset.
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
            dep_id
                .as_str()
                .and_then(|did| {
                    all_moments
                        .iter()
                        .find(|m| m.get("moment_id").and_then(|x| x.as_str()) == Some(did))
                })
                .cloned()
        })
        .collect();

    let body = serde_json::json!({
        "asset_id": asset_id,
        "moment": moment,
        "context_s": context_s,
        "transcript_window": transcript_window,
        "dependencies_expanded": deps,
    });
    Ok(body.to_string())
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

pub const DESCRIPTION: &str = "\
Drill into one editorial beat from the editorial-moments index. \
Returns: the full moment record, ±N seconds of surrounding transcript \
(default 10s), and any `dependencies` moments expanded inline so you \
can read their notes without a second tool call.\
\n\n\
Use after find_beat narrows to a candidate. The transcript window \
gives you the actual phrases to anchor against in apply_edl. The \
dependencies tell you what setup must stay intact when cutting this \
beat standalone.";
