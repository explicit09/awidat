//! `list_assets` tool — paginated listing of project assets.
//!
//! Per `PLAN.md` §6.1 + the survey of Codex's `list_dir`: 1-indexed
//! offset, default limit 25, hard cap 100, per-entry truncation at 500B,
//! "More than N entries found" footer when truncated. Lifted near-verbatim.
//!
//! Scope: `raw` (source media), `renders` (engine outputs), or `all` (both).

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde::Deserialize;

use crate::FunctionCallError;
use crate::anthropic::Tool as ToolSchema;
use crate::tool::{ToolContext, ToolHandler, ToolInvocation, ToolOutput};

/// Default limit. Codex `list_dir.rs:31-33`.
const DEFAULT_LIMIT: usize = 25;
/// Hard cap; queries asking for more get clamped + a footer.
const MAX_LIMIT: usize = 100;
/// Per-entry text cap. Codex `list_dir.rs:24` `MAX_ENTRY_LENGTH`.
const MAX_ENTRY_LENGTH: usize = 500;

/// The `list_assets` tool.
pub struct ListAssetsTool;

#[derive(Debug, Deserialize)]
struct ListAssetsArgs {
    /// `raw` | `renders` | `all`. Default `all`.
    #[serde(default)]
    scope: Option<String>,
    /// 1-indexed first entry to return. Default 1.
    #[serde(default)]
    offset: Option<usize>,
    /// Max entries (default 25, hard cap 100).
    #[serde(default)]
    limit: Option<usize>,
}

#[async_trait]
impl ToolHandler for ListAssetsTool {
    fn name(&self) -> &'static str {
        "list_assets"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "list_assets".into(),
            description: DESCRIPTION.into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "scope": {
                        "type": "string",
                        "enum": ["raw", "renders", "all"],
                        "description": "Which directory(ies) to list. Default 'all'."
                    },
                    "offset": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "1-indexed first entry. Default 1."
                    },
                    "limit": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 100,
                        "description": "Max entries. Default 25."
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
        ctx: ToolContext,
    ) -> Result<ToolOutput, FunctionCallError> {
        let args: ListAssetsArgs = serde_json::from_value(invocation.args).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "list_assets: invalid args ({e}). All fields optional."
            ))
        })?;
        let scope = args.scope.as_deref().unwrap_or("all").to_string();
        let offset = args.offset.unwrap_or(1);
        if offset == 0 {
            return Err(FunctionCallError::RespondToModel(
                "list_assets: offset must be a 1-indexed entry number (≥ 1)".into(),
            ));
        }
        let limit = args.limit.unwrap_or(DEFAULT_LIMIT).min(MAX_LIMIT);

        let mut entries: Vec<AssetEntry> = Vec::new();
        match scope.as_str() {
            "raw" => collect(&ctx.project_root.join("raw"), "raw", &mut entries),
            "renders" => collect(&ctx.project_root.join("renders"), "renders", &mut entries),
            "all" => {
                collect(&ctx.project_root.join("raw"), "raw", &mut entries);
                collect(&ctx.project_root.join("renders"), "renders", &mut entries);
            }
            other => {
                return Err(FunctionCallError::RespondToModel(format!(
                    "list_assets: scope '{other}' not recognized. Use one of: raw, renders, all."
                )));
            }
        }

        // Stable sort by (scope, relative_path).
        entries.sort_by(|a, b| (&a.scope, &a.rel_path).cmp(&(&b.scope, &b.rel_path)));

        let total = entries.len();
        let start = offset.saturating_sub(1).min(total);
        let end = (start + limit).min(total);
        let visible = &entries[start..end];

        let mut out = String::new();
        out.push_str(&format!(
            "scope={scope} total={total} offset={offset} limit={limit}\n"
        ));
        for (i, e) in visible.iter().enumerate() {
            let idx = offset + i;
            let line = format!(
                "{idx:>4}. [{scope}] {rel}  ({size})",
                scope = e.scope,
                rel = e.rel_path,
                size = human_size(e.size_bytes)
            );
            out.push_str(&truncate(line, MAX_ENTRY_LENGTH));
            out.push('\n');
        }
        if end < total {
            out.push_str(&format!(
                "More than {limit} entries found ({remaining} additional). Re-call with offset={next}.\n",
                remaining = total - end,
                next = end + 1,
            ));
        }
        Ok(ToolOutput::text(out))
    }
}

#[derive(Debug)]
struct AssetEntry {
    scope: String,
    rel_path: String,
    size_bytes: u64,
}

fn collect(root: &Path, scope_label: &str, out: &mut Vec<AssetEntry>) {
    if !root.is_dir() {
        return;
    }
    walk(root, root, scope_label, out);
}

fn walk(root: &Path, here: &Path, scope_label: &str, out: &mut Vec<AssetEntry>) {
    let Ok(entries) = std::fs::read_dir(here) else {
        return;
    };
    for ent in entries.flatten() {
        let p = ent.path();
        if p.is_dir() {
            walk(root, &p, scope_label, out);
            continue;
        }
        let Ok(meta) = ent.metadata() else { continue };
        if !meta.is_file() {
            continue;
        }
        let Ok(rel) = p.strip_prefix(root) else { continue };
        out.push(AssetEntry {
            scope: scope_label.to_string(),
            rel_path: rel.to_string_lossy().replace('\\', "/"),
            size_bytes: meta.len(),
        });
    }
}

fn human_size(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{bytes} B")
    } else {
        format!("{v:.1} {}", UNITS[i])
    }
}

fn truncate(s: String, cap: usize) -> String {
    if s.len() <= cap {
        return s;
    }
    let mut end = cap;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

// Suppress unused import.
#[allow(dead_code)]
fn _unused(_p: PathBuf) {}

const DESCRIPTION: &str = "\
List source assets and renders in the project. Returns a numbered, \
paginated list with file sizes. Scope: 'raw' (source media), 'renders' \
(engine outputs), 'all' (both, default). 1-indexed offset, default \
limit 25, hard cap 100. Use this to discover what's available before \
calling `inspect_clip` or `read_index` on a specific asset.\
";

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::broadcast;

    fn ctx_at(root: &Path) -> ToolContext {
        let (tx, _) = broadcast::channel(8);
        ToolContext {
            project_root: root.to_path_buf(),
            events_tx: tx,
            user_input_tx: None,
            job_manager: awidat_render::JobManager::new(),

            approval_tx: None,
            mcp_host: crate::mcp_host::McpHost::new(awidat_mcp::ClientInfo { name: "test".into(), version: "0.0.0".into() }),
            skills: std::sync::Arc::new(crate::skills::SkillRegistry::default()),
        }
    }

    fn invoke(args: serde_json::Value) -> ToolInvocation {
        ToolInvocation {
            call_id: "c1".into(),
            name: "list_assets".into(),
            args,
        }
    }

    fn make(p: &Path, body: &[u8]) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    fn project_with_assets() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        make(&r.join("raw/ep-001.wav"), b"a");
        make(&r.join("raw/ep-002.wav"), &[0u8; 1500]);
        make(&r.join("raw/sub/ep-003.mp4"), &[0u8; 1024 * 1024 + 1]);
        make(&r.join("renders/highlight.mp4"), &[0u8; 4096]);
        dir
    }

    #[tokio::test]
    async fn lists_all_with_default_window() {
        let dir = project_with_assets();
        let out = ListAssetsTool
            .handle(invoke(serde_json::json!({})), ctx_at(dir.path()))
            .await
            .unwrap();
        let body = &out.content;
        assert!(body.contains("scope=all total=4"));
        assert!(body.contains("[raw] ep-001.wav"));
        assert!(body.contains("[raw] ep-002.wav"));
        assert!(body.contains("[raw] sub/ep-003.mp4"));
        assert!(body.contains("[renders] highlight.mp4"));
    }

    #[tokio::test]
    async fn raw_scope_filters() {
        let dir = project_with_assets();
        let out = ListAssetsTool
            .handle(
                invoke(serde_json::json!({"scope": "raw"})),
                ctx_at(dir.path()),
            )
            .await
            .unwrap();
        assert!(out.content.contains("scope=raw total=3"));
        assert!(!out.content.contains("[renders]"));
    }

    #[tokio::test]
    async fn pagination_via_offset_and_limit() {
        let dir = project_with_assets();
        let out = ListAssetsTool
            .handle(
                invoke(serde_json::json!({"offset": 2, "limit": 2})),
                ctx_at(dir.path()),
            )
            .await
            .unwrap();
        // total=4, asking for 2 starting at idx 2 → entries 2 and 3.
        assert!(out.content.contains("offset=2 limit=2"));
        assert!(out.content.contains("More than 2 entries found"));
        // entries 1 and 4 should be absent.
        let lines: Vec<&str> = out.content.lines().collect();
        let numbered: Vec<&&str> = lines.iter().filter(|l| l.trim_start().starts_with(|c: char| c.is_ascii_digit())).collect();
        assert_eq!(numbered.len(), 2);
    }

    #[tokio::test]
    async fn offset_zero_is_respond_to_model() {
        let dir = project_with_assets();
        let err = ListAssetsTool
            .handle(
                invoke(serde_json::json!({"offset": 0})),
                ctx_at(dir.path()),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, FunctionCallError::RespondToModel(msg) if msg.contains("1-indexed")));
    }

    #[tokio::test]
    async fn unknown_scope_is_respond_to_model() {
        let dir = project_with_assets();
        let err = ListAssetsTool
            .handle(
                invoke(serde_json::json!({"scope": "bogus"})),
                ctx_at(dir.path()),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, FunctionCallError::RespondToModel(_)));
    }

    #[tokio::test]
    async fn empty_project_lists_zero() {
        let dir = tempfile::tempdir().unwrap();
        let out = ListAssetsTool
            .handle(invoke(serde_json::json!({})), ctx_at(dir.path()))
            .await
            .unwrap();
        assert!(out.content.contains("total=0"));
    }

    #[test]
    fn human_size_units() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(500), "500 B");
        assert_eq!(human_size(1500), "1.5 KB");
        assert!(human_size(1024 * 1024 + 1).starts_with("1.0 MB"));
    }
}
