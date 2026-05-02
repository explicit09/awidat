//! stdio transport: framing + request/response pump.
//!
//! MCP's stdio transport (per spec doc `2025-06-18`/`2025-11-25`) is
//! **newline-delimited JSON** in both directions, and the server MAY emit
//! arbitrary UTF-8 to stderr for logging. There is *no* `Content-Length:`
//! header in front of frames; this is the one place MCP visibly diverges
//! from the Language Server Protocol it otherwise inspires. We keep this
//! invariant local to this module: everything above sees a request/response
//! API.
//!
//! Architecture:
//!
//! - One reader task drains stdout line-by-line, decodes each JSON-RPC
//!   message, and dispatches: responses go to a `pending` map keyed by
//!   request id, notifications get logged-then-dropped (Week 2 has no
//!   notification subscribers).
//! - One writer task drains an outbound mpsc channel, serializing each
//!   payload + `\n` to stdin. Centralizing writes through the channel makes
//!   stdin access non-aliasing and lets request issuers stay synchronous.
//! - One stderr task drains stderr into a small ring buffer that we surface
//!   on `ServerCrashed` errors.
//! - One waiter task `await`s the child's exit status. If it exits while
//!   any in-flight request is pending, we wake those waiters with
//!   `ServerCrashed`.
//!
//! Shutdown drops the writer (closing stdin), then waits up to 2s for the
//! child to exit, then `kill`s it. Drop reuses this path synchronously by
//! sending the kill signal directly — we don't block on async in `Drop`.

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command};
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio::task::JoinHandle;
use tokio::time::timeout;

use crate::error::McpError;
use crate::protocol::{JsonRpcNotification, JsonRpcRequest, JsonRpcResponse};

/// Cap on the bytes of stderr we keep around for diagnostics on a crash.
const STDERR_RING_CAP: usize = 4096;

/// Default request timeout. Indexers can run for minutes (whisper on a
/// long episode is the worst case), so this is intentionally generous;
/// shorter timeouts are layered above when a tool call is known to be quick.
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(60 * 30);

/// One in-flight request awaiting a response.
type PendingMap = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<serde_json::Value, McpError>>>>>;

/// Owned handle to a launched MCP server. Spawning logic lives in
/// [`Transport::launch`]; the public `Client` wraps this with the high-level
/// MCP request shapes.
pub(crate) struct Transport {
    server_name: String,
    next_id: u64,
    write_tx: mpsc::Sender<WriteMsg>,
    pending: PendingMap,
    stderr_buf: Arc<Mutex<RingBuffer>>,
    /// Kept so dropping the transport tears the child down even if the
    /// caller forgot to `shutdown()`.
    child: Arc<Mutex<Option<Child>>>,
    /// Background tasks — joined on `shutdown`.
    tasks: Vec<JoinHandle<()>>,
}

enum WriteMsg {
    Frame(Vec<u8>),
    Close,
}

/// Bounded ring buffer for stderr capture.
struct RingBuffer {
    buf: Vec<u8>,
    cap: usize,
}

impl RingBuffer {
    fn new(cap: usize) -> Self {
        Self {
            buf: Vec::with_capacity(cap),
            cap,
        }
    }
    fn push(&mut self, bytes: &[u8]) {
        let total = self.buf.len() + bytes.len();
        if total > self.cap {
            // Drop oldest bytes to make room.
            let drop = total - self.cap;
            if drop >= self.buf.len() {
                self.buf.clear();
                let start = bytes.len().saturating_sub(self.cap);
                self.buf.extend_from_slice(&bytes[start..]);
                return;
            }
            self.buf.drain(..drop);
        }
        self.buf.extend_from_slice(bytes);
    }
    fn snapshot_lossy(&self) -> String {
        String::from_utf8_lossy(&self.buf).into_owned()
    }
}

impl Transport {
    /// Spawn the configured server and start the I/O pump tasks. The first
    /// JSON-RPC call is now possible; the caller is responsible for the
    /// MCP `initialize` handshake.
    pub(crate) async fn launch(
        server_name: String,
        cmd: Command,
    ) -> Result<Self, McpError> {
        let mut command = cmd;
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let mut child = command
            .spawn()
            .map_err(|e| McpError::Spawn {
                server: server_name.clone(),
                source: e,
            })?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| McpError::StdioAttach {
                server: server_name.clone(),
                message: "stdin pipe missing".into(),
            })?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| McpError::StdioAttach {
                server: server_name.clone(),
                message: "stdout pipe missing".into(),
            })?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| McpError::StdioAttach {
                server: server_name.clone(),
                message: "stderr pipe missing".into(),
            })?;

        let pending: PendingMap = Arc::new(Mutex::new(HashMap::new()));
        let stderr_buf = Arc::new(Mutex::new(RingBuffer::new(STDERR_RING_CAP)));
        let (write_tx, write_rx) = mpsc::channel::<WriteMsg>(64);
        let child_arc: Arc<Mutex<Option<Child>>> = Arc::new(Mutex::new(Some(child)));

        let mut tasks = Vec::new();
        tasks.push(spawn_writer(server_name.clone(), stdin, write_rx));
        tasks.push(spawn_reader(
            server_name.clone(),
            stdout,
            pending.clone(),
        ));
        tasks.push(spawn_stderr(stderr, stderr_buf.clone()));
        tasks.push(spawn_waiter(
            server_name.clone(),
            child_arc.clone(),
            pending.clone(),
            stderr_buf.clone(),
        ));

        Ok(Self {
            server_name,
            next_id: 1,
            write_tx,
            pending,
            stderr_buf,
            child: child_arc,
            tasks,
        })
    }

    /// Server name (echoed by error variants).
    pub(crate) fn server_name(&self) -> &str {
        &self.server_name
    }

    /// Issue a JSON-RPC request and await a response. Times out per the
    /// supplied `req_timeout` (or [`DEFAULT_REQUEST_TIMEOUT`] if `None`).
    pub(crate) async fn request<P: Serialize>(
        &mut self,
        method: &str,
        params: Option<P>,
        req_timeout: Option<Duration>,
    ) -> Result<serde_json::Value, McpError> {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1).max(1);

        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);

        let params_value = match params {
            Some(p) => Some(serde_json::to_value(&p).map_err(|e| McpError::ProtocolViolation {
                server: self.server_name.clone(),
                message: format!("failed to serialize {method} params: {e}"),
            })?),
            None => None,
        };
        let envelope = JsonRpcRequest {
            jsonrpc: "2.0",
            id,
            method,
            params: params_value,
        };
        let mut bytes = serde_json::to_vec(&envelope).map_err(|e| McpError::ProtocolViolation {
            server: self.server_name.clone(),
            message: format!("failed to serialize {method} envelope: {e}"),
        })?;
        bytes.push(b'\n');

        // If the writer task has already gone (server died), pull the stderr
        // tail and report a crash now rather than wait for a timeout.
        if self.write_tx.send(WriteMsg::Frame(bytes)).await.is_err() {
            self.pending.lock().await.remove(&id);
            return Err(self.crash_error("send to writer failed").await);
        }

        let to = req_timeout.unwrap_or(DEFAULT_REQUEST_TIMEOUT);
        match timeout(to, rx).await {
            Ok(Ok(res)) => res,
            Ok(Err(_)) => {
                // Sender dropped — typically because the reader / waiter
                // declared the server crashed.
                Err(self.crash_error("response channel closed").await)
            }
            Err(_elapsed) => {
                self.pending.lock().await.remove(&id);
                Err(McpError::Timeout {
                    server: self.server_name.clone(),
                    seconds: to.as_secs(),
                })
            }
        }
    }

    /// Send a JSON-RPC notification (no response expected).
    pub(crate) async fn notify<P: Serialize>(
        &self,
        method: &str,
        params: Option<P>,
    ) -> Result<(), McpError> {
        let params_value = match params {
            Some(p) => Some(serde_json::to_value(&p).map_err(|e| McpError::ProtocolViolation {
                server: self.server_name.clone(),
                message: format!("failed to serialize {method} params: {e}"),
            })?),
            None => None,
        };
        let envelope = JsonRpcNotification {
            jsonrpc: "2.0",
            method,
            params: params_value,
        };
        let mut bytes = serde_json::to_vec(&envelope).map_err(|e| McpError::ProtocolViolation {
            server: self.server_name.clone(),
            message: format!("failed to serialize {method} envelope: {e}"),
        })?;
        bytes.push(b'\n');
        self.write_tx
            .send(WriteMsg::Frame(bytes))
            .await
            .map_err(|_| self.crash_error_sync("send to writer failed"))?;
        Ok(())
    }

    /// Cleanly shut down: close stdin, wait briefly for graceful exit, then
    /// kill if still alive. Drains background tasks.
    pub(crate) async fn shutdown(mut self) -> Result<(), McpError> {
        self.shutdown_inner().await
    }

    async fn shutdown_inner(&mut self) -> Result<(), McpError> {
        // Close the writer (drops stdin → server EOFs and exits).
        let _ = self.write_tx.send(WriteMsg::Close).await;

        // Wake any pending requests with Cancelled.
        {
            let mut pending = self.pending.lock().await;
            for (_id, tx) in pending.drain() {
                let _ = tx.send(Err(McpError::Cancelled {
                    server: self.server_name.clone(),
                }));
            }
        }

        // Give the child up to 2s to exit on its own.
        if let Some(mut child) = self.child.lock().await.take() {
            let kill_after = Duration::from_secs(2);
            match timeout(kill_after, child.wait()).await {
                Ok(Ok(_)) => {}
                Ok(Err(e)) => {
                    return Err(McpError::Io {
                        server: self.server_name.clone(),
                        source: e,
                    });
                }
                Err(_) => {
                    let _ = child.kill().await;
                    let _ = child.wait().await;
                }
            }
        }

        // Give helper tasks a moment to finish.
        for t in self.tasks.drain(..) {
            t.abort();
        }
        Ok(())
    }

    async fn crash_error(&self, _hint: &str) -> McpError {
        let stderr = self.stderr_buf.lock().await.snapshot_lossy();
        let exit_status = match self.child.lock().await.as_mut() {
            Some(c) => match c.try_wait() {
                Ok(Some(s)) => s.to_string(),
                Ok(None) => "still running".to_string(),
                Err(_) => "unknown".to_string(),
            },
            None => "exited".to_string(),
        };
        McpError::ServerCrashed {
            server: self.server_name.clone(),
            exit_status,
            stderr,
        }
    }

    fn crash_error_sync(&self, _hint: &str) -> McpError {
        // Non-async variant for `notify` — we don't have access to the lock
        // contents synchronously, so we return a coarser error.
        McpError::ServerCrashed {
            server: self.server_name.clone(),
            exit_status: "writer closed".to_string(),
            stderr: String::new(),
        }
    }
}

impl Drop for Transport {
    fn drop(&mut self) {
        // `kill_on_drop(true)` on the child handles the actual kill; we just
        // need to ensure the helper tasks unwind. They will, because the
        // child's pipes are closed when the child dies.
        for t in self.tasks.drain(..) {
            t.abort();
        }
    }
}

fn spawn_writer(
    server: String,
    mut stdin: ChildStdin,
    mut rx: mpsc::Receiver<WriteMsg>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            match msg {
                WriteMsg::Frame(bytes) => {
                    if let Err(e) = stdin.write_all(&bytes).await {
                        eprintln!(
                            "awidat-mcp: writer to '{server}' lost connection: {e}"
                        );
                        break;
                    }
                    let _ = stdin.flush().await;
                }
                WriteMsg::Close => break,
            }
        }
        // Drop stdin to signal EOF to the server.
        drop(stdin);
    })
}

fn spawn_reader(
    server: String,
    stdout: ChildStdout,
    pending: PendingMap,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        loop {
            line.clear();
            let n = match reader.read_line(&mut line).await {
                Ok(0) => break, // EOF
                Ok(n) => n,
                Err(e) => {
                    eprintln!("awidat-mcp: reader from '{server}' I/O error: {e}");
                    break;
                }
            };
            // Tolerate blank lines (some servers emit them between messages).
            let trimmed = line[..n].trim();
            if trimmed.is_empty() {
                continue;
            }
            match serde_json::from_str::<JsonRpcResponse>(trimmed) {
                Ok(resp) => {
                    if resp.method.is_some() && resp.id.is_none() {
                        // Notification from server — Week 2 ignores all of
                        // them. (Logging notifications are routine.)
                        continue;
                    }
                    let id_num = match resp.id.as_ref().and_then(serde_json::Value::as_u64) {
                        Some(n) => n,
                        None => {
                            // Response with non-numeric id — protocol weirdness;
                            // skip rather than crash the pump.
                            continue;
                        }
                    };
                    if let Some(tx) = pending.lock().await.remove(&id_num) {
                        let res = if let Some(err) = resp.error {
                            Err(McpError::ToolError {
                                server: server.clone(),
                                message: format!("[code {}] {}", err.code, err.message),
                            })
                        } else {
                            Ok(resp.result.unwrap_or(serde_json::Value::Null))
                        };
                        let _ = tx.send(res);
                    }
                }
                Err(e) => {
                    eprintln!(
                        "awidat-mcp: reader from '{server}' got non-JSON-RPC line ({e}): {trimmed}"
                    );
                    // Don't break: a single malformed line shouldn't take
                    // down the channel.
                }
            }
        }
    })
}

fn spawn_stderr(stderr: ChildStderr, buf: Arc<Mutex<RingBuffer>>) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut reader = BufReader::new(stderr);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line).await {
                Ok(0) => break,
                Ok(_) => {
                    let mut b = buf.lock().await;
                    b.push(line.as_bytes());
                }
                Err(_) => break,
            }
        }
    })
}

fn spawn_waiter(
    server: String,
    child: Arc<Mutex<Option<Child>>>,
    pending: PendingMap,
    stderr_buf: Arc<Mutex<RingBuffer>>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        // We can't `wait` while holding the lock without contending with
        // shutdown_inner. So poll: try_wait every 50ms. This is fine for our
        // use case (we only need to detect crashes, not be sub-millisecond
        // responsive about it).
        loop {
            tokio::time::sleep(Duration::from_millis(50)).await;
            let exited;
            let exit_str;
            {
                let mut guard = child.lock().await;
                let Some(c) = guard.as_mut() else { return; };
                match c.try_wait() {
                    Ok(Some(s)) => {
                        exited = true;
                        exit_str = s.to_string();
                    }
                    Ok(None) => {
                        exited = false;
                        exit_str = String::new();
                    }
                    Err(_) => {
                        exited = true;
                        exit_str = "wait error".to_string();
                    }
                }
            }
            if exited {
                let stderr = stderr_buf.lock().await.snapshot_lossy();
                let mut pending = pending.lock().await;
                for (_id, tx) in pending.drain() {
                    let _ = tx.send(Err(McpError::ServerCrashed {
                        server: server.clone(),
                        exit_status: exit_str.clone(),
                        stderr: stderr.clone(),
                    }));
                }
                return;
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_buffer_caps_at_capacity() {
        let mut rb = RingBuffer::new(8);
        rb.push(b"abcd");
        rb.push(b"efgh");
        rb.push(b"ij");
        let s = rb.snapshot_lossy();
        assert_eq!(s.len(), 8);
        assert!(s.ends_with("ij"));
    }

    #[test]
    fn ring_buffer_handles_oversized_single_push() {
        let mut rb = RingBuffer::new(4);
        rb.push(b"abcdefghij");
        let s = rb.snapshot_lossy();
        assert_eq!(s, "ghij");
    }

    #[test]
    fn ring_buffer_appends_under_cap() {
        let mut rb = RingBuffer::new(16);
        rb.push(b"hello ");
        rb.push(b"world");
        assert_eq!(rb.snapshot_lossy(), "hello world");
    }
}
