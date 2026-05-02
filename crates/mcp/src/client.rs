//! High-level MCP client. Wraps [`crate::transport::Transport`] in the
//! `initialize` / `tools/list` / `tools/call` shapes the orchestrator and
//! (later) the agent want to call directly.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use serde::Serialize;
use tokio::process::Command;

use crate::error::McpError;
use crate::protocol::{
    CallToolParams, CallToolResult, ClientInfoWire, ContentBlock, InitializeParams,
    InitializeResult, ListToolsResult, ToolDescriptorWire, MCP_PROTOCOL_VERSION,
};
use crate::transport::Transport;

/// How to spawn an MCP server.
#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// Logical name. The CLI uses this both as the indexer id (Week 2) and
    /// as the tool-namespace label (Week 4+). Lowercase-hyphenated by
    /// convention but the client treats it as opaque.
    pub name: String,
    /// Executable to run, e.g. `"python"` or `"/usr/local/bin/whisper-mcp"`.
    pub command: String,
    /// Arguments after the command.
    pub args: Vec<String>,
    /// Extra env vars set on the child (added to inherited env).
    pub env: HashMap<String, String>,
    /// Optional working directory; defaults to the parent process's cwd.
    pub cwd: Option<PathBuf>,
}

/// What we tell the server about ourselves in `initialize.params.clientInfo`.
#[derive(Debug, Clone, Serialize)]
pub struct ClientInfo {
    /// Application name, e.g. `"awidat"`.
    pub name: String,
    /// Application version.
    pub version: String,
}

/// What the server tells us in `initialize.result.serverInfo`.
#[derive(Debug, Clone)]
pub struct ServerInfo {
    /// Server name as advertised by the server itself.
    pub name: String,
    /// Server version.
    pub version: String,
    /// Negotiated protocol version (echo from server). May differ from
    /// what the client sent.
    pub protocol_version: String,
}

/// One tool entry returned by `tools/list`.
#[derive(Debug, Clone)]
pub struct ToolDescriptor {
    /// Tool id (e.g. `"index_asset"`).
    pub name: String,
    /// Human title (optional).
    pub title: Option<String>,
    /// Human description (optional).
    pub description: Option<String>,
    /// JSON Schema for the tool's input arguments. Opaque on this end —
    /// agents may use it to validate, the orchestrator only needs it to
    /// surface during `awidat index --debug`.
    pub input_schema: Option<serde_json::Value>,
}

impl From<ToolDescriptorWire> for ToolDescriptor {
    fn from(w: ToolDescriptorWire) -> Self {
        Self {
            name: w.name,
            title: w.title,
            description: w.description,
            input_schema: w.input_schema,
        }
    }
}

/// What `call_tool` returns.
#[derive(Debug, Clone)]
pub struct ToolResult {
    /// Multi-format content blocks per MCP. Indexers typically include a
    /// single `text` block carrying the JSON sidecar; some return only
    /// `structured_content`.
    pub content: Vec<ContentBlock>,
    /// True iff the server reported the tool failed.
    pub is_error: bool,
    /// Structured / typed JSON output, if the server provided one. The
    /// indexer orchestrator deserializes this directly into
    /// `IndexSidecar<serde_json::Value>` when present.
    pub structured_content: Option<serde_json::Value>,
}

impl ToolResult {
    /// Convenience: extract a single text content block, if exactly one
    /// exists. Returns `None` for zero or many.
    pub fn single_text(&self) -> Option<&str> {
        let mut iter = self.content.iter().filter_map(|b| match b {
            ContentBlock::Text { text } => Some(text.as_str()),
            ContentBlock::Other => None,
        });
        let first = iter.next()?;
        if iter.next().is_some() {
            return None;
        }
        Some(first)
    }
}

/// The MCP client itself. Owns one server subprocess.
///
/// Lifecycle:
/// 1. [`Client::launch`] spawns the server.
/// 2. [`Client::initialize`] performs the MCP handshake; must be called
///    before any other method.
/// 3. [`Client::list_tools`], [`Client::call_tool`] — the request methods.
/// 4. [`Client::shutdown`] — graceful tear-down. `Drop` falls back to
///    a forced kill (via `tokio::process::Command::kill_on_drop`).
pub struct Client {
    transport: Transport,
    initialized: bool,
}

impl Client {
    /// Spawn the configured MCP server. The server is now alive but the MCP
    /// `initialize` handshake has not happened yet — the next call must be
    /// [`Self::initialize`].
    pub async fn launch(config: ServerConfig) -> Result<Self, McpError> {
        let mut cmd = Command::new(&config.command);
        cmd.args(&config.args);
        for (k, v) in &config.env {
            cmd.env(k, v);
        }
        if let Some(cwd) = &config.cwd {
            cmd.current_dir(cwd);
        }
        let transport = Transport::launch(config.name.clone(), cmd).await?;
        Ok(Self {
            transport,
            initialized: false,
        })
    }

    /// MCP `initialize` handshake. Sends our protocol version and identity,
    /// reads back the server's identity / capabilities, then sends
    /// `notifications/initialized` per the spec.
    pub async fn initialize(&mut self, info: ClientInfo) -> Result<ServerInfo, McpError> {
        let params = InitializeParams {
            protocol_version: MCP_PROTOCOL_VERSION,
            capabilities: serde_json::json!({}),
            client_info: ClientInfoWire {
                name: &info.name,
                version: &info.version,
            },
        };
        let raw = self
            .transport
            .request("initialize", Some(params), Some(Duration::from_secs(20)))
            .await?;
        let parsed: InitializeResult =
            serde_json::from_value(raw).map_err(|e| McpError::ProtocolViolation {
                server: self.transport.server_name().to_string(),
                message: format!("malformed initialize result: {e}"),
            })?;
        // Per spec: client must send notifications/initialized before further
        // requests. (We sent `initialize` first since the server expects it.)
        self.transport
            .notify::<serde_json::Value>("notifications/initialized", None)
            .await?;
        self.initialized = true;
        Ok(ServerInfo {
            name: parsed.server_info.name,
            version: parsed.server_info.version,
            protocol_version: parsed.protocol_version,
        })
    }

    /// `tools/list`. Returns every tool the server advertises.
    pub async fn list_tools(&mut self) -> Result<Vec<ToolDescriptor>, McpError> {
        self.ensure_initialized()?;
        let raw = self
            .transport
            .request::<serde_json::Value>(
                "tools/list",
                None,
                Some(Duration::from_secs(20)),
            )
            .await?;
        let parsed: ListToolsResult =
            serde_json::from_value(raw).map_err(|e| McpError::ProtocolViolation {
                server: self.transport.server_name().to_string(),
                message: format!("malformed tools/list result: {e}"),
            })?;
        Ok(parsed.tools.into_iter().map(ToolDescriptor::from).collect())
    }

    /// `tools/call`. Invokes the named tool with the given JSON arguments.
    /// A `tools/call` result with `isError: true` is surfaced as the typed
    /// [`ToolResult`] (the caller decides whether to treat it as fatal).
    /// JSON-RPC errors are surfaced as [`McpError::ToolError`].
    ///
    /// Optional `req_timeout` overrides the per-request default. Pass
    /// `Some(Duration::from_secs(60 * 30))` for indexer runs that may take
    /// many minutes; `None` uses the transport default (also 30 minutes).
    pub async fn call_tool(
        &mut self,
        name: &str,
        arguments: serde_json::Value,
    ) -> Result<ToolResult, McpError> {
        self.call_tool_with_timeout(name, arguments, None).await
    }

    /// Variant of [`Self::call_tool`] with explicit timeout.
    pub async fn call_tool_with_timeout(
        &mut self,
        name: &str,
        arguments: serde_json::Value,
        req_timeout: Option<Duration>,
    ) -> Result<ToolResult, McpError> {
        self.ensure_initialized()?;
        let params = CallToolParams {
            name,
            arguments: Some(arguments),
        };
        let raw = self
            .transport
            .request("tools/call", Some(params), req_timeout)
            .await?;
        let parsed: CallToolResult =
            serde_json::from_value(raw).map_err(|e| McpError::ProtocolViolation {
                server: self.transport.server_name().to_string(),
                message: format!("malformed tools/call result: {e}"),
            })?;
        Ok(ToolResult {
            content: parsed.content,
            is_error: parsed.is_error,
            structured_content: parsed.structured_content,
        })
    }

    /// Cleanly shut down the server (close stdin, wait briefly, kill if
    /// still alive). Idempotent in the sense that drop also tears down.
    pub async fn shutdown(self) -> Result<(), McpError> {
        self.transport.shutdown().await
    }

    fn ensure_initialized(&self) -> Result<(), McpError> {
        if self.initialized {
            Ok(())
        } else {
            Err(McpError::ProtocolViolation {
                server: self.transport.server_name().to_string(),
                message: "client used before initialize() handshake".into(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_config_constructs() {
        let _ = ServerConfig {
            name: "x".into(),
            command: "echo".into(),
            args: vec!["hi".into()],
            env: HashMap::new(),
            cwd: None,
        };
    }

    #[test]
    fn tool_result_single_text_works() {
        let r = ToolResult {
            content: vec![ContentBlock::Text { text: "hi".into() }],
            is_error: false,
            structured_content: None,
        };
        assert_eq!(r.single_text(), Some("hi"));
        let r = ToolResult {
            content: vec![],
            is_error: false,
            structured_content: None,
        };
        assert_eq!(r.single_text(), None);
        let r = ToolResult {
            content: vec![
                ContentBlock::Text { text: "a".into() },
                ContentBlock::Text { text: "b".into() },
            ],
            is_error: false,
            structured_content: None,
        };
        assert_eq!(r.single_text(), None);
    }

    #[tokio::test]
    async fn ensure_initialized_rejects_pre_handshake() {
        // Construct a Client whose transport points at a benign program.
        // We spawn `cat`, which never speaks MCP — so we don't actually
        // call initialize / list / call. We only verify that ensure_*
        // returns the right error type before initialization.
        let cfg = ServerConfig {
            name: "test-cat".into(),
            command: "cat".into(),
            args: vec![],
            env: HashMap::new(),
            cwd: None,
        };
        let client = Client::launch(cfg).await.unwrap();
        let res = client.ensure_initialized();
        assert!(matches!(res, Err(McpError::ProtocolViolation { .. })));
        client.shutdown().await.unwrap();
    }
}
