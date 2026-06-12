//! `load_project_instructions` — progressive project-doc access.
//!
//! Codex can inject AGENTS.md automatically, but Montage keeps the first
//! turn lean and exposes project docs on demand through this read-only MCP
//! tool instead.

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
}

pub fn run(args: LoadProjectInstructionsArgs, ctx: McpToolCtx) -> Result<String, String> {
    let max_bytes = args
        .max_bytes
        .unwrap_or(DEFAULT_MAX_BYTES)
        .min(HARD_MAX_BYTES);
    let Some(path) = instruction_path(&ctx) else {
        return Ok("No project AGENTS.md or AGENTS.override.md found.".to_string());
    };
    let bytes = std::fs::read(&path)
        .map_err(|e| format!("load_project_instructions: read {}: {e}", path.display()))?;
    let truncated = bytes.len() > max_bytes;
    let visible_len = bytes.len().min(max_bytes);
    let body = String::from_utf8_lossy(&bytes[..visible_len]);
    let mut out = format!(
        "# Project instructions from {}\n\n{}",
        path.strip_prefix(&ctx.project_root)
            .unwrap_or(path.as_path())
            .display(),
        body.trim()
    );
    if truncated {
        out.push_str(&format!(
            "\n\n[truncated after {max_bytes} bytes; request a higher max_bytes if needed]"
        ));
    }
    Ok(out)
}

fn instruction_path(ctx: &McpToolCtx) -> Option<std::path::PathBuf> {
    ["AGENTS.override.md", "AGENTS.md"]
        .into_iter()
        .map(|name| ctx.project_root.join(name))
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
            LoadProjectInstructionsArgs { max_bytes: None },
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
            LoadProjectInstructionsArgs { max_bytes: None },
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
            LoadProjectInstructionsArgs { max_bytes: Some(3) },
            McpToolCtx {
                project_root: project.path().to_path_buf(),
            },
        )
        .unwrap();

        assert!(out.contains("abc"));
        assert!(out.contains("truncated after 3 bytes"));
        assert!(!out.contains("abcdef"));
    }
}
