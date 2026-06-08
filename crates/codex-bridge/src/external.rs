use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use codex_app_server_protocol::ClientRequest;
use codex_app_server_protocol::RequestId;
use codex_app_server_protocol::ServerNotification;
use codex_app_server_protocol::ServerRequest;
use serde::de::DeserializeOwned;
use serde_json::json;
use tokio::io::AsyncBufReadExt;
use tokio::io::AsyncWriteExt;
use tokio::io::BufReader;
use tokio::process::Child;
use tokio::process::ChildStdin;
use tokio::process::Command;
use tokio::sync::Mutex;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use crate::BridgeError;

const DEFAULT_CODEX_BIN: &str = "codex";

type PendingResponse = oneshot::Sender<Result<serde_json::Value, String>>;
type PendingResponses = Arc<Mutex<HashMap<i64, PendingResponse>>>;

#[derive(Debug)]
pub enum ExternalServerEvent {
    Notification(ServerNotification),
    Request(ServerRequest),
}

pub struct ExternalAppServerClient {
    stdin: Arc<Mutex<ChildStdin>>,
    pending: PendingResponses,
    next_request_id: AtomicI64,
    child: Arc<Mutex<Child>>,
    reader_task: JoinHandle<()>,
}

impl ExternalAppServerClient {
    pub async fn start(
        project_root: &Path,
        mcp_server_path: Option<PathBuf>,
    ) -> Result<(Self, mpsc::Receiver<ExternalServerEvent>), BridgeError> {
        let codex_bin =
            std::env::var("MONTAGE_CODEX_BIN").unwrap_or_else(|_| DEFAULT_CODEX_BIN.to_string());
        let args = app_server_args(mcp_server_path.as_deref(), project_root);
        let mut command = Command::new(codex_bin);
        command
            .args(args)
            .current_dir(project_root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true);

        let mut child = command
            .spawn()
            .map_err(|e| BridgeError::Startup(format!("spawn codex app-server: {e}")))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| BridgeError::Startup("codex app-server stdin unavailable".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| BridgeError::Startup("codex app-server stdout unavailable".into()))?;

        let pending = Arc::new(Mutex::new(HashMap::new()));
        let (event_tx, event_rx) = mpsc::channel(128);
        let reader_pending = Arc::clone(&pending);
        let reader_task = tokio::spawn(async move {
            read_stdout(stdout, reader_pending, event_tx).await;
        });

        Ok((
            Self {
                stdin: Arc::new(Mutex::new(stdin)),
                pending,
                next_request_id: AtomicI64::new(1),
                child: Arc::new(Mutex::new(child)),
                reader_task,
            },
            event_rx,
        ))
    }

    pub fn next_request_id(&self) -> i64 {
        self.next_request_id.fetch_add(1, Ordering::Relaxed)
    }

    pub async fn request<T: DeserializeOwned>(
        &self,
        request: ClientRequest,
    ) -> Result<T, BridgeError> {
        let value = serde_json::to_value(&request)
            .map_err(|e| BridgeError::Request(format!("serialize request: {e}")))?;
        let id = value
            .get("id")
            .and_then(|id| id.as_i64())
            .ok_or_else(|| BridgeError::Request("request missing integer id".into()))?;
        let result = self.request_value(id, value).await?;
        serde_json::from_value(result)
            .map_err(|e| BridgeError::Request(format!("deserialize response: {e}")))
    }

    pub async fn send_notification(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<(), BridgeError> {
        let value = match params {
            Some(params) => json!({ "method": method, "params": params }),
            None => json!({ "method": method }),
        };
        self.write_json(value).await
    }

    pub async fn resolve(
        &self,
        request_id: RequestId,
        result: serde_json::Value,
    ) -> Result<(), BridgeError> {
        let id = serde_json::to_value(request_id)
            .map_err(|e| BridgeError::Resolve(format!("serialize request id: {e}")))?;
        self.write_json(json!({ "id": id, "result": result })).await
    }

    pub async fn shutdown(&self) {
        self.reader_task.abort();
        let mut child = self.child.lock().await;
        let _ = child.kill().await;
    }

    async fn request_value(
        &self,
        id: i64,
        value: serde_json::Value,
    ) -> Result<serde_json::Value, BridgeError> {
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);
        if let Err(e) = self.write_json(value).await {
            self.pending.lock().await.remove(&id);
            return Err(e);
        }
        rx.await
            .map_err(|_| BridgeError::Request("response channel dropped".into()))?
            .map_err(BridgeError::Request)
    }

    async fn write_json(&self, value: serde_json::Value) -> Result<(), BridgeError> {
        let mut stdin = self.stdin.lock().await;
        let payload = serde_json::to_string(&value)
            .map_err(|e| BridgeError::Request(format!("serialize jsonrpc message: {e}")))?;
        stdin
            .write_all(payload.as_bytes())
            .await
            .map_err(|e| BridgeError::Request(format!("write jsonrpc message: {e}")))?;
        stdin
            .write_all(b"\n")
            .await
            .map_err(|e| BridgeError::Request(format!("write jsonrpc newline: {e}")))?;
        stdin
            .flush()
            .await
            .map_err(|e| BridgeError::Request(format!("flush jsonrpc message: {e}")))
    }
}

pub fn app_server_args(mcp_server_path: Option<&Path>, project_root: &Path) -> Vec<String> {
    let mut args = vec![
        "app-server".to_string(),
        "--listen".to_string(),
        "stdio://".to_string(),
        "-c".to_string(),
        "model_auto_compact_token_limit=200000".to_string(),
    ];
    if let Some(mcp_path) = mcp_server_path {
        args.extend([
            "-c".to_string(),
            format_toml_string_override("mcp_servers.montage.command", mcp_path),
            "-c".to_string(),
            format_toml_string_override(
                "mcp_servers.montage.env.MONTAGE_PROJECT_ROOT",
                project_root,
            ),
        ]);
    }
    args
}

fn format_toml_string_override(key: &str, path: &Path) -> String {
    let value = toml::Value::String(path.display().to_string()).to_string();
    format!("{key}={value}")
}

async fn read_stdout(
    stdout: tokio::process::ChildStdout,
    pending: PendingResponses,
    event_tx: mpsc::Sender<ExternalServerEvent>,
) {
    let mut lines = BufReader::new(stdout).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        if let Some(id) = response_id(&value) {
            if let Some(tx) = pending.lock().await.remove(&id) {
                let result = if let Some(error) = value.get("error") {
                    Err(error.to_string())
                } else {
                    Ok(value
                        .get("result")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null))
                };
                let _ = tx.send(result);
            }
            continue;
        }
        if value.get("method").is_none() {
            continue;
        }
        let event = if value.get("id").is_some() {
            serde_json::from_value::<ServerRequest>(value).map(ExternalServerEvent::Request)
        } else {
            serde_json::from_value::<ServerNotification>(value)
                .map(ExternalServerEvent::Notification)
        };
        if let Ok(event) = event {
            let _ = event_tx.send(event).await;
        }
    }
}

fn response_id(value: &serde_json::Value) -> Option<i64> {
    if value.get("method").is_some() {
        return None;
    }
    value.get("id")?.as_i64()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_server_args_include_stdio_and_montage_mcp_overrides() {
        let args = app_server_args(
            Some(Path::new("/bin/montage-mcp-server")),
            Path::new("/tmp/p"),
        );
        assert!(args.windows(3).any(|w| w == ["--listen", "stdio://", "-c"]));
        assert!(
            args.iter()
                .any(|arg| arg == "mcp_servers.montage.command=\"/bin/montage-mcp-server\"")
        );
        assert!(
            args.iter()
                .any(|arg| arg == "mcp_servers.montage.env.MONTAGE_PROJECT_ROOT=\"/tmp/p\"")
        );
    }

    #[test]
    fn app_server_args_allow_missing_mcp_path_for_degraded_startup() {
        let args = app_server_args(None, Path::new("/tmp/p"));
        assert_eq!(
            args,
            [
                "app-server",
                "--listen",
                "stdio://",
                "-c",
                "model_auto_compact_token_limit=200000"
            ]
        );
    }

    #[test]
    fn response_id_ignores_server_requests() {
        let request = json!({"id": 1, "method": "item/tool/requestUserInput", "params": {}});
        assert_eq!(response_id(&request), None);
        let response = json!({"id": 2, "result": {}});
        assert_eq!(response_id(&response), Some(2));
    }
}
