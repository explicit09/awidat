//! Awidat config: TOML at two locations, project overrides global.
//!
//! - **Global**: `~/.config/awidat/config.toml` (XDG; on macOS the same path
//!   under `~/.config/` is used by convention even though XDG isn't native).
//! - **Project**: `<project>/.awidat/config.toml` (per-project overrides).
//!
//! The merge rule is "project entirely replaces global per top-level key."
//! For `[[mcp.servers]]` (the only collection in v1), entries with matching
//! `name` in the project override the global entry; new names append.
//!
//! See `crates/config/EXAMPLE.toml` for the canonical shape.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::trace;

/// Errors loading or parsing config.
#[derive(Debug, Error)]
pub enum ConfigError {
    /// I/O reading a config file.
    #[error("I/O error reading config '{path}': {source}")]
    Io {
        /// File that failed.
        path: String,
        /// Underlying.
        #[source]
        source: std::io::Error,
    },

    /// TOML parse error.
    #[error("malformed TOML in '{path}': {message}")]
    Parse {
        /// File that failed.
        path: String,
        /// Diagnostic.
        message: String,
    },

    /// HOME / XDG_CONFIG_HOME not resolvable. Rare.
    #[error("cannot locate user config dir; set XDG_CONFIG_HOME or HOME")]
    NoUserConfigDir,
}

/// Top-level config shape. Both global and project files deserialize into
/// this; merge happens via [`Config::overlay`].
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Config {
    /// MCP server registrations (indexers + week-4 agent tools).
    #[serde(default)]
    pub mcp: McpConfig,
}

/// `[mcp]` section.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct McpConfig {
    /// Registered MCP servers. Each is launched on demand by the engine.
    #[serde(default)]
    pub servers: Vec<McpServer>,
}

/// One MCP server registration. Mirrors `awidat_mcp::ServerConfig` but is
/// the *config* shape (de/serializable) — the engine converts to
/// `ServerConfig` at launch time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServer {
    /// Logical id. Doubles as the indexer name in `index/<name>/` for
    /// indexer-shaped servers.
    pub name: String,
    /// Executable to run.
    pub command: String,
    /// Arguments after the command.
    #[serde(default)]
    pub args: Vec<String>,
    /// Extra env vars set on the child.
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// Optional working directory. If relative, resolved against the file
    /// the entry came from at load time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
    /// What kind of server this is. Indexers have a known tool surface
    /// (`index_asset`); generic agent tools are anything else. v1 only has
    /// indexers.
    #[serde(default = "McpServerKind::default")]
    pub kind: McpServerKind,
}

/// Server-kind discriminator. We don't dispatch on this in v1 — it's
/// metadata for the engine and the agent. New kinds (e.g. `"sampling"`,
/// `"resource"`) slot in additively.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum McpServerKind {
    /// Footage indexer. Exposes `index_asset` per `INDEX_SCHEMA.md`.
    Indexer,
    /// Generic agent tool (Week 4+).
    Tool,
}

impl McpServerKind {
    /// Default kind for entries that don't specify (`indexer`).
    pub const fn default() -> Self {
        Self::Indexer
    }
}

impl Config {
    /// Load global + project, layering project on top of global.
    /// Either may be absent (returns its defaults).
    pub fn load(project_root: Option<&Path>) -> Result<Self, ConfigError> {
        let mut merged = match global_config_path()? {
            Some(p) if p.exists() => Self::load_file(&p)?,
            _ => Self::default(),
        };
        if let Some(root) = project_root {
            let project_path = project_config_path(root);
            if project_path.exists() {
                let project = Self::load_file(&project_path)?;
                merged = merged.overlay(project);
            }
        }
        Ok(merged)
    }

    /// Load a single TOML file.
    pub fn load_file(path: &Path) -> Result<Self, ConfigError> {
        trace!(?path, "loading config file");
        let text = std::fs::read_to_string(path).map_err(|e| ConfigError::Io {
            path: path.display().to_string(),
            source: e,
        })?;
        toml::from_str::<Self>(&text).map_err(|e| ConfigError::Parse {
            path: path.display().to_string(),
            message: e.to_string(),
        })
    }

    /// Layer `project` on top of `self`. Per-server merge: project entries
    /// matching a global server `name` replace the global entry; project
    /// entries with new names append.
    #[must_use]
    pub fn overlay(self, project: Self) -> Self {
        let mut servers = self.mcp.servers;
        for entry in project.mcp.servers {
            if let Some(existing) = servers.iter_mut().find(|s| s.name == entry.name) {
                *existing = entry;
            } else {
                servers.push(entry);
            }
        }
        Self {
            mcp: McpConfig { servers },
        }
    }

    /// Find an MCP server by name.
    pub fn find_server(&self, name: &str) -> Option<&McpServer> {
        self.mcp.servers.iter().find(|s| s.name == name)
    }

    /// Filter to indexer-kinded servers in registration order.
    pub fn indexers(&self) -> impl Iterator<Item = &McpServer> {
        self.mcp
            .servers
            .iter()
            .filter(|s| s.kind == McpServerKind::Indexer)
    }
}

/// Path to the global config file. `None` if no user config dir exists
/// (rare; unset HOME on Unix). Does *not* check the file actually exists.
pub fn global_config_path() -> Result<Option<PathBuf>, ConfigError> {
    let dir = dirs::config_dir().ok_or(ConfigError::NoUserConfigDir)?;
    Ok(Some(dir.join("awidat").join("config.toml")))
}

/// Path to the project-local config file.
#[must_use]
pub fn project_config_path(project_root: &Path) -> PathBuf {
    project_root.join(".awidat").join("config.toml")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, body: &str) {
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p).unwrap();
        }
        std::fs::write(path, body).unwrap();
    }

    #[test]
    fn empty_config_parses() {
        let c: Config = toml::from_str("").unwrap();
        assert!(c.mcp.servers.is_empty());
    }

    #[test]
    fn server_entry_round_trips() {
        let body = r#"
[[mcp.servers]]
name = "whisper"
command = "uv"
args = ["run", "--directory", "/abs/path/whisper-mcp", "whisper-mcp"]
kind = "indexer"

[mcp.servers.env]
WHISPER_MODEL = "small.en"
"#;
        let c: Config = toml::from_str(body).unwrap();
        assert_eq!(c.mcp.servers.len(), 1);
        let s = &c.mcp.servers[0];
        assert_eq!(s.name, "whisper");
        assert_eq!(s.command, "uv");
        assert_eq!(s.args.len(), 4);
        assert_eq!(s.env.get("WHISPER_MODEL").map(String::as_str), Some("small.en"));
        assert_eq!(s.kind, McpServerKind::Indexer);
    }

    #[test]
    fn overlay_replaces_matching_name_and_appends_new() {
        let global = Config {
            mcp: McpConfig {
                servers: vec![
                    McpServer {
                        name: "whisper".into(),
                        command: "uv".into(),
                        args: vec!["run".into(), "whisper-mcp".into()],
                        env: HashMap::new(),
                        cwd: None,
                        kind: McpServerKind::Indexer,
                    },
                    McpServer {
                        name: "scenedetect".into(),
                        command: "uv".into(),
                        args: vec!["run".into(), "scenedetect-mcp".into()],
                        env: HashMap::new(),
                        cwd: None,
                        kind: McpServerKind::Indexer,
                    },
                ],
            },
        };
        let mut project_env = HashMap::new();
        project_env.insert("WHISPER_MODEL".into(), "large-v3-turbo".into());
        let project = Config {
            mcp: McpConfig {
                servers: vec![
                    // Replaces global whisper.
                    McpServer {
                        name: "whisper".into(),
                        command: "uv".into(),
                        args: vec!["run".into(), "whisper-mcp-fast".into()],
                        env: project_env,
                        cwd: None,
                        kind: McpServerKind::Indexer,
                    },
                    // New entry, appended.
                    McpServer {
                        name: "audio-energy".into(),
                        command: "uv".into(),
                        args: vec!["run".into(), "audio-energy-mcp".into()],
                        env: HashMap::new(),
                        cwd: None,
                        kind: McpServerKind::Indexer,
                    },
                ],
            },
        };
        let merged = global.overlay(project);
        let names: Vec<&str> = merged.mcp.servers.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["whisper", "scenedetect", "audio-energy"]);
        let whisper = merged.find_server("whisper").unwrap();
        assert_eq!(whisper.args, vec!["run", "whisper-mcp-fast"]);
        assert_eq!(
            whisper.env.get("WHISPER_MODEL").map(String::as_str),
            Some("large-v3-turbo")
        );
    }

    #[test]
    fn load_with_no_files_returns_default() {
        // No global, no project — Config::load() returns default.
        let c = Config::load(None).unwrap();
        assert!(c.mcp.servers.is_empty());
    }

    #[test]
    fn load_project_only() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = project_config_path(dir.path());
        write(
            &cfg,
            r#"
[[mcp.servers]]
name = "whisper"
command = "uv"
args = ["run", "whisper-mcp"]
"#,
        );
        let c = Config::load(Some(dir.path())).unwrap();
        assert_eq!(c.mcp.servers.len(), 1);
        assert_eq!(c.mcp.servers[0].name, "whisper");
    }

    #[test]
    fn malformed_toml_is_a_parse_error() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = project_config_path(dir.path());
        write(&cfg, "this = is = not = toml");
        let err = Config::load(Some(dir.path())).unwrap_err();
        assert!(matches!(err, ConfigError::Parse { .. }), "{err}");
    }

    #[test]
    fn indexers_iter_filters_by_kind() {
        let c = Config {
            mcp: McpConfig {
                servers: vec![
                    McpServer {
                        name: "whisper".into(),
                        command: "uv".into(),
                        args: vec![],
                        env: HashMap::new(),
                        cwd: None,
                        kind: McpServerKind::Indexer,
                    },
                    McpServer {
                        name: "bash".into(),
                        command: "/bin/bash".into(),
                        args: vec![],
                        env: HashMap::new(),
                        cwd: None,
                        kind: McpServerKind::Tool,
                    },
                ],
            },
        };
        let names: Vec<&str> = c.indexers().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["whisper"]);
    }
}
