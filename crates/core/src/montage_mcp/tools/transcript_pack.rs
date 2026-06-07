//! `transcript_pack` — compact transcript evidence for planning.
//! Ported from `crates/core/src/tools/transcript_pack.rs`.

use montage_proto::project::Project;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::montage_mcp::context::McpToolCtx;
use crate::transcript_pack::{
    DEFAULT_MAX_WORDS_PER_ASSET, TranscriptPackOptions, build_timeline_transcript_pack,
    build_transcript_pack,
};

/// Arguments to `transcript_pack`.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct TranscriptPackArgs {
    /// Optional project asset id to include. Omit to pack every
    /// whisper transcript sidecar.
    #[serde(default)]
    pub asset_id: Option<String>,
    /// Maximum word-timing entries per asset. Defaults to 120, hard
    /// cap 500.
    #[serde(default)]
    pub max_words_per_asset: Option<usize>,
    /// When true, pack matching whisper sidecars without filtering to
    /// the current timeline. Defaults to false.
    #[serde(default)]
    pub include_all_assets: bool,
}

pub fn run(args: TranscriptPackArgs, ctx: McpToolCtx) -> Result<String, String> {
    let options = TranscriptPackOptions {
        asset_id: args.asset_id,
        max_words_per_asset: args
            .max_words_per_asset
            .or(Some(DEFAULT_MAX_WORDS_PER_ASSET)),
    };
    let pack = if args.include_all_assets {
        build_transcript_pack(&ctx.project_root, options)
    } else {
        let project = Project::read(&ctx.project_root)
            .map_err(|e| format!("transcript_pack: read project: {e}"))?;
        build_timeline_transcript_pack(&ctx.project_root, &project.timeline, options)
    }
    .map_err(|e| format!("transcript_pack: {e}"))?;
    serde_json::to_string(&pack).map_err(|e| format!("transcript_pack: encode output: {e}"))
}

pub const DESCRIPTION: &str = "\
Build a compact transcript pack from whisper sidecars for speech-led \
cut, caption, and cleanup planning.\
";
