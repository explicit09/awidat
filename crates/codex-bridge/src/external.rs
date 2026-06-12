use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

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
use crate::wire;
use crate::wire::ServerNotification;
use crate::wire::ServerRequest;

const DEFAULT_CODEX_BIN: &str = "codex";
const METHOD_NOT_FOUND: i64 = -32601;
const SERVER_ERROR: i64 = -32000;
const AUTH_REFRESH_UNSUPPORTED: &str =
    "chatgpt auth token refresh is not supported for external app-server runtime";
const PERMISSION_MODE_FILE: &str = "permission_mode";
const SUPPORTED_SERVER_REQUEST_METHODS: &[&str] = &[
    "item/commandExecution/requestApproval",
    "item/fileChange/requestApproval",
    "item/permissions/requestApproval",
    "item/tool/requestUserInput",
];

type PendingResponse = oneshot::Sender<Result<serde_json::Value, String>>;
type PendingResponses = Arc<Mutex<HashMap<i64, PendingResponse>>>;
type SharedStdin = Arc<Mutex<ChildStdin>>;

#[derive(Debug)]
pub enum ExternalServerEvent {
    Notification(wire::ServerNotification),
    Request(wire::ServerRequest),
}

pub struct ExternalAppServerClient {
    stdin: SharedStdin,
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
        let codex_bin = resolve_codex_bin();
        // Bundled sidecars (codex, rg, ffmpeg, …) live next to codex_bin. Capture
        // that dir before codex_bin is moved into the command so we can put it on
        // the agent's PATH below.
        let sidecar_dir = codex_bin
            .parent()
            .filter(|dir| !dir.as_os_str().is_empty())
            .map(|dir| dir.to_path_buf());
        let args = app_server_args(mcp_server_path.as_deref(), project_root);
        let mut command = Command::new(codex_bin);
        command
            .args(args)
            .current_dir(project_root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true);
        // Prepend the bundled sidecar dir to PATH so codex's shell tool can find
        // bundled binaries like `rg` in a packaged app, where the GUI-inherited
        // PATH is minimal and excludes the app bundle's sidecar location.
        if let Some(dir) = sidecar_dir {
            let existing = std::env::var_os("PATH").unwrap_or_default();
            let mut entries = vec![dir];
            entries.extend(std::env::split_paths(&existing));
            if let Ok(joined) = std::env::join_paths(entries) {
                command.env("PATH", joined);
            }
        }

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

        let stdin = Arc::new(Mutex::new(stdin));
        let pending = Arc::new(Mutex::new(HashMap::new()));
        let (event_tx, event_rx) = mpsc::channel(128);
        let reader_pending = Arc::clone(&pending);
        let reader_stdin = Arc::clone(&stdin);
        let reader_task = tokio::spawn(async move {
            read_stdout(stdout, reader_pending, reader_stdin, event_tx).await;
        });

        Ok((
            Self {
                stdin,
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
        value: serde_json::Value,
    ) -> Result<T, BridgeError> {
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
        request_id: wire::RequestId,
        result: serde_json::Value,
    ) -> Result<(), BridgeError> {
        self.write_json(json!({ "id": request_id, "result": result }))
            .await
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
        write_json_to_stdin(&self.stdin, value)
            .await
            .map_err(BridgeError::Request)
    }
}

pub fn app_server_args(mcp_server_path: Option<&Path>, project_root: &Path) -> Vec<String> {
    app_server_args_with_openrouter_cost_estimate(
        mcp_server_path,
        project_root,
        std::env::var("OPENROUTER_VIDEO_COST_ESTIMATE_USD")
            .ok()
            .as_deref(),
    )
}

/// Custom auto-compaction prompt. Codex's default compact prompt is a
/// generic "handoff summary" — it drops the facts a mid-production
/// editing agent needs to keep going: which skills were loaded (their
/// L2 bodies are tool outputs, which compaction discards entirely),
/// which playbook stage is in flight, and what edits already landed.
/// Without these, the post-compaction model improvises the rest of the
/// pipeline instead of following the playbook. See
/// vendor/codex-rs/core/src/compact.rs for what survives compaction:
/// initial context + ~20k tokens of recent user messages + this
/// summary; everything else is gone.
const COMPACT_PROMPT: &str = "\
You are performing a CONTEXT CHECKPOINT COMPACTION for a video-editing agent \
that is mid-task. Write a handoff summary so another LLM can resume the SAME \
production without losing the workflow.

Include, in this order:
1. The user's goal, the active workflow/playbook, and the current stage — \
list stages already DONE and stages REMAINING.
2. Skills loaded via load_skill (exact names). State explicitly that the \
resuming LLM MUST call load_skill again for each one before continuing — \
skill playbook bodies do NOT survive compaction.
3. Timeline/edit state: edits applied (clip/proposal ids and source ranges), \
accepted-but-unapplied proposal ids, and anything rolled back or failed.
4. Key decisions, measurements, thresholds, constraints, and user \
preferences already established.
5. Concrete next steps, in playbook order.

Never present the task as complete while playbook stages remain. Do not drop \
the stage list to save space.";

fn app_server_args_with_openrouter_cost_estimate(
    mcp_server_path: Option<&Path>,
    project_root: &Path,
    openrouter_cost_estimate_usd: Option<&str>,
) -> Vec<String> {
    let mut args = vec![
        "app-server".to_string(),
        "--listen".to_string(),
        "stdio://".to_string(),
        "--enable-codex-api-key-env".to_string(),
        "-c".to_string(),
        "model_auto_compact_token_limit=200000".to_string(),
        "-c".to_string(),
        format_toml_string_override_value("compact_prompt", COMPACT_PROMPT),
        "-c".to_string(),
        "project_doc_max_bytes=0".to_string(),
        "-c".to_string(),
        format_toml_string_override_value("approval_policy", codex_approval_policy(project_root)),
        "-c".to_string(),
        "features.tool_search_always_defer_mcp_tools=true".to_string(),
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
        if let Some(estimate) = openrouter_cost_estimate_usd
            && !estimate.trim().is_empty()
        {
            args.extend([
                "-c".to_string(),
                format_toml_string_override_value(
                    "mcp_servers.montage.env.OPENROUTER_VIDEO_COST_ESTIMATE_USD",
                    estimate.trim(),
                ),
            ]);
        }
    }
    args
}

fn codex_approval_policy(project_root: &Path) -> &'static str {
    let path = project_root.join(".montage").join(PERMISSION_MODE_FILE);
    match std::fs::read_to_string(path).ok().as_deref().map(str::trim) {
        Some("autopilot") => "never",
        Some("manual" | "copilot") | None => "on-request",
        Some(_) => "on-request",
    }
}

fn format_toml_string_override(key: &str, path: &Path) -> String {
    format_toml_string_override_value(key, &path.display().to_string())
}

fn format_toml_string_override_value(key: &str, raw: &str) -> String {
    // Always emit a single-line TOML basic string. The toml crate's
    // Display switches to `"""` multi-line form for values containing
    // newlines, which codex's `-c` value parser rejects — escape
    // control characters instead.
    let mut value = String::with_capacity(raw.len() + 2);
    value.push('"');
    for c in raw.chars() {
        match c {
            '"' => value.push_str("\\\""),
            '\\' => value.push_str("\\\\"),
            '\n' => value.push_str("\\n"),
            '\r' => value.push_str("\\r"),
            '\t' => value.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                value.push_str(&format!("\\u{:04X}", c as u32));
            }
            c => value.push(c),
        }
    }
    value.push('"');
    format!("{key}={value}")
}

fn resolve_codex_bin() -> PathBuf {
    if let Some(path) = std::env::var_os("MONTAGE_CODEX_BIN").map(PathBuf::from) {
        return path;
    }
    if let Some(path) = bundled_codex_bin() {
        return path;
    }
    PathBuf::from(DEFAULT_CODEX_BIN)
}

fn bundled_codex_bin() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let parent = exe.parent()?;
    for name in bundled_codex_binary_names() {
        let candidate = parent.join(&name);
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

fn bundled_codex_binary_names() -> Vec<String> {
    let mut names = Vec::new();
    if let Some(triple) = current_target_triple() {
        names.push(format!("codex-{triple}"));
        if cfg!(windows) {
            names.push(format!("codex-{triple}.exe"));
        }
    }
    if cfg!(windows) {
        names.push("codex.exe".to_string());
    }
    names.push("codex".to_string());
    names
}

fn current_target_triple() -> Option<&'static str> {
    match (std::env::consts::ARCH, std::env::consts::OS) {
        ("aarch64", "macos") => Some("aarch64-apple-darwin"),
        ("x86_64", "macos") => Some("x86_64-apple-darwin"),
        ("aarch64", "linux") => Some("aarch64-unknown-linux-gnu"),
        ("x86_64", "linux") => Some("x86_64-unknown-linux-gnu"),
        ("x86_64", "windows") => Some("x86_64-pc-windows-msvc"),
        _ => None,
    }
}

async fn read_stdout(
    stdout: tokio::process::ChildStdout,
    pending: PendingResponses,
    stdin: SharedStdin,
    event_tx: mpsc::Sender<ExternalServerEvent>,
) {
    let mut lines = BufReader::new(stdout).lines();
    loop {
        let line = match lines.next_line().await {
            Ok(Some(line)) => line,
            Ok(None) => {
                fail_pending(&pending, "codex app-server stdout closed").await;
                break;
            }
            Err(e) => {
                fail_pending(&pending, format!("read codex app-server stdout: {e}")).await;
                break;
            }
        };

        if line.trim().is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        handle_jsonrpc_message(value, &pending, &stdin, &event_tx).await;
    }
}

async fn handle_jsonrpc_message(
    value: serde_json::Value,
    pending: &PendingResponses,
    stdin: &SharedStdin,
    event_tx: &mpsc::Sender<ExternalServerEvent>,
) {
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
        return;
    }
    if value.get("method").is_none() {
        return;
    }

    if value.get("id").is_some() {
        handle_server_request_message(value, stdin, event_tx).await;
        return;
    }

    if let Ok(notification) = serde_json::from_value::<ServerNotification>(value) {
        let _ = event_tx
            .send(ExternalServerEvent::Notification(notification))
            .await;
    }
}

async fn handle_server_request_message(
    value: serde_json::Value,
    stdin: &SharedStdin,
    event_tx: &mpsc::Sender<ExternalServerEvent>,
) {
    let raw_id = value.get("id").cloned();
    let method = value
        .get("method")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("<unknown>")
        .to_string();

    match serde_json::from_value::<ServerRequest>(value) {
        Ok(request) if request.method == "account/chatgptAuthTokens/refresh" => {
            let _ = write_error_response(stdin, request.id, SERVER_ERROR, AUTH_REFRESH_UNSUPPORTED)
                .await;
        }
        Ok(request) if is_supported_server_request_method(&request.method) => {
            let _ = event_tx.send(ExternalServerEvent::Request(request)).await;
        }
        Ok(request) => {
            let message = format!(
                "unsupported external app-server request `{}`",
                request.method
            );
            let _ = write_error_response(stdin, request.id, METHOD_NOT_FOUND, &message).await;
        }
        Err(_) => {
            if let Some(id) = raw_id {
                let message = format!("unsupported external app-server request `{method}`");
                let _ = write_error_response(stdin, id, METHOD_NOT_FOUND, &message).await;
            }
        }
    }
}

fn is_supported_server_request_method(method: &str) -> bool {
    SUPPORTED_SERVER_REQUEST_METHODS.contains(&method)
}

async fn fail_pending(pending: &PendingResponses, message: impl Into<String>) {
    let message = message.into();
    let mut pending = pending.lock().await;
    for (_, tx) in pending.drain() {
        let _ = tx.send(Err(message.clone()));
    }
}

async fn write_error_response(
    stdin: &SharedStdin,
    id: serde_json::Value,
    code: i64,
    message: &str,
) -> Result<(), String> {
    write_json_to_stdin(stdin, error_response(id, code, message)).await
}

async fn write_json_to_stdin(stdin: &SharedStdin, value: serde_json::Value) -> Result<(), String> {
    let mut stdin = stdin.lock().await;
    let payload =
        serde_json::to_string(&value).map_err(|e| format!("serialize jsonrpc message: {e}"))?;
    stdin
        .write_all(payload.as_bytes())
        .await
        .map_err(|e| format!("write jsonrpc message: {e}"))?;
    stdin
        .write_all(b"\n")
        .await
        .map_err(|e| format!("write jsonrpc newline: {e}"))?;
    stdin
        .flush()
        .await
        .map_err(|e| format!("flush jsonrpc message: {e}"))
}

fn error_response(id: serde_json::Value, code: i64, message: &str) -> serde_json::Value {
    json!({
        "id": id,
        "error": {
            "code": code,
            "message": message,
            "data": null,
        },
    })
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
    use std::sync::atomic::{AtomicUsize, Ordering};

    static TEST_DIR_COUNTER: AtomicUsize = AtomicUsize::new(0);

    fn test_project_dir(name: &str) -> PathBuf {
        let nonce = TEST_DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "montage-codex-bridge-{name}-{}-{nonce}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create test project dir");
        dir
    }

    #[test]
    fn app_server_args_include_stdio_and_montage_mcp_overrides() {
        let dir = test_project_dir("mcp-overrides");
        let args = app_server_args(Some(Path::new("/bin/montage-mcp-server")), &dir);
        assert!(
            args.windows(3)
                .any(|w| w == ["--listen", "stdio://", "--enable-codex-api-key-env"])
        );
        assert!(
            args.iter()
                .any(|arg| arg == "mcp_servers.montage.command=\"/bin/montage-mcp-server\"")
        );
        let project_override = format!(
            "mcp_servers.montage.env.MONTAGE_PROJECT_ROOT=\"{}\"",
            dir.display()
        );
        assert!(args.iter().any(|arg| arg == &project_override));
        assert!(
            args.iter()
                .any(|arg| arg == "features.tool_search_always_defer_mcp_tools=true")
        );
    }

    #[test]
    fn app_server_args_forward_openrouter_cost_estimate_to_mcp() {
        let dir = test_project_dir("cost-estimate");
        let args = app_server_args_with_openrouter_cost_estimate(
            Some(Path::new("/bin/montage-mcp-server")),
            &dir,
            Some("0.42"),
        );
        assert!(args.iter().any(|arg| {
            arg == "mcp_servers.montage.env.OPENROUTER_VIDEO_COST_ESTIMATE_USD=\"0.42\""
        }));
    }

    #[test]
    fn app_server_args_allow_missing_mcp_path_for_degraded_startup() {
        let dir = test_project_dir("missing-mcp");
        let args = app_server_args(None, &dir);
        let expected: Vec<String> = vec![
            "app-server".into(),
            "--listen".into(),
            "stdio://".into(),
            "--enable-codex-api-key-env".into(),
            "-c".into(),
            "model_auto_compact_token_limit=200000".into(),
            "-c".into(),
            format_toml_string_override_value("compact_prompt", COMPACT_PROMPT),
            "-c".into(),
            "project_doc_max_bytes=0".into(),
            "-c".into(),
            "approval_policy=\"on-request\"".into(),
            "-c".into(),
            "features.tool_search_always_defer_mcp_tools=true".into(),
        ];
        assert_eq!(args, expected);
    }

    #[test]
    fn compact_prompt_override_survives_toml_round_trip() {
        // Mirror codex's `-c` parsing exactly (utils/cli
        // config_override.rs): split on the first '=', then parse the
        // value side as a TOML value via a sentinel table. The prompt
        // is multi-line, so this guards the single-line escaping in
        // format_toml_string_override_value.
        let override_arg = format_toml_string_override_value("compact_prompt", COMPACT_PROMPT);
        let (key, value_str) = override_arg.split_once('=').expect("key=value form");
        assert_eq!(key, "compact_prompt");
        let wrapped = format!("_x_ = {value_str}");
        let table: toml::Table = toml::from_str(&wrapped).expect("valid TOML value");
        assert_eq!(
            table.get("_x_").and_then(toml::Value::as_str),
            Some(COMPACT_PROMPT),
        );
        assert!(COMPACT_PROMPT.contains("load_skill again"));
    }

    #[test]
    fn app_server_args_default_old_projects_to_manual_approval() {
        let dir = test_project_dir("default-manual");
        let args = app_server_args(None, &dir);
        assert!(
            args.iter()
                .any(|arg| arg == "approval_policy=\"on-request\"")
        );
    }

    #[test]
    fn app_server_args_map_autopilot_to_codex_never() {
        let dir = test_project_dir("autopilot");
        std::fs::create_dir_all(dir.join(".montage")).expect("create .montage");
        std::fs::write(dir.join(".montage/permission_mode"), "autopilot")
            .expect("write permission mode");

        let args = app_server_args(None, &dir);

        assert!(args.iter().any(|arg| arg == "approval_policy=\"never\""));
    }

    #[test]
    fn response_id_ignores_server_requests() {
        let request = json!({"id": 1, "method": "item/tool/requestUserInput", "params": {}});
        assert_eq!(response_id(&request), None);
        let response = json!({"id": 2, "result": {}});
        assert_eq!(response_id(&response), Some(2));
    }

    #[test]
    fn codex_crates_are_not_required_default_dependencies() {
        let manifest =
            std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"))
                .expect("read manifest");
        let parsed: toml::Value = toml::from_str(&manifest).expect("parse manifest");
        let dependencies = parsed
            .get("dependencies")
            .and_then(toml::Value::as_table)
            .expect("dependencies table");

        for name in [
            "codex-app-server-client",
            "codex-app-server-protocol",
            "codex-config",
            "codex-core",
            "codex-protocol",
            "codex-arg0",
            "codex-feedback",
        ] {
            let dependency = dependencies.get(name).expect("codex dependency exists");
            let optional = dependency
                .get("optional")
                .and_then(toml::Value::as_bool)
                .unwrap_or(false);
            assert!(optional, "{name} must be optional");
        }

        let features = parsed
            .get("features")
            .and_then(toml::Value::as_table)
            .expect("features table");
        assert!(
            features.contains_key("in-process-codex"),
            "in-process-codex feature must own vendored Codex dependencies"
        );
    }

    #[tokio::test]
    async fn fail_pending_wakes_waiters_with_error() {
        let pending = Arc::new(Mutex::new(HashMap::new()));
        let (tx, rx) = oneshot::channel();
        pending.lock().await.insert(7, tx);

        fail_pending(&pending, "codex app-server exited").await;

        assert!(pending.lock().await.is_empty());
        match rx.await {
            Ok(Err(message)) => assert_eq!(message, "codex app-server exited"),
            other => panic!("expected pending waiter error, got {other:?}"),
        }
    }

    #[test]
    fn error_response_uses_jsonrpc_error_shape() {
        assert_eq!(
            error_response(
                json!(9),
                METHOD_NOT_FOUND,
                "unsupported external app-server request"
            ),
            json!({
                "id": 9,
                "error": {
                    "code": -32601,
                    "message": "unsupported external app-server request",
                    "data": null,
                },
            })
        );
    }

    #[test]
    fn bundled_codex_binary_names_include_tauri_sidecar_name() {
        let names = bundled_codex_binary_names();
        if let Some(triple) = current_target_triple() {
            assert!(
                names.contains(&format!("codex-{triple}")),
                "expected suffixed Tauri sidecar name in {names:?}"
            );
        }
        assert!(names.contains(&DEFAULT_CODEX_BIN.to_string()));
    }

    #[test]
    fn supported_server_request_policy_rejects_unknown_methods() {
        assert!(is_supported_server_request_method(
            "item/commandExecution/requestApproval"
        ));
        assert!(is_supported_server_request_method(
            "item/tool/requestUserInput"
        ));
        assert!(!is_supported_server_request_method(
            "mcpServer/elicitation/request"
        ));
        assert!(!is_supported_server_request_method("future/server/request"));
    }
}
