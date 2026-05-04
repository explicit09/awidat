//! Bundled indexer registry.
//!
//! Awidat ships with 10 canonical indexers pre-registered. New
//! projects work with zero `[[mcp.servers]]` config — the user's
//! `.awidat/config.toml` (and `~/.config/awidat/config.toml`) become
//! purely *additive overlays*: add custom indexers, swap a model,
//! or `enabled = false` to disable a default.
//!
//! ## Resolution model
//!
//! Two paths need to be discovered at default-load time:
//!
//! - **Python workspace root** — where the bundled `<name>-mcp`
//!   packages live. Resolved in priority order:
//!     1. `AWIDAT_PYTHON_ROOT` env var (explicit override; dev use)
//!     2. Walk up from the awidat binary looking for `python/`
//!     3. Documented install location: `<prefix>/share/awidat/python`
//!     4. Fall back to a literal-empty path; defaults are still
//!        registered, but launching them will fail with a clear error
//!        (`uv` won't find the package). The user can fix by setting
//!        the env var or installing from a packaged release.
//!
//! - **`uv` executable** — `which uv` first, else
//!   `~/.local/bin/uv` (uv's documented install path on macOS/Linux),
//!   else fall back to the literal `"uv"` (assumes PATH at launch).
//!
//! ## Why this lives in awidat-config and not awidat-cli
//!
//! Multiple consumers need the registry: the CLI's `awidat index`,
//! the TUI command's session creation, and any future MCP-host
//! integration test fixture. Putting it under config keeps the data-
//! not-code rule intact (the engine is still data-driven; we just
//! provide better defaults *as data* every install gets).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::{McpConfig, McpServer, McpServerKind};

/// Resolve the python workspace root per the priority model in the
/// module doc. Returns `None` only when every fallback misses; in
/// that case the defaults still register but launching them errors.
pub fn python_root() -> Option<PathBuf> {
    if let Ok(env) = std::env::var("AWIDAT_PYTHON_ROOT") {
        let p = PathBuf::from(env);
        if p.exists() {
            return Some(p);
        }
        // The user explicitly set the var. Trust them — don't fall
        // back. They'll get a clear "package not found" if it's
        // wrong, which is better than silently using a different
        // python tree.
        return Some(p);
    }
    // Walk up from the binary location, then from the current dir.
    // Either should land on the awidat repo root in a dev install.
    if let Ok(exe) = std::env::current_exe()
        && let Some(parent) = exe.parent()
        && let Some(found) = walk_up_for_python(parent)
    {
        return Some(found);
    }
    if let Ok(cwd) = std::env::current_dir()
        && let Some(found) = walk_up_for_python(&cwd)
    {
        return Some(found);
    }
    // Documented install locations. Try Homebrew prefix first since
    // that's where `brew install awidat` would land it.
    for candidate in [
        "/opt/homebrew/share/awidat/python",
        "/usr/local/share/awidat/python",
    ] {
        let p = PathBuf::from(candidate);
        if p.exists() {
            return Some(p);
        }
    }
    if let Some(home) = dirs::home_dir() {
        let p = home.join(".local/share/awidat/python");
        if p.exists() {
            return Some(p);
        }
    }
    None
}

/// Walk up from `start` looking for a directory containing a
/// `python/packages/awidat-mcp/pyproject.toml` — the unambiguous
/// marker of the awidat python workspace.
fn walk_up_for_python(start: &Path) -> Option<PathBuf> {
    let mut cur: Option<&Path> = Some(start);
    while let Some(dir) = cur {
        let candidate = dir.join("python");
        if candidate.join("packages/awidat-mcp/pyproject.toml").exists() {
            return Some(candidate);
        }
        cur = dir.parent();
    }
    None
}

/// Resolve the `uv` executable path. Returns the absolute path when
/// findable; falls back to the literal `"uv"` (assumes PATH).
pub fn uv_command() -> String {
    // `which uv` — the canonical resolver. We don't shell out to
    // `which`; we walk PATH ourselves to keep startup deterministic
    // and avoid a process spawn on every Config::load.
    if let Ok(path_var) = std::env::var("PATH") {
        for dir in path_var.split(':') {
            let candidate = PathBuf::from(dir).join("uv");
            if candidate.is_file() {
                return candidate.to_string_lossy().into_owned();
            }
        }
    }
    if let Some(home) = dirs::home_dir() {
        let candidate = home.join(".local/bin/uv");
        if candidate.is_file() {
            return candidate.to_string_lossy().into_owned();
        }
    }
    "uv".to_string()
}

/// One bundled indexer recipe.
struct IndexerRecipe {
    name: &'static str,
    package: &'static str,
    /// Optional baseline env vars (e.g. `WHISPER_MODEL = "small.en"`).
    env: &'static [(&'static str, &'static str)],
}

/// The canonical list. Adding a new indexer to the awidat install
/// is a one-line edit here, no engine changes — the data-not-code
/// rule per `INDEX_SCHEMA.md`. Order is the dispatch order
/// (whisper before topic before editorial-moments because of the
/// dependency chain; visual indexers after).
const RECIPES: &[IndexerRecipe] = &[
    IndexerRecipe {
        name: "audio-energy",
        package: "audio-energy-mcp",
        env: &[],
    },
    IndexerRecipe {
        name: "scenedetect",
        package: "scenedetect-mcp",
        env: &[],
    },
    IndexerRecipe {
        name: "whisper",
        package: "whisper-mcp",
        // Default to the small model; users override via global/project
        // config or via setting the env directly.
        env: &[("WHISPER_MODEL", "small.en")],
    },
    IndexerRecipe {
        name: "topic",
        package: "topic-mcp",
        env: &[],
    },
    IndexerRecipe {
        name: "editorial-moments",
        package: "editorial-moments-mcp",
        env: &[],
    },
    IndexerRecipe {
        name: "clip",
        package: "clip-mcp",
        env: &[],
    },
    IndexerRecipe {
        name: "face",
        package: "face-mcp",
        env: &[],
    },
    IndexerRecipe {
        name: "shot",
        package: "shot-mcp",
        env: &[],
    },
    IndexerRecipe {
        name: "gaze",
        package: "gaze-mcp",
        env: &[],
    },
    IndexerRecipe {
        name: "frame-quality",
        package: "frame-quality-mcp",
        env: &[],
    },
];

/// Materialize the bundled defaults into a [`crate::Config`]. Called
/// from [`crate::Config::load`].
///
/// Resolves `uv` and the python workspace root *once* per call. If
/// the python root can't be found, the entries are still registered
/// (so disable-via-overlay still works); their `cwd` is left unset
/// and launches will fail with a clear "package not found" error.
pub fn with_defaults() -> crate::Config {
    let cmd = uv_command();
    let python_cwd = python_root();

    let servers = RECIPES
        .iter()
        .map(|recipe| {
            let env: HashMap<String, String> = recipe
                .env
                .iter()
                .map(|(k, v)| ((*k).into(), (*v).into()))
                .collect();
            McpServer {
                name: recipe.name.into(),
                command: cmd.clone(),
                args: vec![
                    "run".into(),
                    "--package".into(),
                    recipe.package.into(),
                    recipe.package.into(),
                ],
                env,
                cwd: python_cwd.clone(),
                kind: McpServerKind::Indexer,
                enabled: true,
            }
        })
        .collect();

    crate::Config {
        mcp: McpConfig { servers },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ten_default_indexers_registered() {
        let cfg = with_defaults();
        assert_eq!(cfg.mcp.servers.len(), 10);
        let names: Vec<&str> = cfg.mcp.servers.iter().map(|s| s.name.as_str()).collect();
        // Spot-check that the headline indexers are present.
        assert!(names.contains(&"whisper"));
        assert!(names.contains(&"clip"));
        assert!(names.contains(&"editorial-moments"));
        assert!(names.contains(&"face"));
    }

    #[test]
    fn defaults_are_all_enabled() {
        let cfg = with_defaults();
        assert!(cfg.mcp.servers.iter().all(|s| s.enabled));
    }

    #[test]
    fn defaults_are_indexer_kind() {
        let cfg = with_defaults();
        assert!(
            cfg.mcp
                .servers
                .iter()
                .all(|s| s.kind == McpServerKind::Indexer)
        );
    }

    #[test]
    fn whisper_default_env_includes_small_model() {
        let cfg = with_defaults();
        let whisper = cfg.find_server("whisper").unwrap();
        assert_eq!(
            whisper.env.get("WHISPER_MODEL").map(String::as_str),
            Some("small.en")
        );
    }

    #[test]
    fn args_use_uv_run_package_pattern() {
        let cfg = with_defaults();
        let clip = cfg.find_server("clip").unwrap();
        assert_eq!(clip.args, vec!["run", "--package", "clip-mcp", "clip-mcp"]);
    }

    // Note: `python_root()` reads `AWIDAT_PYTHON_ROOT` directly. We
    // don't have a unit test that mutates that env var because the
    // workspace forbids `unsafe { std::env::set_var(...) }` and
    // `set_var` is unsafe in 2024-edition Rust. The integration test
    // for the resolution path runs as part of the CLI smoke (where
    // we can set the env in the spawned process).
}
