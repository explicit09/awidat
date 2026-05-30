//! `list_episodes` — read durable first-class episode spans.

use awidat_proto::awidat_meta::EpisodeSpan;
use awidat_proto::project::Project;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::awidat_mcp::context::McpToolCtx;

#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct ListEpisodesArgs {}

#[derive(Debug, Serialize)]
struct ListEpisodesResponse {
    status: &'static str,
    total: usize,
    episodes: Vec<EpisodeView>,
}

#[derive(Debug, Serialize)]
struct EpisodeView {
    id: String,
    name: Option<String>,
    order: Option<u32>,
    asset_id: String,
    source_start_s: f64,
    source_end_s: f64,
    duration_s: f64,
    confidence: Option<f64>,
    status: String,
    evidence: Vec<String>,
}

pub fn run(_args: ListEpisodesArgs, ctx: McpToolCtx) -> Result<String, String> {
    let project = Project::read(&ctx.project_root)
        .map_err(|e| format!("list_episodes: unable to read project: {e}"))?;
    let episodes = project
        .timeline
        .metadata
        .awidat
        .as_ref()
        .map(|meta| meta.episodes.clone())
        .unwrap_or_default();

    let response = ListEpisodesResponse {
        status: "ready",
        total: episodes.len(),
        episodes: episodes.into_iter().map(EpisodeView::from).collect(),
    };
    serde_json::to_string(&response).map_err(|e| format!("list_episodes serialize: {e}"))
}

impl From<EpisodeSpan> for EpisodeView {
    fn from(episode: EpisodeSpan) -> Self {
        let duration_s = episode.duration_s();
        Self {
            id: episode.id,
            name: episode.name,
            order: episode.order,
            asset_id: episode.asset_id,
            source_start_s: episode.source_start_s,
            source_end_s: episode.source_end_s,
            duration_s,
            confidence: episode.confidence,
            status: serde_json::to_value(episode.status)
                .ok()
                .and_then(|value| value.as_str().map(ToOwned::to_owned))
                .unwrap_or_else(|| "review_needed".to_string()),
            evidence: episode.evidence,
        }
    }
}

pub const DESCRIPTION: &str = "\
List durable episode spans stored in Timeline.metadata.awidat.episodes as \
JSON. Each episode includes id, optional name/order, source asset id, \
source start/end/duration, confidence, review status, and evidence. Use \
this after podcast_episode_spans/apply_episode_spans to inspect accepted, \
review-needed, and rejected spans without mutating the project.";
