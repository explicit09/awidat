use serde::Deserialize;
use serde_json::json;

pub type RequestId = serde_json::Value;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadResponse {
    pub thread: ThreadSummary,
}

#[derive(Debug, Deserialize)]
pub struct ThreadSummary {
    pub id: String,
}

#[derive(Debug, Deserialize)]
pub struct TurnStartResponse {
    pub turn: TurnSummary,
}

#[derive(Debug, Deserialize)]
pub struct TurnSummary {
    pub id: String,
}

#[derive(Debug, Deserialize)]
pub struct ServerNotification {
    pub method: String,
    #[serde(default)]
    pub params: serde_json::Value,
}

#[derive(Debug, Deserialize)]
pub struct ServerRequest {
    pub id: RequestId,
    pub method: String,
    #[serde(default)]
    pub params: serde_json::Value,
}

pub fn initialize_request(id: i64, client_version: &str) -> serde_json::Value {
    json!({
        "id": id,
        "method": "initialize",
        "params": {
            "clientInfo": {
                "name": "montage-desktop",
                "title": "Montage Desktop",
                "version": client_version,
            },
            "capabilities": {
                "experimentalApi": true,
            },
        },
    })
}

/// Montage's default model for a freshly started thread when the caller
/// hasn't chosen one. gpt-5.6-terra is the refreshed model catalog's
/// "balanced agentic coding model for everyday work" — see
/// vendor/codex-rs/models-manager/models.json — and replaces gpt-5.4, which
/// the catalog no longer lists (`visibility: "hide"`). Montage pins this
/// explicitly rather than relying on the app-server's own catalog-order
/// default (currently gpt-5.6-sol, the first `visibility: "list"` entry),
/// since that ordering is upstream's to change on any future refresh.
/// Resuming an existing thread (`thread_resume_request`) intentionally does
/// not apply this — a resumed thread keeps whatever model it already used.
pub const MONTAGE_DEFAULT_MODEL: &str = "gpt-5.6-terra";

pub fn thread_start_request(
    id: i64,
    project_root: &std::path::Path,
    developer_instructions: Option<String>,
) -> serde_json::Value {
    json!({
        "id": id,
        "method": "thread/start",
        "params": {
            "cwd": project_root.display().to_string(),
            "developerInstructions": developer_instructions,
            "model": MONTAGE_DEFAULT_MODEL,
        },
    })
}

pub fn thread_resume_request(
    id: i64,
    thread_id: String,
    project_root: &std::path::Path,
    developer_instructions: Option<String>,
) -> serde_json::Value {
    json!({
        "id": id,
        "method": "thread/resume",
        "params": {
            "threadId": thread_id,
            "cwd": project_root.display().to_string(),
            "developerInstructions": developer_instructions,
        },
    })
}

pub fn turn_start_request(
    id: i64,
    thread_id: &str,
    text: String,
    model: Option<String>,
) -> serde_json::Value {
    json!({
        "id": id,
        "method": "turn/start",
        "params": {
            "threadId": thread_id,
            "input": [{
                "type": "text",
                "text": text,
                "textElements": [],
            }],
            "model": model,
        },
    })
}

pub fn turn_interrupt_request(id: i64, thread_id: &str, turn_id: &str) -> serde_json::Value {
    json!({
        "id": id,
        "method": "turn/interrupt",
        "params": {
            "threadId": thread_id,
            "turnId": turn_id,
        },
    })
}
