//! `plan_generated_media` — read-only planner for generated-media prompt
//! candidates. Ported from `crates/core/src/tools/plan_generated_media.rs`
//! to the in-process MCP server.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::montage_mcp::context::McpToolCtx;
use crate::montage_mcp::tools::find_generated_broll_opportunities::asset_requests_for_moment;

/// Arguments to `plan_generated_media`.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct PlanGeneratedMediaArgs {
    /// Free-text description of the editorial moment to illustrate.
    pub moment: String,
    /// Optional style modifier such as "documentary" or "cinematic".
    #[serde(default)]
    pub style: Option<String>,
}

/// Run `plan_generated_media`. The project root from [`McpToolCtx`] is
/// unused — this planner builds purely from `args` — but the signature
/// is kept uniform with the rest of the MCP tool surface.
pub fn run(args: PlanGeneratedMediaArgs, _ctx: McpToolCtx) -> Result<String, String> {
    let moment = args.moment.trim();
    if moment.is_empty() {
        return Err("plan_generated_media: moment is empty.".into());
    }
    let style = args.style.as_deref().unwrap_or("natural editorial");
    let asset_requests = asset_requests_for_moment(moment, moment);
    let candidates = vec![
        format!("{style} b-roll cutaway illustrating {moment}, steady camera, realistic lighting"),
        format!("{style} establishing shot for {moment}, no text, no logos"),
        format!("{style} detail shot that supports {moment}, documentary tone"),
    ];
    Ok(serde_json::json!({
        "workflow_purpose": "broll",
        "artifact_kind": "video",
        "recommended_provider": "openrouter",
        "candidates": candidates,
        "options": {
            "providers": ["openrouter", "mock"],
            "default_model": crate::generated_media::openrouter::DEFAULT_VIDEO_MODEL,
            "generation_modes": ["text_to_video"],
            "note": "Use openrouter for US-accessible video generation. Use mock for offline tests."
        },
        "asset_requests": asset_requests,
        "fallback_note": "If requested assets are unavailable, use a generic unbranded prompt and avoid exact logos, readable UI text, or unapproved likeness."
    })
    .to_string())
}

pub const DESCRIPTION: &str = "\
Read-only planner for generated b-roll. Returns deterministic prompt \
candidates and provider/mode options without writing the registry.";
