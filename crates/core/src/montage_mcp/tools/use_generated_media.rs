//! `use_generated_media` — return a ready-to-apply `*** Insert BRoll`
//! EDL fragment for a completed generated-media asset. Ported from
//! `crates/core/src/tools/use_generated_media.rs` to the in-process
//! MCP server.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::generated_media::registry::{
    GeneratedMediaState, Registry, validate_generated_output_path,
};
use crate::montage_mcp::context::McpToolCtx;

/// Arguments to `use_generated_media`.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct UseGeneratedMediaArgs {
    /// Identifier of the generated-media job to consume.
    pub job_id: String,
    /// Where the cutaway lands. Same anchor shape as the EDL grammar.
    pub anchor: AnchorArg,
    /// Cutaway length in seconds (0.5–30).
    pub duration_s: f64,
    /// `overlay` (default) or `replace`.
    #[serde(default)]
    pub position: Option<String>,
}

/// Anchor shape matching the EDL grammar.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum AnchorArg {
    Transcript { transcript_snippet: String },
    Uuid { clip_uuid: String },
}

impl Default for AnchorArg {
    fn default() -> Self {
        AnchorArg::Uuid {
            clip_uuid: String::new(),
        }
    }
}

/// Run `use_generated_media` against the project resolved from
/// [`McpToolCtx`]. Returns a JSON status body as `Ok(String)`;
/// validation failures return `Err(String)`.
pub fn run(args: UseGeneratedMediaArgs, ctx: McpToolCtx) -> Result<String, String> {
    if !(0.5..=30.0).contains(&args.duration_s) {
        return Err(format!(
            "use_generated_media: duration_s={} out of range. Use 0.5-30.0.",
            args.duration_s
        ));
    }
    let position = match args.position.as_deref().unwrap_or("overlay") {
        "overlay" => "overlay",
        "replace" => "replace",
        other => {
            return Err(format!(
                "use_generated_media: invalid position '{other}'. Use 'overlay' or 'replace'."
            ));
        }
    };

    let registry = Registry::load_or_default(&ctx.project_root)
        .map_err(|e| format!("use_generated_media: {e}"))?;
    let record = registry
        .get(&args.job_id)
        .ok_or_else(|| format!("use_generated_media: job '{}' not found.", args.job_id))?;
    if record.state != GeneratedMediaState::Succeeded {
        return Err(format!(
            "use_generated_media: job '{}' is {:?}, not succeeded.",
            record.job_id, record.state
        ));
    }
    let asset_path = record.output_video_path().ok_or_else(|| {
        format!(
            "use_generated_media: job '{}' has no output video path.",
            record.job_id
        )
    })?;
    validate_generated_output_path(asset_path).map_err(|e| format!("use_generated_media: {e}"))?;
    let absolute_path = ctx.project_root.join(asset_path);
    let edl_fragment = build_edl_fragment(asset_path, &args.anchor, args.duration_s, position)
        .map_err(|message| format!("use_generated_media: {message}"))?;

    Ok(serde_json::json!({
        "asset_path": asset_path,
        "absolute_path": absolute_path.display().to_string(),
        "edl_fragment": edl_fragment,
        "provenance": {
            "job_id": record.job_id,
            "provider": record.provider,
            "model": record.model,
            "prompt_hash": record.prompt_hash,
            "requires_disclosure": record.requires_disclosure,
            "uses_likeness": record.uses_likeness
        },
        "next_step": "Hand the edl_fragment to apply_edl to place the cutaway, then run view_timeline/podcast_visual_polish and verify this generated asset still matches the resolved transcript anchor before claiming B-roll is done."
    })
    .to_string())
}

fn build_edl_fragment(
    asset_rel: &str,
    anchor: &AnchorArg,
    duration_s: f64,
    position: &str,
) -> Result<String, String> {
    let anchor_line = match anchor {
        AnchorArg::Transcript { transcript_snippet } => {
            reject_edl_control_chars(transcript_snippet, "transcript_snippet")?;
            let escaped = transcript_snippet.replace('"', "\\\"");
            format!("@@ anchor: transcript_snippet=\"{escaped}\"")
        }
        AnchorArg::Uuid { clip_uuid } => {
            reject_edl_control_chars(clip_uuid, "clip_uuid")?;
            format!("@@ anchor: clip_uuid={clip_uuid}")
        }
    };
    Ok(format!(
        "*** Begin EDL\n\
         *** Insert BRoll\n\
         {anchor_line}\n\
         + asset: {asset_rel}\n\
         + duration_s: {duration_s}\n\
         + position: {position}\n\
         *** End EDL\n"
    ))
}

fn reject_edl_control_chars(value: &str, field: &str) -> Result<(), String> {
    if value.contains('\n') || value.contains('\r') {
        return Err(format!("{field} cannot contain newlines"));
    }
    Ok(())
}

pub const DESCRIPTION: &str = "\
Return a ready-to-apply Insert BRoll EDL fragment for a completed generated \
media asset. This does not call apply_edl or mutate the timeline.";
