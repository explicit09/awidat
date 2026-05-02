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
    CallToolParams, CallToolResult, ClientCapabilitiesWire, ClientInfoWire, ContentBlock,
    InitializeParams, InitializeResult, ListToolsResult, MCP_PROTOCOL_VERSION,
    ServerCapabilitiesWire, ToolDescriptorWire,
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

/// What the server tells us in `initialize.result.serverInfo` plus
/// `initialize.result.capabilities`.
#[derive(Debug, Clone)]
pub struct ServerInfo {
    /// Server name as advertised by the server itself.
    pub name: String,
    /// Server version.
    pub version: String,
    /// Negotiated protocol version (echo from server). May differ from
    /// what the client sent.
    pub protocol_version: String,
    /// Capabilities the server advertised. Names are typed per the MCP
    /// `2025-11-25` spec; unknown names land in
    /// [`ServerCapabilities::other`].
    pub capabilities: ServerCapabilities,
    /// Optional server-attached human-readable instructions.
    pub instructions: Option<String>,
}

/// Server-advertised capabilities. Public counterpart of the wire type;
/// each known capability is `Option<serde_json::Value>` because the spec
/// lets servers nest config under each name.
#[derive(Debug, Default, Clone)]
pub struct ServerCapabilities {
    /// `tools` — server hosts tools (every indexer).
    pub tools: Option<serde_json::Value>,
    /// `resources` — server exposes addressable resources.
    pub resources: Option<serde_json::Value>,
    /// `prompts` — server hosts prompt templates.
    pub prompts: Option<serde_json::Value>,
    /// `logging` — server emits `notifications/message` log frames.
    pub logging: Option<serde_json::Value>,
    /// Forward-compat passthrough for any capability name we don't know
    /// about yet (e.g. `completions`).
    pub other: HashMap<String, serde_json::Value>,
}

impl From<ServerCapabilitiesWire> for ServerCapabilities {
    fn from(w: ServerCapabilitiesWire) -> Self {
        Self {
            tools: w.tools,
            resources: w.resources,
            prompts: w.prompts,
            logging: w.logging,
            other: w.other,
        }
    }
}

impl ServerCapabilities {
    /// Returns true iff the server advertised the `tools` capability.
    /// Indexers always do; required before calling `tools/list`.
    pub fn supports_tools(&self) -> bool {
        self.tools.is_some()
    }
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
    capabilities: ServerCapabilities,
}

impl Client {
    /// Spawn the configured MCP server. The server is now alive but the MCP
    /// `initialize` handshake has not happened yet — the next call must be
    /// [`Self::initialize`].
    pub fn launch(config: ServerConfig) -> Result<Self, McpError> {
        let ServerConfig {
            name,
            command,
            args,
            env,
            cwd,
        } = config;
        let mut cmd = Command::new(&command);
        cmd.args(&args);
        for (k, v) in &env {
            cmd.env(k, v);
        }
        if let Some(cwd) = &cwd {
            cmd.current_dir(cwd);
        }
        let transport = Transport::launch(name, cmd)?;
        Ok(Self {
            transport,
            initialized: false,
            capabilities: ServerCapabilities::default(),
        })
    }

    /// MCP `initialize` handshake. Sends our protocol version and identity,
    /// reads back the server's identity / capabilities, then sends
    /// `notifications/initialized` per the spec.
    pub async fn initialize(&mut self, info: ClientInfo) -> Result<ServerInfo, McpError> {
        self.initialize_with_timeout(info, Duration::from_secs(20))
            .await
    }

    /// Variant of [`Self::initialize`] with an explicit request timeout.
    pub async fn initialize_with_timeout(
        &mut self,
        info: ClientInfo,
        req_timeout: Duration,
    ) -> Result<ServerInfo, McpError> {
        let params = InitializeParams {
            protocol_version: MCP_PROTOCOL_VERSION,
            capabilities: ClientCapabilitiesWire::default(),
            client_info: ClientInfoWire {
                name: &info.name,
                version: &info.version,
            },
        };
        let raw = self
            .transport
            .request("initialize", Some(params), Some(req_timeout))
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
        let capabilities: ServerCapabilities = parsed.capabilities.into();
        self.capabilities = capabilities.clone();
        self.initialized = true;
        Ok(ServerInfo {
            name: parsed.server_info.name,
            version: parsed.server_info.version,
            protocol_version: parsed.protocol_version,
            capabilities,
            instructions: parsed.instructions,
        })
    }

    /// Capabilities the server advertised during `initialize`. Empty before
    /// `initialize` returns. Survives across `tools/list` and `tools/call`
    /// calls so callers don't have to stash the [`ServerInfo`].
    pub fn capabilities(&self) -> &ServerCapabilities {
        &self.capabilities
    }

    /// `tools/list`. Returns every tool the server advertises.
    pub async fn list_tools(&mut self) -> Result<Vec<ToolDescriptor>, McpError> {
        self.ensure_initialized()?;
        let raw = self
            .transport
            .request::<serde_json::Value>("tools/list", None, Some(Duration::from_secs(20)))
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
        self.call_tool_full(name, arguments, req_timeout, None, None)
            .await
    }

    /// Cancellable variant of [`Self::call_tool`]. If `cancel` fires while
    /// the tool call is in flight, the client sends a `notifications/cancelled`
    /// frame to the server (per MCP `2025-11-25`) and returns
    /// [`McpError::Cancelled`]. Use this for long-running tools — render
    /// jobs, whisper transcriptions on a 1h episode — where the orchestrator
    /// may need to abort mid-flight.
    pub async fn call_tool_with_cancel(
        &mut self,
        name: &str,
        arguments: serde_json::Value,
        cancel: &tokio_util::sync::CancellationToken,
    ) -> Result<ToolResult, McpError> {
        self.call_tool_full(name, arguments, None, Some(cancel), None)
            .await
    }

    /// Variant of [`Self::call_tool`] that subscribes to
    /// `notifications/progress` events from the server. The client adds
    /// `_meta.progressToken` to the outbound params (per MCP `2025-11-25`
    /// progress utility) so the server knows where to send updates; events
    /// are forwarded onto `progress_tx` until the call returns.
    ///
    /// `progress_tx` is consumed for the duration of the call. The
    /// subscription is removed automatically once the response arrives,
    /// times out, is cancelled, or the server crashes.
    pub async fn call_tool_with_progress(
        &mut self,
        name: &str,
        arguments: serde_json::Value,
        progress_tx: tokio::sync::mpsc::Sender<crate::transport::ProgressEvent>,
        cancel: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<ToolResult, McpError> {
        self.call_tool_full(name, arguments, None, cancel, Some(progress_tx))
            .await
    }

    /// Internal: full-shape call_tool used by every public variant.
    async fn call_tool_full(
        &mut self,
        name: &str,
        arguments: serde_json::Value,
        req_timeout: Option<Duration>,
        cancel: Option<&tokio_util::sync::CancellationToken>,
        progress_tx: Option<tokio::sync::mpsc::Sender<crate::transport::ProgressEvent>>,
    ) -> Result<ToolResult, McpError> {
        self.ensure_initialized()?;
        let params = CallToolParams {
            name,
            arguments: Some(arguments),
        };
        let raw = self
            .transport
            .request_full("tools/call", Some(params), req_timeout, cancel, progress_tx)
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
        let client = Client::launch(cfg).unwrap();
        let res = client.ensure_initialized();
        assert!(matches!(res, Err(McpError::ProtocolViolation { .. })));
        client.shutdown().await.unwrap();
    }
}
