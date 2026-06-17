//! App-hosted MCP endpoint for external editor inspection.
//!
//! This complements the existing `montage-mcp-server` sidecar. The sidecar
//! exposes backend/project tools; this module runs inside the desktop app so
//! outside agents can inspect the currently open editor and capture what is on
//! screen.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use montage_proto::otio::{Stack, StackChild, Timeline, TrackChild};
use montage_proto::project::Project;
use serde::Serialize;
use serde_json::{Value, json};
use tauri::{AppHandle, Manager};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tracing::{info, warn};

use crate::state::{MontageState, ViewState};

/// Loopback endpoint external agents can connect to while the desktop app is
/// running.
pub const LISTEN_ADDR: &str = "127.0.0.1:8420";

/// Start the app-hosted MCP server in the Tauri async runtime.
///
/// Opt-in only: this endpoint exposes live editor state and screenshot capture
/// (using the app's screen-recording permission), so it must not run on every
/// launch where any local process could reach `127.0.0.1:8420`. It starts only
/// when `MONTAGE_APP_MCP` is set.
pub fn start(app: AppHandle) {
    if std::env::var_os("MONTAGE_APP_MCP").is_none() {
        info!("app MCP server disabled (set MONTAGE_APP_MCP=1 to enable)");
        return;
    }
    tauri::async_runtime::spawn(async move {
        let listener = match TcpListener::bind(LISTEN_ADDR).await {
            Ok(listener) => listener,
            Err(err) => {
                warn!(addr = LISTEN_ADDR, error = %err, "app MCP server failed to bind");
                return;
            }
        };

        info!(addr = LISTEN_ADDR, "app MCP server listening");
        loop {
            let (stream, peer) = match listener.accept().await {
                Ok(accepted) => accepted,
                Err(err) => {
                    warn!(error = %err, "app MCP server accept failed");
                    continue;
                }
            };

            if !peer.ip().is_loopback() {
                warn!(peer = %peer, "app MCP rejected non-loopback peer");
                continue;
            }

            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(err) = handle_connection(app, stream).await {
                    warn!(error = %err, "app MCP request failed");
                }
            });
        }
    });
}

async fn handle_connection(app: AppHandle, mut stream: TcpStream) -> Result<(), AppMcpError> {
    let raw = read_http_request(&mut stream).await?;
    let allow_origin = allowed_cors_origin(&raw);
    let cors = allow_origin.as_deref();
    if is_options_request(&raw) {
        write_http_response(&mut stream, 200, "", "", cors).await?;
        return Ok(());
    }

    let request = parse_http_request(&raw)?;
    if request.method != "POST" {
        write_http_response(
            &mut stream,
            405,
            "application/json",
            &json_error_text("method not allowed"),
            cors,
        )
        .await?;
        return Ok(());
    }
    if request.path != "/" && request.path != "/mcp" {
        write_http_response(
            &mut stream,
            404,
            "application/json",
            &json_error_text("not found"),
            cors,
        )
        .await?;
        return Ok(());
    }

    let snapshot = collect_snapshot(&app).await;
    let response = handle_json_rpc_value(request.body, snapshot, ScreenshotHandler::desktop());
    if response.is_null() {
        // JSON-RPC notification (e.g. notifications/initialized): the MCP
        // Streamable HTTP transport requires `202 Accepted` with no body for
        // accepted notifications, which strict browser clients check.
        write_http_response(&mut stream, 202, "", "", cors).await?;
    } else {
        write_http_response(
            &mut stream,
            200,
            "application/json",
            &response.to_string(),
            cors,
        )
        .await?;
    }
    Ok(())
}

async fn read_http_request(stream: &mut TcpStream) -> Result<Vec<u8>, AppMcpError> {
    let mut buffer = Vec::with_capacity(8192);
    let mut temp = [0_u8; 4096];
    loop {
        let n = stream.read(&mut temp).await?;
        if n == 0 {
            break;
        }
        buffer.extend_from_slice(&temp[..n]);
        if let Some(header_end) = find_header_end(&buffer) {
            let content_length = parse_content_length(&buffer[..header_end]).unwrap_or(0);
            let expected_len = header_end + 4 + content_length;
            if buffer.len() >= expected_len {
                buffer.truncate(expected_len);
                break;
            }
        }
        if buffer.len() > 2 * 1024 * 1024 {
            return Err(AppMcpError::Http("request too large".into()));
        }
    }
    Ok(buffer)
}

fn find_header_end(buffer: &[u8]) -> Option<usize> {
    buffer.windows(4).position(|window| window == b"\r\n\r\n")
}

fn parse_content_length(headers: &[u8]) -> Option<usize> {
    let header_text = std::str::from_utf8(headers).ok()?;
    for line in header_text.lines() {
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            return value.trim().parse().ok();
        }
    }
    None
}

fn is_options_request(raw: &[u8]) -> bool {
    std::str::from_utf8(raw)
        .map(|text| text.starts_with("OPTIONS "))
        .unwrap_or(false)
}

/// Echo the request `Origin` only when it is a loopback origin (localhost /
/// 127.0.0.1 / [::1], any port). A non-loopback or absent origin gets no CORS
/// header — non-browser clients are unaffected since they ignore CORS, while a
/// browser dev origin like `http://localhost:5173` now matches.
fn allowed_cors_origin(raw: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(raw).ok()?;
    let head = text.split_once("\r\n\r\n").map(|(h, _)| h).unwrap_or(text);
    let origin = head.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.trim()
            .eq_ignore_ascii_case("origin")
            .then(|| value.trim().to_string())
    })?;
    is_loopback_origin(&origin).then_some(origin)
}

fn is_loopback_origin(origin: &str) -> bool {
    let Some(rest) = origin
        .strip_prefix("http://")
        .or_else(|| origin.strip_prefix("https://"))
    else {
        return false;
    };
    let authority = rest.split('/').next().unwrap_or(rest);
    let host = authority
        .rsplit_once(':')
        .map(|(host, _)| host)
        .unwrap_or(authority);
    matches!(host, "localhost" | "127.0.0.1" | "[::1]" | "::1")
}

async fn write_http_response(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: &str,
    allow_origin: Option<&str>,
) -> Result<(), AppMcpError> {
    let reason = match status {
        200 => "OK",
        202 => "Accepted",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "Error",
    };
    let content_type_header = if content_type.is_empty() {
        String::new()
    } else {
        format!("Content-Type: {content_type}\r\n")
    };
    let cors_header = match allow_origin {
        Some(origin) => format!(
            "Access-Control-Allow-Origin: {origin}\r\n\
             Access-Control-Allow-Methods: POST, OPTIONS\r\n\
             Access-Control-Allow-Headers: Content-Type, MCP-Protocol-Version, Mcp-Session-Id\r\n"
        ),
        None => String::new(),
    };
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\n\
         {content_type_header}\
         {cors_header}\
         Content-Length: {}\r\n\
         Connection: close\r\n\
         \r\n\
         {body}",
        body.len()
    );
    stream.write_all(response.as_bytes()).await?;
    Ok(())
}

fn json_error_text(message: &str) -> String {
    json!({ "error": message }).to_string()
}

#[derive(Debug)]
struct HttpRequest {
    method: String,
    path: String,
    body: Value,
}

fn parse_http_request(raw: &[u8]) -> Result<HttpRequest, AppMcpError> {
    let text = std::str::from_utf8(raw).map_err(|err| AppMcpError::Http(err.to_string()))?;
    let (head, body) = text
        .split_once("\r\n\r\n")
        .ok_or_else(|| AppMcpError::Http("missing HTTP header terminator".into()))?;
    let request_line = head
        .lines()
        .next()
        .ok_or_else(|| AppMcpError::Http("missing request line".into()))?;
    let mut parts = request_line.split_whitespace();
    let method = parts
        .next()
        .ok_or_else(|| AppMcpError::Http("missing method".into()))?
        .to_string();
    let path = parts
        .next()
        .ok_or_else(|| AppMcpError::Http("missing path".into()))?
        .to_string();
    let body = serde_json::from_str(body).map_err(|err| AppMcpError::Http(err.to_string()))?;
    Ok(HttpRequest { method, path, body })
}

#[derive(Debug, Clone, Serialize, Default)]
struct AppMcpSnapshot {
    app: &'static str,
    project_root: Option<String>,
    view_state: Option<AppMcpViewState>,
    timeline: Option<TimelineSummary>,
    timeline_error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct AppMcpViewState {
    stem: String,
    current_time_s: f64,
    is_playing: bool,
}

impl From<ViewState> for AppMcpViewState {
    fn from(value: ViewState) -> Self {
        Self {
            stem: value.stem,
            current_time_s: value.current_time_s,
            is_playing: value.is_playing,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct TimelineSummary {
    name: String,
    duration_s: f64,
    track_count: usize,
    clip_count: usize,
    gap_count: usize,
    transition_count: usize,
}

async fn collect_snapshot(app: &AppHandle) -> AppMcpSnapshot {
    let state = app.state::<MontageState>();
    let project_root = state.project_root.lock().await.clone();
    let view_state = state
        .view_state
        .lock()
        .await
        .clone()
        .map(AppMcpViewState::from);
    snapshot_from_parts(project_root, view_state)
}

fn snapshot_from_parts(
    project_root: Option<PathBuf>,
    view_state: Option<AppMcpViewState>,
) -> AppMcpSnapshot {
    let mut snapshot = AppMcpSnapshot {
        app: "Montage",
        project_root: project_root.as_ref().map(|path| path.display().to_string()),
        view_state,
        timeline: None,
        timeline_error: None,
    };

    if let Some(root) = project_root {
        match Project::read(&root) {
            Ok(project) => snapshot.timeline = Some(summarize_timeline(&project.timeline)),
            Err(err) => snapshot.timeline_error = Some(err.to_string()),
        }
    }

    snapshot
}

fn summarize_timeline(timeline: &Timeline) -> TimelineSummary {
    let mut counts = TimelineCounts::default();
    let duration_s = stack_duration_s(&timeline.tracks, &mut counts);
    TimelineSummary {
        name: timeline.name.clone(),
        duration_s,
        track_count: counts.track_count,
        clip_count: counts.clip_count,
        gap_count: counts.gap_count,
        transition_count: counts.transition_count,
    }
}

#[derive(Default)]
struct TimelineCounts {
    track_count: usize,
    clip_count: usize,
    gap_count: usize,
    transition_count: usize,
}

fn stack_duration_s(stack: &Stack, counts: &mut TimelineCounts) -> f64 {
    // Always walk children to populate the track/clip/gap/transition counters,
    // even when an explicit source_range overrides the reported duration —
    // otherwise a trimmed/imported root stack looks empty to MCP clients.
    let walked = stack
        .children
        .iter()
        .map(|child| stack_child_duration_s(child, counts))
        .fold(0.0, f64::max);

    match &stack.source_range {
        Some(range) => range.duration.to_seconds().max(0.0),
        None => walked,
    }
}

fn stack_child_duration_s(child: &StackChild, counts: &mut TimelineCounts) -> f64 {
    match child {
        StackChild::Track(track) => {
            counts.track_count += 1;
            track
                .children
                .iter()
                .map(|child| track_child_duration_s(child, counts))
                .sum()
        }
        StackChild::Stack(stack) => stack_duration_s(stack, counts),
        StackChild::Clip(clip) => {
            counts.clip_count += 1;
            clip.source_range
                .as_ref()
                .map(|range| range.duration.to_seconds().max(0.0))
                .unwrap_or(0.0)
        }
        StackChild::Gap(gap) => {
            counts.gap_count += 1;
            gap.source_range.duration.to_seconds().max(0.0)
        }
    }
}

fn track_child_duration_s(child: &TrackChild, counts: &mut TimelineCounts) -> f64 {
    match child {
        TrackChild::Clip(clip) => {
            counts.clip_count += 1;
            clip.source_range
                .as_ref()
                .map(|range| range.duration.to_seconds().max(0.0))
                .unwrap_or(0.0)
        }
        TrackChild::Gap(gap) => {
            counts.gap_count += 1;
            gap.source_range.duration.to_seconds().max(0.0)
        }
        TrackChild::Transition(_transition) => {
            counts.transition_count += 1;
            // Transitions overlap neighboring clips rather than adding track
            // time; the EDL cursor (apply.rs) treats them as zero duration.
            0.0
        }
        TrackChild::Stack(stack) => stack_duration_s(stack, counts),
    }
}

#[derive(Debug, Clone)]
struct ScreenshotHandler {
    mode: ScreenshotMode,
}

#[derive(Debug, Clone, Copy)]
enum ScreenshotMode {
    #[cfg(test)]
    Disabled,
    Desktop,
}

impl ScreenshotHandler {
    #[cfg(test)]
    fn disabled() -> Self {
        Self {
            mode: ScreenshotMode::Disabled,
        }
    }

    fn desktop() -> Self {
        Self {
            mode: ScreenshotMode::Desktop,
        }
    }

    fn capture(&self, arguments: &Value) -> Result<PathBuf, String> {
        match self.mode {
            #[cfg(test)]
            ScreenshotMode::Disabled => Err("screenshot capture disabled in this context".into()),
            ScreenshotMode::Desktop => capture_desktop_screenshot(arguments),
        }
    }
}

fn handle_json_rpc_value(
    request: Value,
    snapshot: AppMcpSnapshot,
    screenshot_handler: ScreenshotHandler,
) -> Value {
    // A JSON-RPC notification carries no id and must never receive a response
    // (e.g. notifications/initialized, cancellations, roots-change). Handle the
    // missing id once here so no id-less method/default response is emitted.
    if request.get("id").is_none() {
        return Value::Null;
    }
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    let method = request
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default();

    match method {
        "initialize" => success_response(
            id,
            json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {
                    "tools": {},
                    "resources": {}
                },
                "serverInfo": {
                    "name": "Montage Desktop",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }),
        ),
        "tools/list" => success_response(id, json!({ "tools": tool_definitions() })),
        "resources/list" => success_response(
            id,
            json!({
                "resources": [
                    {
                        "uri": "editor://state",
                        "name": "Current editor state",
                        "mimeType": "application/json"
                    },
                    {
                        "uri": "editor://timeline-summary",
                        "name": "Current timeline summary",
                        "mimeType": "application/json"
                    }
                ]
            }),
        ),
        "resources/read" => {
            let uri = request
                .get("params")
                .and_then(|params| params.get("uri"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            match resource_text(uri, &snapshot) {
                Ok(text) => success_response(
                    id,
                    json!({
                        "contents": [{
                            "uri": uri,
                            "mimeType": "application/json",
                            "text": text
                        }]
                    }),
                ),
                Err(message) => error_response(id, -32602, &message),
            }
        }
        "tools/call" => handle_tool_call(id, request.get("params"), snapshot, screenshot_handler),
        _ => error_response(id, -32601, &format!("method not found: {method}")),
    }
}

fn tool_definitions() -> Vec<Value> {
    vec![
        json!({
            "name": "get_editor_state",
            "description": "Return the live Montage desktop state: current project root, latest preview/view state pushed by the app, and timeline counts/duration read from project.otio.json.",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "required": []
            }
        }),
        json!({
            "name": "take_screenshot",
            "description": "Capture the current macOS desktop to a PNG and return the file path. Open the image to inspect the visible Montage editor.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "output_path": {
                        "type": "string",
                        "description": "Optional absolute PNG output path. Defaults to a file in the system temp directory."
                    }
                },
                "required": []
            }
        }),
    ]
}

fn handle_tool_call(
    id: Value,
    params: Option<&Value>,
    snapshot: AppMcpSnapshot,
    screenshot_handler: ScreenshotHandler,
) -> Value {
    let name = params
        .and_then(|params| params.get("name"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let arguments = params
        .and_then(|params| params.get("arguments"))
        .cloned()
        .unwrap_or_else(|| json!({}));

    match name {
        "get_editor_state" => match serde_json::to_string_pretty(&snapshot) {
            Ok(text) => tool_text_response(id, text, false),
            Err(err) => {
                tool_text_response(id, format!("failed to serialize editor state: {err}"), true)
            }
        },
        "take_screenshot" => match screenshot_handler.capture(&arguments) {
            Ok(path) => tool_text_response(
                id,
                json!({ "path": path.display().to_string() }).to_string(),
                false,
            ),
            Err(message) => tool_text_response(id, message, true),
        },
        "" => tool_text_response(id, "missing tool name".into(), true),
        other => tool_text_response(id, format!("unknown tool: {other}"), true),
    }
}

fn tool_text_response(id: Value, text: String, is_error: bool) -> Value {
    success_response(
        id,
        json!({
            "content": [{
                "type": "text",
                "text": text
            }],
            "isError": is_error
        }),
    )
}

fn resource_text(uri: &str, snapshot: &AppMcpSnapshot) -> Result<String, String> {
    match uri {
        "editor://state" => serde_json::to_string_pretty(snapshot)
            .map_err(|err| format!("failed to serialize editor state: {err}")),
        "editor://timeline-summary" => serde_json::to_string_pretty(&snapshot.timeline)
            .map_err(|err| format!("failed to serialize timeline summary: {err}")),
        _ => Err(format!("unknown resource: {uri}")),
    }
}

fn success_response(id: Value, result: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result
    })
}

fn error_response(id: Value, code: i64, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": code,
            "message": message
        }
    })
}

fn capture_desktop_screenshot(arguments: &Value) -> Result<PathBuf, String> {
    let output_path = match arguments.get("output_path").and_then(Value::as_str) {
        Some(path) if !path.trim().is_empty() => PathBuf::from(path),
        _ => default_screenshot_path()?,
    };

    capture_desktop_screenshot_to(&output_path)?;
    Ok(output_path)
}

fn default_screenshot_path() -> Result<PathBuf, String> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|err| format!("system clock before unix epoch: {err}"))?
        .as_millis();
    Ok(std::env::temp_dir().join(format!("montage-app-mcp-screenshot-{millis}.png")))
}

#[cfg(target_os = "macos")]
fn capture_desktop_screenshot_to(path: &Path) -> Result<(), String> {
    let status = std::process::Command::new("/usr/sbin/screencapture")
        .arg("-x")
        .arg(path)
        .status()
        .map_err(|err| format!("failed to run screencapture: {err}"))?;

    if status.success() {
        Ok(())
    } else {
        Err(format!("screencapture exited with status {status}"))
    }
}

#[cfg(not(target_os = "macos"))]
fn capture_desktop_screenshot_to(_path: &Path) -> Result<(), String> {
    Err("take_screenshot is currently implemented with macOS screencapture only".into())
}

#[derive(Debug, thiserror::Error)]
enum AppMcpError {
    #[error("HTTP error: {0}")]
    Http(String),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        AppMcpSnapshot, AppMcpViewState, ScreenshotHandler, handle_json_rpc_value,
        parse_http_request, snapshot_from_parts,
    };

    #[test]
    fn routes_initialize_and_lists_tools() {
        let response = handle_json_rpc_value(
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/list"
            }),
            AppMcpSnapshot::default(),
            ScreenshotHandler::disabled(),
        );

        assert_eq!(response["jsonrpc"], "2.0");
        let tools = response["result"]["tools"].as_array().unwrap();
        assert!(tools.iter().any(|tool| tool["name"] == "get_editor_state"));
        assert!(tools.iter().any(|tool| tool["name"] == "take_screenshot"));
    }

    #[test]
    fn routes_unknown_tool_as_mcp_tool_error() {
        let response = handle_json_rpc_value(
            json!({
                "jsonrpc": "2.0",
                "id": 7,
                "method": "tools/call",
                "params": {
                    "name": "missing_tool",
                    "arguments": {}
                }
            }),
            AppMcpSnapshot::default(),
            ScreenshotHandler::disabled(),
        );

        assert_eq!(response["result"]["isError"], true);
        assert!(
            response["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("unknown tool")
        );
    }

    #[test]
    fn parses_http_json_rpc_request() {
        let request = concat!(
            "POST /mcp HTTP/1.1\r\n",
            "Host: 127.0.0.1:8420\r\n",
            "Content-Length: 44\r\n",
            "\r\n",
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/list\"}"
        );

        let parsed = parse_http_request(request.as_bytes()).unwrap();

        assert_eq!(parsed.method, "POST");
        assert_eq!(parsed.path, "/mcp");
        assert_eq!(parsed.body["method"], "tools/list");
    }

    #[test]
    fn parses_content_length_after_request_line() {
        let headers = concat!(
            "POST /mcp HTTP/1.1\r\n",
            "Host: 127.0.0.1:8420\r\n",
            "Content-Type: application/json\r\n",
            "Content-Length: 46"
        );

        assert_eq!(super::parse_content_length(headers.as_bytes()), Some(46));
    }

    #[test]
    fn summarizes_project_timeline() {
        let dir = tempfile::tempdir().unwrap();
        montage_proto::project::Project::init(dir.path()).unwrap();

        let snapshot = snapshot_from_parts(
            Some(dir.path().to_path_buf()),
            Some(AppMcpViewState {
                stem: "camera-a".into(),
                current_time_s: 12.5,
                is_playing: false,
            }),
        );

        assert_eq!(
            snapshot.project_root.as_deref(),
            Some(dir.path().to_str().unwrap())
        );
        assert_eq!(snapshot.view_state.unwrap().stem, "camera-a");
        let timeline = snapshot.timeline.unwrap();
        assert_eq!(
            timeline.name,
            dir.path().file_name().unwrap().to_str().unwrap()
        );
        assert_eq!(timeline.track_count, 0);
        assert_eq!(timeline.clip_count, 0);
        assert_eq!(timeline.duration_s, 0.0);
        assert!(snapshot.timeline_error.is_none());
    }
}
