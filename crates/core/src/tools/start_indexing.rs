//! `start_indexing` tool — runs the indexer pipeline over assets in
//! the project's `raw/` dir and returns the report inline.
//!
//! Why synchronous (vs. `start_render`-style fire-and-forget): the
//! agent typically calls indexing because the *next thing it wants
//! to do* (transcript queries, scene detection lookups) needs the
//! sidecars present. Returning a job_id and asking the model to
//! poll-loop is just busy-work; awaiting the run inline is what
//! the model wants anyway.
//!
//! The desktop has its own `index_project` Tauri command for the
//! "click a button, see a progress bar" path — that one's truly
//! background. This tool is the agent's lever, not the GUI's.

use async_trait::async_trait;
use awidat_config::Config;
use awidat_index::{AssetInput, PairOutcome};
use awidat_mcp::ClientInfo;
use awidat_proto::index::AssetId;
use serde::Deserialize;

use crate::FunctionCallError;
use crate::anthropic::Tool as ToolSchema;
use crate::tool::{ToolContext, ToolHandler, ToolInvocation, ToolOutput};

/// The `start_indexing` tool.
pub struct StartIndexingTool;

#[derive(Debug, Deserialize, Default)]
struct StartIndexingArgs {
    /// Optional indexer-name filter. If non-empty, only indexers
    /// whose `name` matches one of these run. Default: all
    /// configured indexers.
    #[serde(default)]
    indexers: Vec<String>,
}

#[async_trait]
impl ToolHandler for StartIndexingTool {
    fn name(&self) -> &'static str {
        "start_indexing"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "start_indexing".into(),
            description: DESCRIPTION.into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "indexers": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Optional name filter. Run only these indexers (e.g. ['whisper'] to (re)transcribe). Empty/omitted = all configured indexers."
                    }
                }
            }),
            ..Default::default()
        }
    }

    fn is_mutating(&self, _invocation: &ToolInvocation) -> bool {
        // Indexing writes sidecars + manifest; long, expensive,
        // disk-touching. Approval-gate it like start_render.
        true
    }

    async fn handle(
        &self,
        invocation: ToolInvocation,
        ctx: ToolContext,
    ) -> Result<ToolOutput, FunctionCallError> {
        let args: StartIndexingArgs = if invocation.args.is_null() {
            StartIndexingArgs::default()
        } else {
            serde_json::from_value(invocation.args).map_err(|e| {
                FunctionCallError::RespondToModel(format!(
                    "start_indexing: invalid args ({e}). Optional: {{ \"indexers\": [\"whisper\", ...] }}"
                ))
            })?
        };

        let project_root = ctx.project_root.clone();
        let config = Config::load(Some(&project_root)).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "start_indexing: load config: {e}"
            ))
        })?;
        let mut servers: Vec<_> = config.indexers().cloned().collect();
        if servers.is_empty() {
            return Err(FunctionCallError::RespondToModel(
                "start_indexing: no indexers configured. \
                 Add `[[mcp.servers]]` entries with kind = \"indexer\" \
                 to <project>/.awidat/config.toml or ~/.config/awidat/config.toml."
                    .into(),
            ));
        }
        if !args.indexers.is_empty() {
            servers.retain(|s| args.indexers.iter().any(|n| n == &s.name));
            if servers.is_empty() {
                return Err(FunctionCallError::RespondToModel(format!(
                    "start_indexing: indexers filter {:?} matched none of the \
                     configured indexers. Configured: {}",
                    args.indexers,
                    config
                        .indexers()
                        .map(|s| s.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )));
            }
        }

        let assets = collect_assets(&project_root).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "start_indexing: scan {}/raw: {e}",
                project_root.display()
            ))
        })?;
        if assets.is_empty() {
            return Err(FunctionCallError::RespondToModel(format!(
                "start_indexing: no assets to index. \
                 Drop source files under {}/raw or use the desktop's Import.",
                project_root.display()
            )));
        }

        let client_info = ClientInfo {
            name: "awidat-agent".into(),
            version: env!("CARGO_PKG_VERSION").into(),
        };

        let report = awidat_index::run(
            &project_root,
            &servers,
            &assets,
            client_info,
            4,
            // We pass `None` for progress — the report at the end
            // is what the model sees. The desktop's separate
            // index_project command emits per-pair progress to its
            // protocol pipe; this tool's caller is the agent loop,
            // not the GUI.
            None,
        )
        .await
        .map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "start_indexing: dispatcher: {e}"
            ))
        })?;

        Ok(ToolOutput::text(format_report(&report, &servers, &assets)))
    }
}

fn format_report(
    report: &awidat_index::IndexReport,
    servers: &[awidat_config::McpServer],
    assets: &[AssetInput],
) -> String {
    let (skipped, wrote, failed, dep_skipped) = report.counts();
    let mut out = String::new();
    out.push_str(&format!(
        "indexed {} asset(s) with {} indexer(s):\n",
        assets.len(),
        servers.len()
    ));
    out.push_str(&format!(
        "  {wrote} wrote, {skipped} skipped (sha unchanged), \
         {failed} failed, {dep_skipped} blocked-by-dep\n"
    ));
    if report.has_failures() {
        out.push_str("\nfailures:\n");
        for o in &report.outcomes {
            if let PairOutcome::Failed { indexer, asset, message } = o {
                out.push_str(&format!("  ✗ {indexer} · {asset}: {message}\n"));
            }
        }
    }
    out
}

fn collect_assets(project_root: &std::path::Path) -> std::io::Result<Vec<AssetInput>> {
    let raw_dir = project_root.join("raw");
    if !raw_dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    walk(project_root, &raw_dir, &mut out)?;
    Ok(out)
}

fn walk(
    project_root: &std::path::Path,
    dir: &std::path::Path,
    out: &mut Vec<AssetInput>,
) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            walk(project_root, &path, out)?;
        } else if path.is_file() {
            let id = path
                .strip_prefix(project_root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            out.push(AssetInput {
                id: AssetId::new(id),
                path,
            });
        }
    }
    Ok(())
}

const DESCRIPTION: &str = "\
Run the configured indexers (whisper transcription, scene detection, \
audio energy, editorial moments, etc) over every media file in the \
project's raw/ dir. Returns the summary report inline once finished. \
The dispatcher is sha-keyed — re-running on already-indexed assets \
is a fast no-op, so it's safe to call any time you suspect sidecars \
might be stale. Pass an optional `indexers` filter (e.g. ['whisper']) \
to re-run only specific producers — useful when iterating on a single \
indexer's tuning. WARNING: indexing a fresh asset can take 20+ minutes \
for hour-long video; only call when the user has asked for an editorial \
operation that needs the sidecars and view_episode shows they're missing. \
Don't proactively re-index already-indexed projects.\
";

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::broadcast;

    fn ctx_at(root: &std::path::Path) -> ToolContext {
        let (tx, _) = broadcast::channel(8);
        ToolContext {
            project_root: root.to_path_buf(),
            events_tx: tx,
            user_input_tx: None,
            job_manager: awidat_render::JobManager::new(),
            approval_tx: None,
            mcp_host: crate::mcp_host::McpHost::new(awidat_mcp::ClientInfo {
                name: "test".into(),
                version: "0.0.0".into(),
            }),
            skills: std::sync::Arc::new(crate::skills::SkillRegistry::default()),
            subagent_return: None,
        }
    }

    fn invoke(args: serde_json::Value) -> ToolInvocation {
        ToolInvocation {
            call_id: "c1".into(),
            name: "start_indexing".into(),
            args,
        }
    }

    #[tokio::test]
    async fn empty_raw_dir_is_respond_to_model() {
        // Bare project (no raw/ contents) returns a friendly
        // "no assets" message, not a panic. Whether the user's
        // global config has indexers configured varies per
        // machine; the no-assets check fires either way.
        let dir = tempfile::tempdir().unwrap();
        let err = StartIndexingTool
            .handle(invoke(serde_json::json!({})), ctx_at(dir.path()))
            .await
            .unwrap_err();
        match err {
            FunctionCallError::RespondToModel(msg) => {
                assert!(
                    msg.contains("no assets") || msg.contains("no indexers"),
                    "got: {msg}"
                );
            }
            other => panic!("want RespondToModel, got {other:?}"),
        }
    }

    #[test]
    fn schema_marks_indexers_optional() {
        let s = StartIndexingTool.schema();
        let schema = s.input_schema;
        // Sanity: indexers is in the property list.
        let props = schema.get("properties").unwrap();
        assert!(props.get("indexers").is_some());
        // No `required` field in the schema (or it doesn't include
        // 'indexers') — the call is valid with no args.
        let required = schema.get("required");
        if let Some(req) = required {
            let arr = req.as_array().unwrap();
            assert!(!arr.iter().any(|v| v == "indexers"));
        }
    }

    #[test]
    fn is_mutating_returns_true() {
        let inv = ToolInvocation {
            call_id: "c1".into(),
            name: "start_indexing".into(),
            args: serde_json::json!({}),
        };
        assert!(StartIndexingTool.is_mutating(&inv));
    }
}
