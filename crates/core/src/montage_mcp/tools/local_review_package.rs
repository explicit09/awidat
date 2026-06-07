//! `author_local_review_package` — create a local review package
//! from a rendered output path and the latest vedit commit metadata.
//! Ported in step 5 from `crates/core/src/tools/local_review_package.rs`
//! to the in-process MCP server.
//!
//! This is intentionally local-first: it writes a JSON artifact under
//! `<project>/.montage/review-packages/` and does not attempt third-
//! party sync or ingestion.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::montage_mcp::context::McpToolCtx;
use crate::review::build_local_review_package;

#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct LocalReviewPackageArgs {
    /// Path to a rendered review asset (absolute or project-relative).
    pub render_path: String,
    /// Optional package tags.
    #[serde(default)]
    pub tags: Vec<String>,
}

/// Run `author_local_review_package` against the project resolved from
/// [`McpToolCtx`]. Returns the JSON body as `Ok(String)`; argument or
/// build errors return `Err(String)`.
pub fn run(args: LocalReviewPackageArgs, ctx: McpToolCtx) -> Result<String, String> {
    if args.render_path.trim().is_empty() {
        return Err("author_local_review_package: render_path cannot be empty".into());
    }

    let package = build_local_review_package(&ctx.project_root, &args.render_path, args.tags)
        .map_err(|e| format!("author_local_review_package: {e}"))?;
    serde_json::to_string_pretty(&package)
        .map_err(|e| format!("author_local_review_package: failed to serialize package: {e}"))
}

/// Tool description, served to the model via MCP `tools/list`.
pub const DESCRIPTION: &str = "\
Author a local review package from a rendered output path. The package links that \
asset to the latest vedit commit, including commit header, commit hash, timeline hash, \
generated time, tags, and the commit reasoning body. The package is written as JSON \
under `<project>/.montage/review-packages/` and returned as a JSON object.\
If you are handing off a review render to a collaborator, use this tool before \
you share the file manually; third-party review APIs are intentionally not part of \
this local-only flow.";
