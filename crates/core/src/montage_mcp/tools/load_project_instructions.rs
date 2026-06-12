//! `load_project_instructions` — progressive project-doc access.
//!
//! Codex can inject AGENTS.md automatically, but Montage keeps the first
//! turn lean and exposes project docs on demand through this read-only MCP
//! tool instead.

use std::path::{Component, Path, PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::montage_mcp::context::McpToolCtx;

const DEFAULT_MAX_BYTES: usize = 20_000;
const HARD_MAX_BYTES: usize = 64_000;

#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct LoadProjectInstructionsArgs {
    /// Optional byte cap for the returned document. Defaults to 20KB,
    /// hard-capped at 64KB.
    #[serde(default)]
    pub max_bytes: Option<usize>,
    /// Optional project-relative path being worked on. When set, nested
    /// `AGENTS.override.md`/`AGENTS.md` files between the project root and this
    /// path are included (root first, most specific last) so directory-scoped
    /// instructions that override root guidance are returned.
    #[serde(default)]
    pub target_path: Option<String>,
}

pub fn run(args: LoadProjectInstructionsArgs, ctx: McpToolCtx) -> Result<String, String> {
    let max_bytes = args
        .max_bytes
        .unwrap_or(DEFAULT_MAX_BYTES)
        .min(HARD_MAX_BYTES);
    let paths = instruction_paths(&ctx, args.target_path.as_deref());
    if paths.is_empty() {
        return Ok("No project AGENTS.md or AGENTS.override.md found.".to_string());
    }

    let mut out = String::new();
    let mut remaining = max_bytes;
    let mut truncated = false;
    for path in &paths {
        let bytes = std::fs::read(path)
            .map_err(|e| format!("load_project_instructions: read {}: {e}", path.display()))?;
        let rel = path
            .strip_prefix(&ctx.project_root)
            .unwrap_or(path.as_path());
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str(&format!(
            "# Project instructions from {}\n\n",
            rel.display()
        ));
        let visible_len = bytes.len().min(remaining);
        out.push_str(String::from_utf8_lossy(&bytes[..visible_len]).trim());
        remaining -= visible_len;
        if bytes.len() > visible_len {
            truncated = true;
            break;
        }
    }
    if truncated {
        out.push_str(&format!(
            "\n\n[truncated after {max_bytes} bytes; request a higher max_bytes if needed]"
        ));
    }
    Ok(out)
}

/// Instruction files from the project root down to `target_path`'s directory,
/// ordered root-first so more specific nested files appear last. A single root
/// file is returned when `target_path` is `None` (or names nothing nested).
fn instruction_paths(ctx: &McpToolCtx, target_path: Option<&str>) -> Vec<PathBuf> {
    let mut dirs = vec![ctx.project_root.clone()];
    if let Some(target) = target_path {
        let mut dir = ctx.project_root.clone();
        // Only walk down through normal components so a `..`/absolute path
        // can't escape the project root.
        for component in Path::new(target).components() {
            if let Component::Normal(part) = component {
                dir = dir.join(part);
                dirs.push(dir.clone());
            }
        }
    }
    dirs.into_iter()
        .filter_map(|dir| instruction_file(&dir))
        .collect()
}

fn instruction_file(dir: &Path) -> Option<PathBuf> {
    ["AGENTS.override.md", "AGENTS.md"]
        .into_iter()
        .map(|name| dir.join(name))
        .find(|path| path.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_project_agents_md() {
        let project = tempfile::tempdir().unwrap();
        std::fs::write(project.path().join("AGENTS.md"), "  project rules  \n").unwrap();

        let out = run(
            LoadProjectInstructionsArgs {
                max_bytes: None,
                target_path: None,
            },
            McpToolCtx {
                project_root: project.path().to_path_buf(),
            },
        )
        .unwrap();

        assert!(out.contains("# Project instructions from AGENTS.md"));
        assert!(out.contains("project rules"));
    }

    #[test]
    fn override_wins_over_agents_md() {
        let project = tempfile::tempdir().unwrap();
        std::fs::write(project.path().join("AGENTS.md"), "base").unwrap();
        std::fs::write(project.path().join("AGENTS.override.md"), "override").unwrap();

        let out = run(
            LoadProjectInstructionsArgs {
                max_bytes: None,
                target_path: None,
            },
            McpToolCtx {
                project_root: project.path().to_path_buf(),
            },
        )
        .unwrap();

        assert!(out.contains("AGENTS.override.md"));
        assert!(out.contains("override"));
        assert!(!out.contains("base"));
    }

    #[test]
    fn caps_returned_bytes() {
        let project = tempfile::tempdir().unwrap();
        std::fs::write(project.path().join("AGENTS.md"), "abcdef").unwrap();

        let out = run(
            LoadProjectInstructionsArgs {
                max_bytes: Some(3),
                target_path: None,
            },
            McpToolCtx {
                project_root: project.path().to_path_buf(),
            },
        )
        .unwrap();

        assert!(out.contains("abc"));
        assert!(out.contains("truncated after 3 bytes"));
        assert!(!out.contains("abcdef"));
    }

    #[test]
    fn target_path_includes_nested_agents_chain() {
        let project = tempfile::tempdir().unwrap();
        std::fs::write(project.path().join("AGENTS.md"), "root rules").unwrap();
        let nested = project.path().join("crates").join("tui");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("AGENTS.md"), "tui-local rules").unwrap();

        let out = run(
            LoadProjectInstructionsArgs {
                max_bytes: None,
                target_path: Some("crates/tui/src/main.rs".to_string()),
            },
            McpToolCtx {
                project_root: project.path().to_path_buf(),
            },
        )
        .unwrap();

        // Root first, nested last (more specific overrides win for the reader).
        assert!(out.contains("root rules"));
        assert!(out.contains("tui-local rules"));
        let root_at = out.find("root rules").unwrap();
        let nested_at = out.find("tui-local rules").unwrap();
        assert!(root_at < nested_at);
    }

    #[test]
    fn target_path_cannot_escape_project_root() {
        let project = tempfile::tempdir().unwrap();
        std::fs::write(project.path().join("AGENTS.md"), "root rules").unwrap();

        let out = run(
            LoadProjectInstructionsArgs {
                max_bytes: None,
                target_path: Some("../../etc/passwd".to_string()),
            },
            McpToolCtx {
                project_root: project.path().to_path_buf(),
            },
        )
        .unwrap();

        // `..` components are ignored, so only the root file is returned.
        assert!(out.contains("root rules"));
    }
}
