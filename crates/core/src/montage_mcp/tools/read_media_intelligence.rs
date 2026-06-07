//! `read_media_intelligence` — durable progressive intelligence state.
//! Ported in step 5 from
//! `crates/core/src/tools/read_media_intelligence.rs`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::montage_mcp::context::McpToolCtx;

#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct ReadMediaIntelligenceArgs {
    /// Optional project-relative source asset id.
    #[serde(default)]
    pub asset_id: Option<String>,
}

#[derive(Debug, Serialize)]
struct ReadMediaIntelligenceResponse {
    status: &'static str,
    project_root: String,
    asset_count: usize,
    ready_asset_count: usize,
    processing_asset_count: usize,
    blocked_asset_count: usize,
    offline_asset_count: usize,
    summary_for_agent: String,
    package: montage_proto::professional::MediaIntelligencePackage,
}

pub fn run(args: ReadMediaIntelligenceArgs, ctx: McpToolCtx) -> Result<String, String> {
    let mut package =
        crate::media_intelligence::build_media_intelligence_package(&ctx.project_root)
            .map_err(|e| format!("read_media_intelligence: unable to scan project media: {e}"))?;

    if let Some(asset_id) = args.asset_id.as_deref() {
        package.assets.retain(|asset| asset.asset_id == asset_id);
        if package.assets.is_empty() {
            return Err(format!(
                "read_media_intelligence: asset '{asset_id}' was not found under project raw media. \
                 Use list_assets/import_media first, or relink the source before inspecting progressive state."
            ));
        }
    }

    let ready_asset_count = count_state(
        &package,
        montage_proto::professional::MediaIntelligenceAggregateState::Ready,
    );
    let processing_asset_count = count_state(
        &package,
        montage_proto::professional::MediaIntelligenceAggregateState::Processing,
    );
    let blocked_asset_count = count_state(
        &package,
        montage_proto::professional::MediaIntelligenceAggregateState::Blocked,
    );
    let offline_asset_count = count_state(
        &package,
        montage_proto::professional::MediaIntelligenceAggregateState::Offline,
    );
    let response = ReadMediaIntelligenceResponse {
        status: response_status(
            processing_asset_count,
            blocked_asset_count,
            offline_asset_count,
        ),
        project_root: ctx.project_root.to_string_lossy().into_owned(),
        asset_count: package.assets.len(),
        ready_asset_count,
        processing_asset_count,
        blocked_asset_count,
        offline_asset_count,
        summary_for_agent: summarize_for_agent(
            package.assets.len(),
            ready_asset_count,
            processing_asset_count,
            blocked_asset_count,
            offline_asset_count,
        ),
        package,
    };
    serde_json::to_string(&response)
        .map_err(|e| format!("read_media_intelligence serialization failed: {e}"))
}

fn count_state(
    package: &montage_proto::professional::MediaIntelligencePackage,
    state: montage_proto::professional::MediaIntelligenceAggregateState,
) -> usize {
    package
        .assets
        .iter()
        .filter(|asset| asset.aggregate_state == state)
        .count()
}

fn response_status(processing: usize, blocked: usize, offline: usize) -> &'static str {
    if offline > 0 || blocked > 0 {
        "blocked"
    } else if processing > 0 {
        "processing"
    } else {
        "ok"
    }
}

fn summarize_for_agent(
    total: usize,
    ready: usize,
    processing: usize,
    blocked: usize,
    offline: usize,
) -> String {
    if total == 0 {
        return "No raw media assets were found. Import media before building progressive intelligence."
            .into();
    }
    if offline > 0 || blocked > 0 {
        return format!(
            "{offline} offline and {blocked} blocked asset(s) need repair before downstream intelligence can be trusted."
        );
    }
    if processing > 0 {
        return format!("{processing} of {total} asset(s) have processing layers in flight.");
    }
    format!("{ready} of {total} asset(s) have every intelligence layer ready.")
}

pub const DESCRIPTION: &str = "\
Read the progressive intelligence state machine for raw media assets without \
side effects. Returns independent layer readiness for source, proxy, waveform, \
transcript, speakers, scenes, topics, moments, clip candidates, and b-roll, \
plus aggregate state and next actions.";
