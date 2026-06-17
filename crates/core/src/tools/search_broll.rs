//! `search_broll` tool — query Pexels for stock B-roll matching a
//! free-form query. Returns up to N preview thumbnails + per-rendition
//! metadata. The agent uses this to scout candidates before committing
//! to a download (which is `use_broll`'s job).
//!
//! Read-only. Does not touch the timeline or the project's filesystem.
//!
//! Pexels' free tier caps at 200 search calls/hour. The tool itself
//! doesn't cache — the agent's session memory carries the result. If
//! you need to re-search the same query inside one session, call this
//! tool again; the cache is the conversation context, not us.

use async_trait::async_trait;
use serde::Deserialize;

use crate::FunctionCallError;
use crate::pexels;
use crate::tool::{ToolContext, ToolHandler, ToolInvocation, ToolOutput};
use crate::tool_schema::Tool as ToolSchema;

/// Default per-page on Pexels searches. The agent's UI typically only
/// previews top-3, so 5 is generous; the cap is 30.
const DEFAULT_PER_PAGE: u32 = 5;

/// The `search_broll` tool.
pub struct SearchBrollTool;

#[derive(Debug, Deserialize)]
struct Args {
    /// Pexels query string. Free-form; the API's relevance ranker
    /// does the matching. Examples: "city skyline morning",
    /// "busy office tracking shot", "coffee being poured slow motion".
    query: String,
    /// Max results to return. Default 5, hard cap 30.
    #[serde(default)]
    per_page: Option<u32>,
}

#[async_trait]
impl ToolHandler for SearchBrollTool {
    fn name(&self) -> &'static str {
        "search_broll"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "search_broll".into(),
            description: DESCRIPTION.into(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["query"],
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Pexels search query. Be specific (\"empty city street at dawn\") rather than abstract (\"loneliness\")."
                    },
                    "per_page": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 30,
                        "description": "How many results to return. Default 5."
                    }
                }
            }),
            ..Default::default()
        }
    }

    fn is_mutating(&self, _invocation: &ToolInvocation) -> bool {
        false
    }

    async fn handle(
        &self,
        invocation: ToolInvocation,
        _ctx: ToolContext,
    ) -> Result<ToolOutput, FunctionCallError> {
        let args: Args = serde_json::from_value(invocation.args).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "search_broll: invalid args ({e}). Required: {{\"query\": string}}."
            ))
        })?;
        if args.query.trim().is_empty() {
            return Err(FunctionCallError::RespondToModel(
                "search_broll: query is empty. Pass a phrase like \"city skyline morning\".".into(),
            ));
        }
        let per_page = args.per_page.unwrap_or(DEFAULT_PER_PAGE).clamp(1, 30);

        let client = pexels::Client::from_env_or_keychain(pexels::ClientConfig::default())
            .map_err(map_pexels_err)?;

        let resp = client
            .search_videos(&args.query, per_page)
            .await
            .map_err(map_pexels_err)?;

        let results: Vec<SearchResult> = resp
            .videos
            .iter()
            .map(|v| SearchResult {
                pexels_id: v.id,
                duration_s: v.duration,
                width: v.width,
                height: v.height,
                preview_thumbnail: v.image.clone(),
                pexels_page: v.url.clone(),
                attribution: format!("{} (Pexels: {})", v.user.name, v.user.url),
                renditions: v
                    .video_files
                    .iter()
                    .map(|f| Rendition {
                        rendition_id: f.id,
                        quality: f.quality.clone(),
                        file_type: f.file_type.clone(),
                        width: f.width,
                        height: f.height,
                        link: f.link.clone(),
                    })
                    .collect(),
                frame_previews: v
                    .video_pictures
                    .iter()
                    .take(4)
                    .map(|p| p.picture.clone())
                    .collect(),
            })
            .collect();

        let body = serde_json::json!({
            "query": args.query,
            "per_page": per_page,
            "total_results": resp.total_results,
            "results": results,
        });
        Ok(ToolOutput::text(body.to_string()))
    }
}

/// One search hit, projected to what the agent + UI need to choose.
#[derive(Debug, Clone, serde::Serialize)]
struct SearchResult {
    /// Pexels' stable id; pass to `use_broll` to download.
    pexels_id: u64,
    /// Length in seconds.
    duration_s: u32,
    /// Native width.
    width: u32,
    /// Native height.
    height: u32,
    /// Single-image preview URL (the "poster" thumbnail).
    preview_thumbnail: String,
    /// Pexels page URL — used for attribution UX.
    pexels_page: String,
    /// Attribution string ready to show.
    attribution: String,
    /// Available downloadable renditions (mp4/webm at various sizes).
    /// The agent doesn't normally need to see these — `use_broll`
    /// picks. Surfaced for transparency / debugging.
    renditions: Vec<Rendition>,
    /// Up to 4 frame thumbnails sampled along the clip — useful for
    /// the BrollNoteCard scrub-strip preview.
    frame_previews: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
struct Rendition {
    rendition_id: u64,
    quality: String,
    file_type: String,
    width: Option<u32>,
    height: Option<u32>,
    link: String,
}

fn map_pexels_err(err: pexels::PexelsError) -> FunctionCallError {
    match err {
        pexels::PexelsError::MissingApiKey => FunctionCallError::RespondToModel(
            "search_broll: Pexels is needed for stock b-roll. Add your Pexels key \
             in Settings -> Advanced -> Provider Keys, then retry."
                .into(),
        ),
        pexels::PexelsError::Api { status: 429, .. } => FunctionCallError::RespondToModel(
            "search_broll: Pexels rate-limited (200/hr free-tier cap). \
             Pause and try a narrower query later."
                .into(),
        ),
        other => {
            FunctionCallError::RespondToModel(format!("search_broll: Pexels call failed: {other}"))
        }
    }
}

const DESCRIPTION: &str = "\
Search Pexels for stock B-roll videos matching a free-form query. \
Returns: { query, per_page, total_results, results: [{ pexels_id, \
duration_s, width, height, preview_thumbnail, pexels_page, \
attribution, renditions, frame_previews }] }.\
\n\nUse this to scout candidate cutaways for a moment surfaced by \
`find_broll_opportunities` (or any other moment you think wants \
b-roll). Tell the user the previews; they pick. Then call \
`use_broll(pexels_id, ...)` to download + place.\
\n\nBe specific in your query: \"empty city street at dawn\" beats \
\"loneliness\". Pexels' relevance ranker is concrete-noun friendly. \
Default per_page=5; cap 30 — top-3 is usually all the user wants \
to see.\
\n\nRequires a Pexels key. Calling without one surfaces a setup prompt \
for Settings -> Advanced -> Provider Keys rather than crashing.\
";
