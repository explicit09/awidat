//! High-level MCP client. Thin wrapper over [`rmcp::service::RunningService`].
//!
//! `rmcp` owns: stdio framing, request id allocation, late-response handling,
//! `notifications/cancelled`, `_meta.progressToken` injection, capability
//! negotiation, the `2025-11-25` protocol version. We own: subprocess
//! launch, our `ServerConfig` shape, the small surface our agent and CLI
//! actually call, and per-request progress subscription routing via
//! [`crate::handler::MontageHandler`].

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use rmcp::ServiceExt;
use rmcp::model::{
    CallToolRequestParams, CallToolResult, ClientCapabilities, ClientRequest, Implementation,
    InitializeRequestParams, NumberOrString, ProgressToken,
};
use rmcp::service::{PeerRequestOptions, RoleClient, RunningService};
use rmcp::transport::TokioChildProcess;
use serde::Serialize;
use tokio::process::Command;
use tokio::sync::mpsc;

use crate::error::McpError;
use crate::handler::{MontageHandler, ProgressEvent};

/// Default request timeout. Indexers can run for minutes (whisper on a long
/// episode is the worst case), so this is intentionally generous; shorter
/// timeouts are layered above when a tool call is known to be quick.
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// MCP protocol version we negotiate against. rmcp 0.15 exposes the
/// version as a `Display`-formatted newtype; older versions had
/// `.as_str()` returning a `&'static str`. We string-format once and
/// return a `&'static str` so call sites stay unchanged.
pub fn mcp_protocol_version() -> &'static str {
    use std::sync::OnceLock;
    static CACHED: OnceLock<String> = OnceLock::new();
    CACHED
        .get_or_init(|| rmcp::model::ProtocolVersion::LATEST.to_string())
        .as_str()
}

/// Backwards-compat constant for code that imports `MCP_PROTOCOL_VERSION`.
/// Prefer [`mcp_protocol_version`] in new code.
pub const MCP_PROTOCOL_VERSION: &str = "2025-11-25";

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
    /// Application name, e.g. `"montage"`.
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
    /// Negotiated protocol version (echo from server).
    pub protocol_version: String,
    /// Capabilities the server advertised.
    pub capabilities: ServerCapabilities,
    /// Optional server-attached human-readable instructions.
    pub instructions: Option<String>,
}

/// Server-advertised capabilities. Mirrors `rmcp::model::ServerCapabilities`
/// but simplified to bools at the boundary so callers don't import rmcp
/// types.
#[derive(Debug, Default, Clone)]
pub struct ServerCapabilities {
    /// `tools` — server hosts tools (every indexer).
    pub tools: bool,
    /// `resources` — server exposes addressable resources.
    pub resources: bool,
    /// `prompts` — server hosts prompt templates.
    pub prompts: bool,
    /// `logging` — server emits `notifications/message` log frames.
    pub logging: bool,
    /// `completions` — server exposes argument-completion suggestions.
    pub completions: bool,
}

impl ServerCapabilities {
    /// Returns true iff the server advertised the `tools` capability.
    /// Indexers always do; required before calling `tools/list`.
    pub fn supports_tools(&self) -> bool {
        self.tools
    }
}

impl From<rmcp::model::ServerCapabilities> for ServerCapabilities {
    fn from(c: rmcp::model::ServerCapabilities) -> Self {
        Self {
            tools: c.tools.is_some(),
            resources: c.resources.is_some(),
            prompts: c.prompts.is_some(),
            logging: c.logging.is_some(),
            completions: c.completions.is_some(),
        }
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
    /// JSON Schema for the tool's input arguments.
    pub input_schema: Option<serde_json::Value>,
}

impl From<rmcp::model::Tool> for ToolDescriptor {
    fn from(t: rmcp::model::Tool) -> Self {
        Self {
            name: t.name.into_owned(),
            title: t.title,
            description: t.description.map(std::borrow::Cow::into_owned),
            input_schema: serde_json::to_value(&*t.input_schema).ok(),
        }
    }
}

/// One content block on a `tools/call` result.
#[derive(Debug, Clone)]
pub enum ContentBlock {
    /// Plain UTF-8 text.
    Text(String),
    /// Any other block type (image, audio, …) — kept as opaque JSON for
    /// forward compat.
    Other(serde_json::Value),
}

impl From<rmcp::model::Content> for ContentBlock {
    fn from(c: rmcp::model::Content) -> Self {
        match c.raw {
            rmcp::model::RawContent::Text(t) => Self::Text(t.text),
            other => Self::Other(serde_json::to_value(&other).unwrap_or(serde_json::Value::Null)),
        }
    }
}

/// What `call_tool` returns.
#[derive(Debug, Clone)]
pub struct ToolResult {
    /// Multi-format content blocks. Indexers typically include a single
    /// `text` block carrying the JSON sidecar; some return only
    /// `structured_content`.
    pub content: Vec<ContentBlock>,
    /// True iff the server reported the tool failed.
    pub is_error: bool,
    /// Structured / typed JSON output, if the server provided one.
    pub structured_content: Option<serde_json::Value>,
}

impl ToolResult {
    /// Convenience: extract a single text content block, if exactly one
    /// exists. Returns `None` for zero or many.
    pub fn single_text(&self) -> Option<&str> {
        let mut iter = self.content.iter().filter_map(|b| match b {
            ContentBlock::Text(t) => Some(t.as_str()),
            ContentBlock::Other(_) => None,
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
/// 1. [`Client::launch`] spawns the server and starts the rmcp transport.
/// 2. [`Client::initialize`] performs the MCP handshake; must be called
///    before any other method.
/// 3. [`Client::list_tools`], [`Client::call_tool`] — the request methods.
/// 4. [`Client::shutdown`] — graceful tear-down.
pub struct Client {
    server_name: String,
    handler: MontageHandler,
    /// Pending pre-`initialize` state: the spawned child process transport,
    /// kept here until `initialize()` consumes it.
    pending_transport: Option<TokioChildProcess>,
    /// OS pid for the child process when the transport exposes it. Used by
    /// long-running callers for optional process telemetry.
    child_pid: Option<u32>,
    /// `Option` so we can `take()` it to consume in `shutdown`. Always
    /// `Some` between `initialize` and `shutdown`.
    service: Option<Arc<RunningService<RoleClient, MontageHandler>>>,
    capabilities: ServerCapabilities,
    server_info: Option<ServerInfo>,
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
        let transport = TokioChildProcess::new(cmd).map_err(|e| McpError::Spawn {
            server: name.clone(),
            message: e.to_string(),
        })?;
        let child_pid = transport.id();
        Ok(Self {
            server_name: name,
            handler: MontageHandler::default(),
            pending_transport: Some(transport),
            child_pid,
            service: None,
            capabilities: ServerCapabilities::default(),
            server_info: None,
        })
    }

    /// OS pid for the spawned server process, when available.
    pub fn child_pid(&self) -> Option<u32> {
        self.child_pid
    }

    /// MCP `initialize` handshake.
    pub async fn initialize(&mut self, info: ClientInfo) -> Result<ServerInfo, McpError> {
        self.initialize_with_timeout(info, Duration::from_secs(20))
            .await
    }

    /// Variant of [`Self::initialize`] with an explicit handshake timeout.
    pub async fn initialize_with_timeout(
        &mut self,
        info: ClientInfo,
        req_timeout: Duration,
    ) -> Result<ServerInfo, McpError> {
        let transport =
            self.pending_transport
                .take()
                .ok_or_else(|| McpError::ProtocolViolation {
                    server: self.server_name.clone(),
                    message: "initialize called twice on the same client".into(),
                })?;

        // Pre-load our handler with the ClientInfo we want to advertise.
        // The `ClientHandler::get_info` default returns ClientInfo::default();
        // we override on MontageHandler by stashing this and returning it.
        // rmcp 1.8: both param structs are #[non_exhaustive], so build via
        // their constructors instead of struct literals.
        let init_params = InitializeRequestParams::new(
            ClientCapabilities::default(),
            Implementation::new(info.name, info.version),
        )
        .with_protocol_version(rmcp::model::ProtocolVersion::LATEST);
        self.handler.set_client_info(init_params).await;

        let handler = self.handler.clone();
        let serve_fut = handler.serve(transport);
        let service = tokio::time::timeout(req_timeout, serve_fut)
            .await
            .map_err(|_| McpError::Timeout {
                server: self.server_name.clone(),
                seconds: req_timeout.as_secs(),
            })?
            .map_err(|e| map_init_error(&self.server_name, e))?;

        // rmcp 1.8: `peer_info()` returns `Option<Arc<InitializeResult>>`
        // directly (already effectively cloned via the Arc), not
        // `Option<&InitializeResult>`.
        let peer_info = service
            .peer_info()
            .ok_or_else(|| McpError::ProtocolViolation {
                server: self.server_name.clone(),
                message: "server did not return peer_info after initialize".into(),
            })?;

        let capabilities: ServerCapabilities = peer_info.capabilities.clone().into();
        let server_info = ServerInfo {
            name: peer_info.server_info.name.clone(),
            version: peer_info.server_info.version.clone(),
            protocol_version: peer_info.protocol_version.to_string(),
            capabilities: capabilities.clone(),
            instructions: peer_info.instructions.clone(),
        };

        self.capabilities = capabilities;
        self.server_info = Some(server_info.clone());
        self.service = Some(Arc::new(service));
        Ok(server_info)
    }

    /// Capabilities the server advertised during `initialize`. Empty before
    /// `initialize` returns.
    pub fn capabilities(&self) -> &ServerCapabilities {
        &self.capabilities
    }

    /// `tools/list`. Returns every tool the server advertises.
    pub async fn list_tools(&mut self) -> Result<Vec<ToolDescriptor>, McpError> {
        let service = self.service()?;
        let tools = service
            .peer()
            .list_all_tools()
            .await
            .map_err(|e| map_service_error(&self.server_name, e))?;
        Ok(tools.into_iter().map(ToolDescriptor::from).collect())
    }

    /// `tools/call`. Default per-request timeout; no cancellation; no progress
    /// subscription.
    pub async fn call_tool(
        &mut self,
        name: &str,
        arguments: serde_json::Value,
    ) -> Result<ToolResult, McpError> {
        self.call_tool_full(name, arguments, None, None, None).await
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
    /// the tool call is in flight, the client sends `notifications/cancelled`
    /// to the server (per MCP `2025-11-25`) and returns
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
    /// `notifications/progress` events. rmcp injects `_meta.progressToken`
    /// into the request automatically; we register a subscriber against
    /// that token and forward every matching `notifications/progress` frame
    /// onto `progress_tx` until the call returns. Subscription is removed
    /// when the response arrives or the call is cancelled.
    pub async fn call_tool_with_progress(
        &mut self,
        name: &str,
        arguments: serde_json::Value,
        progress_tx: mpsc::Sender<ProgressEvent>,
        cancel: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<ToolResult, McpError> {
        self.call_tool_full(name, arguments, None, cancel, Some(progress_tx))
            .await
    }

    #[tracing::instrument(
        name = "mcp.call_tool",
        skip(self, arguments, cancel, progress_tx),
        fields(server = %self.server_name, tool = name)
    )]
    async fn call_tool_full(
        &mut self,
        name: &str,
        arguments: serde_json::Value,
        req_timeout: Option<Duration>,
        cancel: Option<&tokio_util::sync::CancellationToken>,
        progress_tx: Option<mpsc::Sender<ProgressEvent>>,
    ) -> Result<ToolResult, McpError> {
        let service = self.service()?.clone();
        let arguments_obj = match arguments {
            serde_json::Value::Object(map) => Some(map),
            serde_json::Value::Null => None,
            other => Some({
                let mut map = serde_json::Map::new();
                map.insert("value".into(), other);
                map
            }),
        };
        // rmcp 1.8: `CallToolRequestParams` is #[non_exhaustive]; build via
        // its constructor. `name` takes `impl Into<Cow<'static, str>>`, so
        // an owned `String` converts directly.
        let mut params = CallToolRequestParams::new(name.to_string());
        params.arguments = arguments_obj;

        let to = req_timeout.unwrap_or(DEFAULT_REQUEST_TIMEOUT);
        let request = ClientRequest::CallToolRequest(rmcp::model::CallToolRequest::new(params));
        let options = PeerRequestOptions::with_timeout(to);

        let handle = service
            .peer()
            .send_cancellable_request(request, options)
            .await
            .map_err(|e| map_service_error(&self.server_name, e))?;

        // Register a progress subscriber under the token rmcp picked.
        // Held until the call returns (Drop deregisters).
        let _progress_guard = match progress_tx {
            Some(tx) => match &handle.progress_token {
                ProgressToken(NumberOrString::Number(n)) => {
                    Some(self.handler.register_progress(*n, tx).await)
                }
                ProgressToken(NumberOrString::String(_)) => None,
            },
            None => None,
        };

        let response_result = match cancel {
            Some(c) => {
                // `handle.await_response(self)` and `handle.cancel(self)`
                // both consume the handle. Stash the id and clone the peer
                // up front so a cancel firing first can send
                // notifications/cancelled directly without the handle.
                let request_id = handle.id.clone();
                let peer_for_cancel = service.peer().clone();
                let response_fut = handle.await_response();
                tokio::pin!(response_fut);
                tokio::select! {
                    biased;
                    () = c.cancelled() => {
                        // Best-effort cancel notification. If the server is
                        // already dead the send will fail and we'll just
                        // return Cancelled anyway.
                        let cancel_notif = rmcp::model::CancelledNotification::new(
                            rmcp::model::CancelledNotificationParam {
                                request_id,
                                reason: Some("client cancelled".into()),
                            },
                        );
                        let _ = peer_for_cancel
                            .send_notification(cancel_notif.into())
                            .await;
                        return Err(McpError::Cancelled {
                            server: self.server_name.clone(),
                        });
                    }
                    res = &mut response_fut => res,
                }
            }
            None => handle.await_response().await,
        };

        let server_result = response_result.map_err(|e| map_service_error(&self.server_name, e))?;
        let result: CallToolResult = match server_result {
            rmcp::model::ServerResult::CallToolResult(r) => r,
            _ => {
                return Err(McpError::ProtocolViolation {
                    server: self.server_name.clone(),
                    message: "tools/call returned unexpected ServerResult variant".into(),
                });
            }
        };

        Ok(ToolResult {
            content: result.content.into_iter().map(ContentBlock::from).collect(),
            is_error: result.is_error.unwrap_or(false),
            structured_content: result.structured_content,
        })
    }

    /// Cleanly shut down the server.
    pub async fn shutdown(mut self) -> Result<(), McpError> {
        if let Some(service) = self.service.take()
            && let Ok(service) = Arc::try_unwrap(service)
        {
            let _ = service.cancel().await;
        }
        Ok(())
    }

    fn service(&self) -> Result<&Arc<RunningService<RoleClient, MontageHandler>>, McpError> {
        self.service
            .as_ref()
            .ok_or_else(|| McpError::ProtocolViolation {
                server: self.server_name.clone(),
                message: "client used before initialize() handshake".into(),
            })
    }
}

fn map_init_error(server: &str, e: rmcp::service::ClientInitializeError) -> McpError {
    use rmcp::service::ClientInitializeError as CIE;
    match e {
        CIE::Cancelled => McpError::Cancelled {
            server: server.into(),
        },
        CIE::ConnectionClosed(msg) => McpError::ServerCrashed {
            server: server.into(),
            message: msg,
        },
        CIE::TransportError { error, context } => McpError::Transport {
            server: server.into(),
            message: format!("{context}: {error}"),
        },
        CIE::JsonRpcError(err) => McpError::ToolError {
            server: server.into(),
            message: format!("[code {}] {}", err.code.0, err.message),
        },
        // ExpectedInitResponse, ExpectedInitResult, ConflictInitResponseId,
        // and any future #[non_exhaustive] additions all fall here.
        other => McpError::ProtocolViolation {
            server: server.into(),
            message: other.to_string(),
        },
    }
}

fn map_service_error(server: &str, e: rmcp::ServiceError) -> McpError {
    use rmcp::ServiceError as SE;
    match e {
        SE::Timeout { timeout } => McpError::Timeout {
            server: server.into(),
            seconds: timeout.as_secs(),
        },
        SE::Cancelled { .. } => McpError::Cancelled {
            server: server.into(),
        },
        SE::McpError(err) => McpError::ToolError {
            server: server.into(),
            message: format!("[code {}] {}", err.code.0, err.message),
        },
        SE::TransportSend(err) => McpError::Transport {
            server: server.into(),
            message: err.to_string(),
        },
        SE::TransportClosed => McpError::ServerCrashed {
            server: server.into(),
            message: "transport closed".into(),
        },
        SE::UnexpectedResponse => McpError::ProtocolViolation {
            server: server.into(),
            message: "unexpected response shape from server".into(),
        },
        // Future #[non_exhaustive] additions land here.
        other => McpError::Transport {
            server: server.into(),
            message: other.to_string(),
        },
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
            content: vec![ContentBlock::Text("hi".into())],
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
                ContentBlock::Text("a".into()),
                ContentBlock::Text("b".into()),
            ],
            is_error: false,
            structured_content: None,
        };
        assert_eq!(r.single_text(), None);
    }
}
