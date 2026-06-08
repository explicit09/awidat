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
