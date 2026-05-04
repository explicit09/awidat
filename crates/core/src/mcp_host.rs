//! Session-scoped MCP connection manager.
//!
//! The "warm-process abstraction" ML-backed tools need. Spawned once
//! per session, drops on session end. Lazy: a server is launched the
//! first time a tool calls into it, not eagerly at session start.
//! That keeps cold-start cost paid only by the user-visible feature
//! that needed it.
//!
//! Mirrors codex's `McpConnectionManager` pattern (see
//! `harnesses/codex/codex-rs/codex-mcp/src/connection_manager.rs`)
//! but slimmed to ~10% of the surface — we don't need elicitation,
//! OAuth, sandbox state, or codex-apps caching.
//!
//! ## Why this exists
//!
//! Without it: every `clip_search(query)` call would have to fresh-
//! spawn `clip-mcp`, pay the ~6s torch import + model load, encode
//! one query, and exit. With it: clip-mcp lives for the whole
//! session, model stays loaded in `_MODEL_CACHE`, every query after
//! the first costs only the encoder forward pass (~50ms on CPU).
//!
//! Validated pattern across 12+ harnesses (see
//! `_notes/cross-harness-research/hot-encoder-pattern.md`): every
//! ML-backed agent harness keeps the model warm in a long-lived
//! process; none spawn per tool call. MCP stdio is the canonical
//! "long-lived ML helper" abstraction.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use awidat_config::McpServer;
use awidat_mcp::{Client, ClientInfo, McpError, ServerConfig};
use thiserror::Error;
use tokio::sync::Mutex;

/// A registered server: its config, plus the lazily-launched client.
struct Slot {
    config: ServerConfig,
    /// `None` until the first tool call against this server initializes
    /// it. After init, kept alive for the session.
    client: Option<Client>,
}

/// Session-scoped MCP host. Tools call [`McpHost::call_tool`]; the
/// host transparently spawns the relevant server on first use and
/// reuses it for every subsequent call.
///
/// Cheaply cloneable — internally `Arc`-wrapped so multiple tool
/// handlers can share the same underlying connection set.
#[derive(Clone)]
pub struct McpHost {
    inner: Arc<Mutex<HashMap<String, Slot>>>,
    client_info: ClientInfo,
}

/// Errors `McpHost::call_tool` can return.
#[derive(Debug, Error)]
pub enum McpHostError {
    /// Server name not registered. The host doesn't know how to
    /// launch it because no `[[mcp.servers]]` entry was given.
    #[error("MCP server '{server}' not registered in this session")]
    UnknownServer {
        /// Server name the caller asked for.
        server: String,
    },
    /// The MCP server failed to launch or initialize.
    #[error("MCP server '{server}' init failed: {message}")]
    InitFailed {
        /// Server name that failed to start.
        server: String,
        /// Underlying launch / initialize error message.
        message: String,
    },
    /// The MCP `tools/call` request itself failed.
    #[error("MCP tool '{server}/{tool}' call failed: {message}")]
    CallFailed {
        /// Server name the call was routed to.
        server: String,
        /// Tool name that failed.
        tool: String,
        /// Underlying transport / protocol error message.
        message: String,
    },
}

impl McpHost {
    /// Build an empty host. Use [`Self::register`] to add server
    /// configs before any tool tries to call into them.
    pub fn new(client_info: ClientInfo) -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            client_info,
        }
    }

    /// Build a host pre-populated with every entry from a
    /// [`awidat_config::Config`]'s `mcp.servers` collection. The most
    /// common entry point — `Session::new` calls this with the
    /// merged user+project config.
    ///
    /// `project_root` is needed to resolve relative `cwd` fields in
    /// the server config (matches the index dispatcher's behavior).
    pub fn from_servers(
        servers: &[McpServer],
        project_root: &std::path::Path,
        client_info: ClientInfo,
    ) -> Self {
        let host = Self::new(client_info);
        let mut map = host.inner.try_lock().expect("fresh host has no contention");
        for s in servers {
            let cfg = server_config_from(s, project_root);
            map.insert(
                s.name.clone(),
                Slot {
                    config: cfg,
                    client: None,
                },
            );
        }
        drop(map);
        host
    }

    /// Manually register one server. Useful in tests + for the rare
    /// case of dynamic registration mid-session (we don't expose this
    /// to the agent today, but it's a natural extension point).
    pub fn register(&self, name: String, config: ServerConfig) {
        let mut map = self.inner.blocking_lock();
        map.insert(
            name,
            Slot {
                config,
                client: None,
            },
        );
    }

    /// Whether `server` is registered (regardless of whether it's
    /// been spawned yet).
    pub async fn has_server(&self, server: &str) -> bool {
        self.inner.lock().await.contains_key(server)
    }

    /// Call `tool` on `server` with `args`. On first call per
    /// (session, server) pair, this lazily launches the server and
    /// runs the MCP `initialize` handshake. Subsequent calls reuse
    /// the same warm process.
    ///
    /// Returns the tool's structured-content JSON if the MCP server
    /// returned one, otherwise the concatenated text content.
    pub async fn call_tool(
        &self,
        server: &str,
        tool: &str,
        args: serde_json::Value,
    ) -> Result<serde_json::Value, McpHostError> {
        // Phase 1: ensure-launched. Holds the lock the *whole* time
        // because launching is exclusive — two concurrent first-calls
        // for the same server should serialize, not double-spawn. The
        // launch itself is fast (process spawn, not the model load
        // inside the python child); the python warm-up happens
        // *during* the first tool call, not the launch. So this lock
        // hold is bounded.
        {
            let mut map = self.inner.lock().await;
            let slot = map
                .get_mut(server)
                .ok_or_else(|| McpHostError::UnknownServer {
                    server: server.to_string(),
                })?;
            if slot.client.is_none() {
                let mut client =
                    Client::launch(slot.config.clone()).map_err(|e: McpError| {
                        McpHostError::InitFailed {
                            server: server.to_string(),
                            message: e.to_string(),
                        }
                    })?;
                client
                    .initialize_with_timeout(self.client_info.clone(), Duration::from_secs(60))
                    .await
                    .map_err(|e: McpError| McpHostError::InitFailed {
                        server: server.to_string(),
                        message: e.to_string(),
                    })?;
                slot.client = Some(client);
            }
        }

        // Phase 2: call_tool. Re-acquire the lock briefly to grab a
        // mutable borrow of the client. The MCP client itself owns
        // an internal request channel, so the call serializes
        // through that — keeping the host lock for the whole
        // duration would prevent other servers from being called in
        // parallel, which we want.
        //
        // Compromise: serialize calls *to the same server* via the
        // map lock, but allow parallel calls *across servers*. To
        // achieve that we'd need per-slot mutexes; for now the
        // simpler whole-map lock is fine — vision tools aren't
        // called in parallel from a single agent turn anyway.
        let mut map = self.inner.lock().await;
        let slot = map
            .get_mut(server)
            .expect("slot guaranteed present after launch phase");
        let client = slot
            .client
            .as_mut()
            .expect("client guaranteed launched after phase 1");

        let result = client
            .call_tool(tool, args)
            .await
            .map_err(|e: McpError| McpHostError::CallFailed {
                server: server.to_string(),
                tool: tool.to_string(),
                message: e.to_string(),
            })?;

        // Prefer structured_content when the server returned it
        // (clip-mcp's query_text returns a typed dict); fall back to
        // concatenating text blocks when it didn't.
        if let Some(structured) = result.structured_content {
            return Ok(structured);
        }
        let text = result
            .single_text()
            .map(str::to_string)
            .unwrap_or_default();
        Ok(serde_json::Value::String(text))
    }
}

/// Build an [`awidat_mcp::ServerConfig`] from the user-facing
/// [`awidat_config::McpServer`] schema. Resolves relative `cwd`
/// fields against `project_root`, matching the index dispatcher.
fn server_config_from(server: &McpServer, project_root: &std::path::Path) -> ServerConfig {
    let cwd = server.cwd.as_ref().map(|c| {
        if c.is_absolute() {
            c.clone()
        } else {
            project_root.join(c)
        }
    });
    ServerConfig {
        name: server.name.clone(),
        command: server.command.clone(),
        args: server.args.clone(),
        env: server.env.clone(),
        cwd,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_server_returns_typed_error() {
        let host = McpHost::new(ClientInfo {
            name: "test".into(),
            version: "0.0.0".into(),
        });
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let err = runtime
            .block_on(host.call_tool("nope", "x", serde_json::json!({})))
            .unwrap_err();
        match err {
            McpHostError::UnknownServer { server } => assert_eq!(server, "nope"),
            other => panic!("want UnknownServer, got {other:?}"),
        }
    }

    #[test]
    fn from_servers_registers_each_entry() {
        let server = McpServer {
            name: "clip".into(),
            command: "uv".into(),
            args: vec!["run".into()],
            env: HashMap::new(),
            cwd: None,
            kind: awidat_config::McpServerKind::Indexer,
        };
        let host = McpHost::from_servers(
            std::slice::from_ref(&server),
            std::path::Path::new("/proj"),
            ClientInfo {
                name: "test".into(),
                version: "0.0.0".into(),
            },
        );
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        assert!(runtime.block_on(host.has_server("clip")));
        assert!(!runtime.block_on(host.has_server("face")));
    }

    #[test]
    fn server_config_from_resolves_relative_cwd() {
        let server = McpServer {
            name: "x".into(),
            command: "true".into(),
            args: vec![],
            env: HashMap::new(),
            cwd: Some(std::path::PathBuf::from("python")),
            kind: awidat_config::McpServerKind::Indexer,
        };
        let cfg = server_config_from(&server, std::path::Path::new("/proj"));
        assert_eq!(cfg.cwd, Some(std::path::PathBuf::from("/proj/python")));
    }
}
