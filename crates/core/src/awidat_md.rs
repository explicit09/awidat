//! `AWIDAT.md` discovery — hand-curated prose context the agent loads
//! at session start.
//!
//! The Codex AGENTS.md / Claude Code CLAUDE.md pattern, ported. Both
//! harnesses converge on a Markdown file at the repo root as their
//! lightest-weight "index": producers / users write down conventions
//! once ("never cut on a breath", "speaker A is the host", "preferred
//! pacing is 90 bpm"), the agent reads them every session.
//!
//! The research note `docs/research/harness-learnings/indexing-research.md`
//! captures the cross-harness convergence: every coding harness with
//! a public design ships some flavor of this file, even the ones with
//! full vector indexes. It's the operationally simplest possible
//! "index" — lossy by design, hand-curated, human-readable, debuggable,
//! free of any sync infrastructure. We're stealing it directly.
//!
//! # Discovery order
//!
//! Walked top-down at session start, concatenated. Files closer to
//! the project root override earlier (more general) ones because
//! language models weight recent context more heavily.
//!
//! 1. **User scope** — `~/.awidat/AWIDAT.override.md`, falling back to
//!    `~/.awidat/AWIDAT.md`. Personal preferences across all projects
//!    ("always render at 1080p preview", "I like 90s clip durations").
//! 2. **Project scope** — `<project_root>/AWIDAT.md`. Per-project
//!    convention checked into source ("this is Joe Rogan's podcast,
//!    speaker A is Joe", "never cut Joe's questions short").
//! 3. **Local scope** — `<project_root>/AWIDAT.local.md`. Gitignored;
//!    per-session preferences that shouldn't ship to teammates.
//!
//! At each layer, an `AWIDAT.override.md` next to (or instead of) the
//! base file fully replaces the layer for temporary experimentation.
//!
//! # Cap
//!
//! Combined output capped at 32 KiB (matches Codex's
//! `project_doc_max_bytes` default). Files are loaded in discovery
//! order; once the cap is reached subsequent files are dropped with
//! a warning. Producers should keep AWIDAT.md tight — bloated context
//! files dilute the model's attention to what matters.

use std::path::{Path, PathBuf};

/// Maximum total size of all loaded AWIDAT.md files combined. Mirrors
/// Codex's `project_doc_max_bytes = 32 KiB` default.
pub const MAX_BYTES: usize = 32 * 1024;

const FILE_BASE: &str = "AWIDAT.md";
const FILE_OVERRIDE: &str = "AWIDAT.override.md";
const FILE_LOCAL: &str = "AWIDAT.local.md";
const SEP: &str = "\n\n--- awidat-doc ---\n\n";

/// Discovery summary: the concatenated text the system prompt should
/// receive, plus the source paths (for telemetry / debugging).
#[derive(Debug, Default, Clone)]
pub struct AwidatDocs {
    /// Concatenated content of every loaded file, separated by
    /// `--- awidat-doc ---`. Empty when no files were found.
    pub text: String,
    /// Absolute paths of every file that contributed (in load order).
    pub sources: Vec<PathBuf>,
    /// Total bytes of `text` (after truncation if any).
    pub bytes: usize,
    /// Set when one or more files were dropped because the cap was
    /// reached. Caller may surface this in the TUI.
    pub truncated: bool,
}

/// Discover and concatenate AWIDAT.md docs.
///
/// `project_root` should be the project directory we're editing in;
/// `home` is the user's home dir (`std::env::home_dir()`-shaped).
/// Pass `None` for `home` to skip user-scope files (useful in tests).
pub fn discover(project_root: &Path, home: Option<&Path>) -> AwidatDocs {
    let mut docs = AwidatDocs::default();
    let mut layers = Vec::<PathBuf>::new();

    // Layer 1: user scope (~/.awidat/{AWIDAT.override.md,AWIDAT.md}).
    // Override wins when present.
    if let Some(home) = home {
        let user_dir = home.join(".awidat");
        let override_p = user_dir.join(FILE_OVERRIDE);
        let base_p = user_dir.join(FILE_BASE);
        if override_p.exists() {
            layers.push(override_p);
        } else if base_p.exists() {
            layers.push(base_p);
        }
    }

    // Layer 2: project scope (<root>/AWIDAT.md OR
    // <root>/AWIDAT.override.md if the user wants to temporarily
    // replace).
    let project_override = project_root.join(FILE_OVERRIDE);
    let project_base = project_root.join(FILE_BASE);
    if project_override.exists() {
        layers.push(project_override);
    } else if project_base.exists() {
        layers.push(project_base);
    }

    // Layer 3: local scope (<root>/AWIDAT.local.md). Always loaded
    // when present, layered on top of project scope. Producers add
    // this to .gitignore.
    let local_p = project_root.join(FILE_LOCAL);
    if local_p.exists() {
        layers.push(local_p);
    }

    for path in layers {
        let Ok(raw) = std::fs::read_to_string(&path) else {
            continue;
        };
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        // Add the separator before subsequent layers (not before the
        // first one).
        let prefix_len = if docs.text.is_empty() {
            0
        } else {
            SEP.len()
        };
        if docs.bytes + prefix_len + trimmed.len() > MAX_BYTES {
            docs.truncated = true;
            break;
        }
        if !docs.text.is_empty() {
            docs.text.push_str(SEP);
            docs.bytes += SEP.len();
        }
        docs.text.push_str(trimmed);
        docs.bytes += trimmed.len();
        docs.sources.push(path);
    }

    docs
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(path: &Path, body: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, body).unwrap();
    }

    #[test]
    fn empty_tree_returns_empty_docs() {
        let dir = tempfile::tempdir().unwrap();
        let docs = discover(dir.path(), None);
        assert!(docs.text.is_empty());
        assert!(docs.sources.is_empty());
        assert_eq!(docs.bytes, 0);
        assert!(!docs.truncated);
    }

    #[test]
    fn project_only_loads_project_md() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("AWIDAT.md"), "host = speaker A");
        let docs = discover(dir.path(), None);
        assert_eq!(docs.text, "host = speaker A");
        assert_eq!(docs.sources.len(), 1);
        assert!(docs.sources[0].ends_with("AWIDAT.md"));
    }

    #[test]
    fn user_and_project_concatenate_user_first() {
        let home = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        write(
            &home.path().join(".awidat/AWIDAT.md"),
            "always render 1080p preview",
        );
        write(&project.path().join("AWIDAT.md"), "host = speaker A");
        let docs = discover(project.path(), Some(home.path()));
        assert!(docs.text.starts_with("always render 1080p preview"));
        assert!(docs.text.contains("host = speaker A"));
        assert!(docs.text.contains("--- awidat-doc ---"));
        assert_eq!(docs.sources.len(), 2);
    }

    #[test]
    fn override_replaces_base_at_user_scope() {
        let home = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        write(&home.path().join(".awidat/AWIDAT.md"), "base content");
        write(
            &home.path().join(".awidat/AWIDAT.override.md"),
            "override content",
        );
        let docs = discover(project.path(), Some(home.path()));
        assert!(docs.text.contains("override content"));
        assert!(!docs.text.contains("base content"));
    }

    #[test]
    fn local_md_layers_on_top_of_project_md() {
        let project = tempfile::tempdir().unwrap();
        write(&project.path().join("AWIDAT.md"), "team conventions");
        write(
            &project.path().join("AWIDAT.local.md"),
            "my sandbox preferences",
        );
        let docs = discover(project.path(), None);
        // Project content should appear before local content.
        let team_idx = docs.text.find("team conventions").unwrap();
        let local_idx = docs.text.find("my sandbox preferences").unwrap();
        assert!(team_idx < local_idx);
        assert_eq!(docs.sources.len(), 2);
    }

    #[test]
    fn empty_files_are_skipped() {
        let project = tempfile::tempdir().unwrap();
        write(&project.path().join("AWIDAT.md"), "real content");
        write(&project.path().join("AWIDAT.local.md"), "   \n\n   ");
        let docs = discover(project.path(), None);
        assert_eq!(docs.text, "real content");
        assert_eq!(docs.sources.len(), 1);
    }

    #[test]
    fn cap_truncates_and_marks_truncated() {
        let project = tempfile::tempdir().unwrap();
        // 30KB of project content fits; a 10KB local file should
        // overflow the 32KB cap and be dropped.
        let big = "a".repeat(30 * 1024);
        let extra = "b".repeat(10 * 1024);
        write(&project.path().join("AWIDAT.md"), &big);
        write(&project.path().join("AWIDAT.local.md"), &extra);
        let docs = discover(project.path(), None);
        assert!(docs.bytes <= MAX_BYTES);
        assert!(docs.truncated, "expected truncation flag");
        assert!(docs.text.contains(&big[..100]));
        assert!(
            !docs.text.contains(&extra[..100]),
            "overflowing local file must be dropped"
        );
        // Only the first file made it.
        assert_eq!(docs.sources.len(), 1);
    }

    #[test]
    fn project_override_replaces_project_base() {
        let project = tempfile::tempdir().unwrap();
        write(&project.path().join("AWIDAT.md"), "checked-in convention");
        write(
            &project.path().join("AWIDAT.override.md"),
            "experimental override",
        );
        let docs = discover(project.path(), None);
        assert!(docs.text.contains("experimental override"));
        assert!(!docs.text.contains("checked-in convention"));
    }
}
