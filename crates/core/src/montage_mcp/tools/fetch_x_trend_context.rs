//! `fetch_x_trend_context` — gather X trend signals for short-form planning.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::montage_mcp::context::McpToolCtx;
use crate::x_trends::{
    XClient, XClientConfig, XRecentSearchResponse, XTrendContextRequest,
    build_context_from_searches, missing_credentials_context,
};

#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct FetchXTrendContextArgs {
    /// Search queries related to the episode topics. Keep these specific.
    pub queries: Vec<String>,
    /// Requested results per query. X recent search requires at least 10.
    #[serde(default)]
    pub max_results: Option<u8>,
}

pub async fn run(args: FetchXTrendContextArgs, _ctx: McpToolCtx) -> Result<String, String> {
    let request = XTrendContextRequest {
        queries: args.queries,
        max_results: args.max_results.unwrap_or_default(),
    }
    .normalized();
    if request.queries.is_empty() {
        return Err("fetch_x_trend_context: at least one non-empty query is required.".into());
    }

    let Some(client) = XClient::from_env_or_keychain(XClientConfig::default())
        .map_err(|error| format!("fetch_x_trend_context: {error}"))?
    else {
        return serde_json::to_string_pretty(&missing_credentials_context(request.queries))
            .map_err(|error| format!("fetch_x_trend_context serialization failed: {error}"));
    };

    let mut searches: Vec<(String, XRecentSearchResponse)> = Vec::new();
    for query in &request.queries {
        let response = client
            .recent_search(query, request.max_results)
            .await
            .map_err(|error| format!("fetch_x_trend_context: {error}"))?;
        searches.push((query.clone(), response));
    }

    serde_json::to_string_pretty(&build_context_from_searches(searches))
        .map_err(|error| format!("fetch_x_trend_context serialization failed: {error}"))
}

pub const DESCRIPTION: &str = "\
Fetch current X trend signals for one or more episode-topic queries and return \
a trend_context payload suitable for plan_short_form_review. Uses the configured \
X bearer token for read access. Publishing is not performed \
here; Twitter/X posting remains handled by the social publishing provider.";
