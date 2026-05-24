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

#![cfg_attr(test, allow(clippy::unwrap_used))]

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::trace;

pub mod defaults;

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
    /// Editorial hooks. Shell commands fired at lifecycle points
    /// (`pre_apply_edl`, `post_apply_edl`). Per PLAN.md §15 Week 7.
    #[serde(default)]
    pub hooks: HooksConfig,
}

/// `[hooks]` section. Each field is a shell command (run via
/// `bash -lc`); empty / unset means no hook for that event.
///
/// Pre-hooks can fail and abort the operation: a non-zero exit
/// status surfaces as an apply_edl error to the agent.
/// Post-hooks fire fire-and-forget; their stderr is captured into
/// the SessionEvent stream for diagnostics but doesn't block the
/// agent.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct HooksConfig {
    /// Run before each `apply_edl` commit. Receives the EDL text
    /// on stdin (so the hook can lint / reject); the hook's
    /// stdout is logged but ignored. Non-zero exit aborts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pre_apply_edl: Option<String>,
    /// Run after each successful `apply_edl` commit. Receives
    /// `{"applied": [<op summaries>]}` on stdin. Side-effect-only;
    /// exit status is logged but doesn't block.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub post_apply_edl: Option<String>,
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
    /// Whether this entry is active. Defaults to `true`. Setting it to
    /// `false` in user config disables a default-bundled indexer
    /// (e.g. `whisper` when the user has no HF_TOKEN and doesn't want
    /// repeated diarization-init failures). Disabled entries are
    /// skipped by the indexer dispatcher and the MCP host.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Names of other indexers this one reads sidecars from. The
    /// dispatcher topo-sorts on this graph: an indexer doesn't run
    /// until every name in `depends_on` has produced its sidecar
    /// successfully. If a producer fails, dependents are skipped
    /// (with a clean message) rather than run-and-error. Defaults
    /// to empty for user-added indexers (today's behavior); the
    /// bundled defaults populate this for `topic` (→ whisper),
    /// `editorial-moments` (→ whisper, topic), `gaze` (→ face),
    /// and `shot` (→ scenedetect, face, gaze).
    #[serde(default)]
    pub depends_on: Vec<String>,
    /// Coarse resource class the index dispatcher uses to avoid
    /// memory-heavy indexers running together. Missing user config
    /// defaults to `light` for backwards compatibility.
    #[serde(default)]
    pub resource_class: IndexerResourceClass,
    /// Optional future-facing profile group. Milestone 1 records this
    /// as metadata only; no CLI or desktop surface dispatches on it yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub indexer_group: Option<IndexerGroup>,
}

fn default_true() -> bool {
    true
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

/// Resource class for indexer scheduling. This is intentionally coarse:
/// the dispatcher uses it to keep memory-heavy processes from overlapping,
/// not to model exact RAM usage.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum IndexerResourceClass {
    /// Memory-heavy model processes that should not overlap with each other.
    Exclusive,
    /// Vision passes that tend to decode video and load native/ML deps.
    Vision,
    /// Lightweight CPU/pixel/audio passes that can usually run alongside
    /// non-exclusive work.
    #[default]
    Light,
    /// Network/API-bound indexers.
    Network,
    /// Local embedding/model passes that are lighter than the exclusive set.
    Embedding,
}

/// Future-facing indexer grouping for progressive indexing profiles. Stored
/// as metadata in milestone 1; no runtime behavior depends on this yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum IndexerGroup {
    /// Fast navigation signals: transcript, audio energy, shots, topics.
    Navigation,
    /// Visual search and on-screen subject signals.
    Vision,
    /// Higher-level editorial judgment signals.
    Editorial,
    /// Frame/color/audio quality signals.
    Quality,
}

impl Config {
    /// Load: defaults → global → project. Each layer overlays the
    /// previous via [`Self::overlay`].
    ///
    /// The defaults layer registers the bundled indexer servers
    /// (whisper, clip, face, etc.) so a freshly-init'd project works
    /// with zero user config. User config becomes additive: add new
    /// servers, override fields on a default server (e.g. swap
    /// whisper-large for whisper-tiny), or set `enabled = false` to
    /// turn one off. See [`defaults`].
    pub fn load(project_root: Option<&Path>) -> Result<Self, ConfigError> {
        let mut merged = defaults::with_defaults();
        if let Some(p) = global_config_path()?
            && p.exists()
        {
            merged = merged.overlay(Self::load_file(&p)?);
        }
        if let Some(root) = project_root {
            let project_path = project_config_path(root);
            if project_path.exists() {
                merged = merged.overlay(Self::load_file(&project_path)?);
            }
        }
        Ok(merged)
    }

    /// Variant of [`Self::load`] that skips the bundled defaults.
    /// Used by tests that want to assert against a clean baseline,
    /// and by tooling that wants to print only user-supplied config.
    pub fn load_without_defaults(project_root: Option<&Path>) -> Result<Self, ConfigError> {
        let mut merged = match global_config_path()? {
            Some(p) if p.exists() => Self::load_file(&p)?,
            _ => Self::default(),
        };
        if let Some(root) = project_root {
            let project_path = project_config_path(root);
            if project_path.exists() {
                merged = merged.overlay(Self::load_file(&project_path)?);
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
        for mut entry in project.mcp.servers {
            if let Some(existing) = servers.iter_mut().find(|s| s.name == entry.name) {
                // Project overrides often replace only command/args/env for a bundled
                // indexer. Preserve bundled scheduler metadata when the override omits
                // the new fields (which deserialize to light/None for compatibility).
                if entry.resource_class == IndexerResourceClass::Light
                    && entry.indexer_group.is_none()
                    && existing.indexer_group.is_some()
                {
                    entry.resource_class = existing.resource_class;
                    entry.indexer_group = existing.indexer_group;
                }
                *existing = entry;
            } else {
                servers.push(entry);
            }
        }
        Self {
            mcp: McpConfig { servers },
            hooks: HooksConfig::default(),
        }
    }

    /// Find an MCP server by name.
    pub fn find_server(&self, name: &str) -> Option<&McpServer> {
        self.mcp.servers.iter().find(|s| s.name == name)
    }

    /// Filter to enabled indexer-kinded servers in registration order.
    pub fn indexers(&self) -> impl Iterator<Item = &McpServer> {
        self.mcp
            .servers
            .iter()
            .filter(|s| s.kind == McpServerKind::Indexer && s.enabled)
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
        assert_eq!(
            s.env.get("WHISPER_MODEL").map(String::as_str),
            Some("small.en")
        );
        assert_eq!(s.kind, McpServerKind::Indexer);
        assert_eq!(s.resource_class, IndexerResourceClass::Light);
        assert_eq!(s.indexer_group, None);
    }

    #[test]
    fn server_entry_parses_resource_metadata() {
        let body = r#"
[[mcp.servers]]
name = "clip"
command = "uv"
resource_class = "exclusive"
indexer_group = "vision"
"#;
        let c: Config = toml::from_str(body).unwrap();
        let s = &c.mcp.servers[0];
        assert_eq!(s.resource_class, IndexerResourceClass::Exclusive);
        assert_eq!(s.indexer_group, Some(IndexerGroup::Vision));
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
                        enabled: true,
                        depends_on: vec![],
                        resource_class: IndexerResourceClass::Light,
                        indexer_group: None,
                    },
                    McpServer {
                        name: "scenedetect".into(),
                        command: "uv".into(),
                        args: vec!["run".into(), "scenedetect-mcp".into()],
                        env: HashMap::new(),
                        cwd: None,
                        kind: McpServerKind::Indexer,
                        enabled: true,
                        depends_on: vec![],
                        resource_class: IndexerResourceClass::Light,
                        indexer_group: None,
                    },
                ],
            },
            hooks: HooksConfig::default(),
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
                        enabled: true,
                        depends_on: vec![],
                        resource_class: IndexerResourceClass::Light,
                        indexer_group: None,
                    },
                    // New entry, appended.
                    McpServer {
                        name: "audio-energy".into(),
                        command: "uv".into(),
                        args: vec!["run".into(), "audio-energy-mcp".into()],
                        env: HashMap::new(),
                        cwd: None,
                        kind: McpServerKind::Indexer,
                        enabled: true,
                        depends_on: vec![],
                        resource_class: IndexerResourceClass::Light,
                        indexer_group: None,
                    },
                ],
            },
            hooks: HooksConfig::default(),
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
    fn load_with_no_files_returns_just_defaults() {
        // No global, no project — Config::load() returns the
        // bundled defaults.
        let c = Config::load(None).unwrap();
        assert_eq!(c.mcp.servers.len(), 12);
        assert!(c.find_server("whisper").is_some());
        assert!(c.find_server("clip").is_some());
    }

    #[test]
    fn load_without_defaults_with_no_files_returns_empty() {
        // The escape hatch: skip defaults entirely.
        let c = Config::load_without_defaults(None).unwrap();
        assert!(c.mcp.servers.is_empty());
    }

    #[test]
    fn load_project_overrides_a_default_server() {
        // The headline overlay behavior: a project config entry with
        // matching name replaces the corresponding default. New names
        // append to the merged list.
        let dir = tempfile::tempdir().unwrap();
        let cfg = project_config_path(dir.path());
        write(
            &cfg,
            r#"
[[mcp.servers]]
name = "whisper"
command = "/custom/uv"
args = ["run", "whisper-tiny"]

[[mcp.servers]]
name = "my-custom-tool"
command = "uv"
args = ["run", "custom-mcp"]
"#,
        );
        let c = Config::load(Some(dir.path())).unwrap();
        // Originally 12 defaults; project added one new ("my-custom-tool").
        assert_eq!(c.mcp.servers.len(), 13);
        let whisper = c.find_server("whisper").unwrap();
        // Project config replaced the default whisper command/args.
        assert_eq!(whisper.command, "/custom/uv");
        assert_eq!(whisper.args, vec!["run", "whisper-tiny"]);
        // Scheduler metadata from the bundled entry survives command-only
        // overrides so heavy defaults don't silently become `light`.
        assert_eq!(whisper.resource_class, IndexerResourceClass::Exclusive);
        assert_eq!(whisper.indexer_group, Some(IndexerGroup::Navigation));
        assert!(c.find_server("my-custom-tool").is_some());
    }

    #[test]
    fn project_can_disable_a_default_indexer() {
        // The "I don't have HF_TOKEN" use case: turn off whisper
        // without removing it from the registry. Disabled entries
        // are filtered out of `indexers()`.
        let dir = tempfile::tempdir().unwrap();
        let cfg = project_config_path(dir.path());
        write(
            &cfg,
            r#"
[[mcp.servers]]
name = "whisper"
command = "/dev/null"
enabled = false
"#,
        );
        let c = Config::load(Some(dir.path())).unwrap();
        let names: Vec<&str> = c.indexers().map(|s| s.name.as_str()).collect();
        assert!(
            !names.contains(&"whisper"),
            "disabled whisper should not appear in indexers(); got {names:?}"
        );
        // But the entry itself is still present (find_server sees it),
        // so it survives further overlays without re-adding from defaults.
        assert!(c.find_server("whisper").is_some());
        assert!(!c.find_server("whisper").unwrap().enabled);
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
                        enabled: true,
                        depends_on: vec![],
                        resource_class: IndexerResourceClass::Light,
                        indexer_group: None,
                    },
                    McpServer {
                        name: "bash".into(),
                        command: "/bin/bash".into(),
                        args: vec![],
                        env: HashMap::new(),
                        cwd: None,
                        kind: McpServerKind::Tool,
                        enabled: true,
                        depends_on: vec![],
                        resource_class: IndexerResourceClass::Light,
                        indexer_group: None,
                    },
                ],
            },
            hooks: HooksConfig::default(),
        };
        let names: Vec<&str> = c.indexers().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["whisper"]);
    }
}
