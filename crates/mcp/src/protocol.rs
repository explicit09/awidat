//! Wire types for MCP / JSON-RPC 2.0.
//!
//! Only the pieces the Week 2 client uses are typed here; everything else is
//! carried as `serde_json::Value`. We deliberately accept unknown fields on
//! every type for forward compatibility with future MCP versions: this is
//! what lets us upgrade the protocol date below without breaking servers that
//! advertise extra capability fields.

use serde::{Deserialize, Serialize};

/// MCP protocol version we negotiate against. The server is expected to echo
/// (or downgrade) this; we accept any string the server replies with —
/// rejecting on protocol-version mismatch would brittle-couple us to the spec
/// version of every server in the wild.
pub const MCP_PROTOCOL_VERSION: &str = "2025-06-18";

/// JSON-RPC 2.0 request envelope used for outbound calls.
#[derive(Debug, Serialize)]
pub(crate) struct JsonRpcRequest<'a> {
    pub jsonrpc: &'static str,
    pub id: u64,
    pub method: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

/// JSON-RPC 2.0 notification envelope (no `id`, no response).
#[derive(Debug, Serialize)]
pub(crate) struct JsonRpcNotification<'a> {
    pub jsonrpc: &'static str,
    pub method: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

/// JSON-RPC 2.0 response envelope. We accept either `result` or `error`.
#[derive(Debug, Deserialize)]
pub(crate) struct JsonRpcResponse {
    #[allow(dead_code)] // we don't validate `jsonrpc` strictly; accepted for compat.
    #[serde(default)]
    pub jsonrpc: Option<String>,
    pub id: Option<serde_json::Value>,
    #[serde(default)]
    pub result: Option<serde_json::Value>,
    #[serde(default)]
    pub error: Option<JsonRpcError>,
    /// If the message has neither `result` nor `error` and no `id`, it's a
    /// notification — surfaced via this field for the dispatcher to ignore /
    /// log.
    #[serde(default)]
    pub method: Option<String>,
}

/// JSON-RPC error object.
#[derive(Debug, Deserialize)]
pub struct JsonRpcError {
    /// Numeric error code (per JSON-RPC 2.0 spec).
    pub code: i64,
    /// Human-readable error message.
    pub message: String,
    /// Optional server-defined data.
    #[serde(default)]
    pub data: Option<serde_json::Value>,
}

// ---- MCP-level params/results ----

/// Params for `initialize`.
#[derive(Debug, Serialize)]
pub(crate) struct InitializeParams<'a> {
    #[serde(rename = "protocolVersion")]
    pub protocol_version: &'a str,
    pub capabilities: serde_json::Value,
    #[serde(rename = "clientInfo")]
    pub client_info: ClientInfoWire<'a>,
}

/// Lightweight client identity sent in `initialize.params.clientInfo`.
#[derive(Debug, Serialize)]
pub(crate) struct ClientInfoWire<'a> {
    pub name: &'a str,
    pub version: &'a str,
}

/// Result of `initialize`. We only read what we use; everything else
/// round-trips through serde's `serde(default)` machinery — but since we
/// don't need the rest, we keep this small.
#[derive(Debug, Deserialize)]
pub(crate) struct InitializeResult {
    #[serde(rename = "protocolVersion")]
    pub protocol_version: String,
    #[serde(rename = "serverInfo")]
    pub server_info: ServerInfoWire,
    /// Server capabilities, kept opaque so we don't reject novel ones.
    #[serde(default)]
    #[allow(dead_code)]
    pub capabilities: serde_json::Value,
}

/// Server identity from `initialize.result.serverInfo`.
#[derive(Debug, Deserialize)]
pub(crate) struct ServerInfoWire {
    pub name: String,
    pub version: String,
}

/// Result of `tools/list`.
#[derive(Debug, Deserialize)]
pub(crate) struct ListToolsResult {
    pub tools: Vec<ToolDescriptorWire>,
    /// Pagination cursor — we don't implement pagination yet but accept it.
    #[serde(default)]
    #[allow(dead_code)]
    pub next_cursor: Option<String>,
}

/// One entry in `tools/list.result.tools`.
#[derive(Debug, Deserialize)]
pub(crate) struct ToolDescriptorWire {
    pub name: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(rename = "inputSchema", default)]
    pub input_schema: Option<serde_json::Value>,
}

/// Params for `tools/call`.
#[derive(Debug, Serialize)]
pub(crate) struct CallToolParams<'a> {
    pub name: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<serde_json::Value>,
}

/// Result of `tools/call`.
#[derive(Debug, Deserialize)]
pub(crate) struct CallToolResult {
    /// Multi-format content blocks. MCP 2025-06-18 onward ships text, image,
    /// audio, and structured-content variants.
    #[serde(default)]
    pub content: Vec<ContentBlock>,
    /// True iff the tool itself reported failure (vs. a JSON-RPC error).
    #[serde(rename = "isError", default)]
    pub is_error: bool,
    /// Structured / typed result. Servers that return JSON-shaped output
    /// (which all our indexers do) put it here. We keep it opaque so the
    /// orchestrator can deserialize into the index sidecar shape itself.
    #[serde(rename = "structuredContent", default)]
    pub structured_content: Option<serde_json::Value>,
}

/// One content block from `tools/call.result.content`. We support the text
/// variant directly and accept everything else as opaque (so an audio-clip
/// returning content doesn't crash the client).
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum ContentBlock {
    /// Plain UTF-8 text. Indexers may dump structured JSON in here for
    /// servers that don't yet emit `structuredContent`.
    #[serde(rename = "text")]
    Text {
        /// The text payload.
        text: String,
    },
    /// Any other block type — kept around for forward compat.
    #[serde(other)]
    Other,
}
