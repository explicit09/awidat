//! Bridge between the codex app-server and the desktop's
//! [`montage_desktop_protocol::Item`] event stream.
//!
//! # What this crate owns
//!
//! * [`CodexAppServer`] — per-project lifecycle around a Codex
//!   external `codex app-server` process and one `thread_id`.
//! * The mappers (see [`mappers`]) that translate codex
//!   `ServerNotification` / `ServerRequest` / `v2::ThreadItem` payloads
//!   into [`montage_desktop_protocol::Item`] values the React renderer
//!   already understands.
//! * The approval round-trip: when codex asks for approval via a
//!   `ServerRequest`, we stash the typed `RequestId` in `pending` keyed
//!   by `item_id`, emit a `Item::ApprovalRequest`, and resolve the
//!   server request when the desktop calls [`CodexAppServer::respond_approval`].
//!
//! # What this crate does NOT own
//!
//! * Tauri commands / event channels — the desktop integration step
//!   plugs [`ItemEmitter`] into Tauri's `app.emit(...)` and exposes
//!   thin Tauri commands that call into this crate.
//! * Persistence — Codex owns thread storage and resume.
//! * The MCP server binary path — callers pass it in as an
//!   `Option<PathBuf>`. The desktop knows how to find it on macOS
//!   bundle layouts; the bridge does not.

mod external;
pub mod mappers;
mod tool_exposure;
mod wire;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use montage_desktop_protocol::{AgentProfile, Item};
use tokio::sync::Mutex;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tracing::debug;
use tracing::warn;

use crate::mappers::map_notification;
use crate::wire::RequestId;
use crate::wire::ServerNotification;
use crate::wire::ServerRequest;

/// Sink for emitted [`Item`]s plus the explicit turn-end signal. The
/// desktop implementation wraps a `tauri::AppHandle` and forwards each
/// call to `Tauri::emit`. Tests substitute a recording mock.
pub trait ItemEmitter: Send + Sync + 'static {
    /// Emit a single timeline [`Item`].
    fn emit_item(&self, item: Item);
    /// Emit the per-turn terminal signal. `error == None` means clean
    /// completion; `Some(msg)` is a turn-fatal failure.
    fn emit_turn_end(&self, turn_id: String, error: Option<String>);
    /// Signal that the project's on-disk OTIO has just been mutated by
    /// a tool call. The desktop listens on `montage://timeline-changed`
    /// and refetches `project.otio.json`. The MCP server lives in a
    /// subprocess that can't talk to Tauri's event bus, so the bridge
    /// raises this event whenever a mutating montage-MCP tool completes
    /// (apply_edl, manage_assets, import_media, vedit_*, podcast_*, …).
    /// Default impl is a no-op so non-Tauri emitters (tests, future
    /// surfaces) don't have to care.
    fn emit_timeline_changed(&self) {}
}

/// Reason an approval is pending — drives which response struct we
/// build when the user replies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingKind {
    /// `item/commandExecution/requestApproval`.
    ExecApproval,
    /// `item/fileChange/requestApproval`.
    FileChangeApproval,
    /// `item/permissions/requestApproval`.
    PermissionApproval,
    /// `item/tool/requestUserInput`.
    UserInput,
}

/// One in-flight server request waiting for a desktop reply.
#[derive(Debug, Clone)]
pub struct PendingServerRequest {
    /// JSONRPC id we resolve against.
    pub jsonrpc_request_id: RequestId,
    /// What kind of approval this is.
    pub kind: PendingKind,
    /// Owning thread id (debug only).
    pub thread_id: String,
    /// Owning turn id (debug only).
    pub turn_id: String,
    /// For [`PendingKind::UserInput`]: the question id we route the
    /// user's reply through.
    pub user_input_question_id: Option<String>,
    /// For [`PendingKind::PermissionApproval`]: the permissions the
    /// agent asked for. We grant exactly these on Allow/Session;
    /// Deny drops them so the response carries an empty profile.
    pub requested_permissions: Option<serde_json::Value>,
}

/// User-facing decision for an approval prompt.
#[derive(Debug, Clone, Copy)]
pub enum ApprovalDecision {
    /// Approve once.
    Allow,
    /// Approve and remember (codex `acceptForSession`).
    AllowForSession,
    /// Decline. The turn continues.
    Deny,
}

/// Error surface for the public bridge API.
#[derive(Debug, thiserror::Error)]
pub enum BridgeError {
    /// Failed to construct or start the codex app-server backend.
    #[error("codex startup failed: {0}")]
    Startup(String),
    /// A `ClientRequest` round-trip failed.
    #[error("codex request failed: {0}")]
    Request(String),
    /// No pending approval/input for the given id (duplicate / stale).
    #[error("no pending approval/input for id {call_id}")]
    UnknownApproval { call_id: String },
    /// `resolve_server_request` returned an error.
    #[error("resolve failed: {0}")]
    Resolve(String),
}

/// Top-level handle to one project's codex session. Rebuild when the
/// desktop switches projects.
pub struct CodexAppServer {
    /// External process client for requests and approval resolution.
    client: external::ExternalAppServerClient,
    /// Stable codex thread id (one per project lifecycle).
    thread_id: String,
    /// Pre-rendered L1 skills catalog. Prepended to every turn so the
    /// agent can route dynamically while full skill bodies stay behind
    /// `load_skill`.
    skills_catalog: Option<String>,
    /// Pre-rendered L1 recent-feedback fragment (Wave 5 C3). Lists the
    /// user's most recent proposal rejections so the agent doesn't repeat
    /// patterns the user just turned down. Prepended to each turn because it
    /// is small behavioural context. `None` when the rejection log is missing
    /// or empty.
    recent_feedback: Option<String>,
    /// Pending server-requests waiting for a desktop reply, keyed by
    /// the codex `item_id` we emitted to the renderer.
    pending: Arc<Mutex<HashMap<String, PendingServerRequest>>>,
    /// Drop the bridge → drop this → pump exits.
    shutdown_tx: Option<oneshot::Sender<()>>,
    /// Joined by [`Self::shutdown`].
    pump_task: Option<JoinHandle<()>>,
}

impl CodexAppServer {
    /// Spawn the external app-server for `project_root` and start its
    /// event pump.
    ///
    /// When `resume_thread_id` is `Some`, the bridge issues a
    /// `thread/resume` instead of `thread/start`, so the existing
    /// rollout's history is re-loaded and the same `thread_id` is
    /// retained. Otherwise a fresh thread is created.
    pub async fn launch(
        emit: Arc<dyn ItemEmitter>,
        project_root: PathBuf,
        mcp_server_path: Option<PathBuf>,
        developer_instructions: Option<String>,
        skills_catalog: Option<String>,
        recent_feedback: Option<String>,
        resume_thread_id: Option<String>,
    ) -> Result<Self, BridgeError> {
        if mcp_server_path.is_none() {
            warn!(
                "montage-mcp-server path not provided; external codex will run without Montage tools"
            );
        }
        let (client, mut event_rx) =
            external::ExternalAppServerClient::start(&project_root, mcp_server_path).await?;
        let initialize_response: serde_json::Value = client
            .request(wire::initialize_request(
                client.next_request_id(),
                env!("CARGO_PKG_VERSION"),
            ))
            .await
            .map_err(|e| BridgeError::Startup(format!("initialize: {e}")))?;
        let _ = initialize_response;
        client
            .send_notification("initialized", None)
            .await
            .map_err(|e| BridgeError::Startup(format!("initialized: {e}")))?;

        let thread_id = if let Some(resume_id) = resume_thread_id {
            let resume_response: wire::ThreadResponse = client
                .request(wire::thread_resume_request(
                    client.next_request_id(),
                    resume_id.clone(),
                    &project_root,
                    developer_instructions,
                ))
                .await
                .map_err(|e| BridgeError::Startup(format!("thread/resume: {e}")))?;
            resume_response.thread.id
        } else {
            let thread_response: wire::ThreadResponse = client
                .request(wire::thread_start_request(
                    client.next_request_id(),
                    &project_root,
                    developer_instructions,
                ))
                .await
                .map_err(|e| BridgeError::Startup(format!("thread/start: {e}")))?;
            thread_response.thread.id
        };

        let pending: Arc<Mutex<HashMap<String, PendingServerRequest>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let pump_pending = Arc::clone(&pending);
        let pump_emit = Arc::clone(&emit);
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();
        let pump_task = tokio::spawn(async move {
            run_external_event_pump(&mut event_rx, pump_pending, pump_emit, &mut shutdown_rx).await;
        });

        Ok(Self {
            client,
            thread_id,
            skills_catalog,
            recent_feedback,
            pending,
            shutdown_tx: Some(shutdown_tx),
            pump_task: Some(pump_task),
        })
    }

    /// Start a new turn with `input` as the user message. Returns the
    /// codex-assigned `turn_id`. `profile` selects the per-turn model and
    /// reasoning effort, and remains sticky for subsequent turns.
    pub async fn start_turn(
        &self,
        input: String,
        profile: AgentProfile,
    ) -> Result<String, BridgeError> {
        let text = compose_turn_input(
            self.recent_feedback.as_deref(),
            self.skills_catalog.as_deref(),
            &input,
        );
        let response: wire::TurnStartResponse = self
            .client
            .request(wire::turn_start_request(
                next_request_id(),
                &self.thread_id,
                text,
                profile,
            ))
            .await?;
        Ok(response.turn.id)
    }

    /// Interrupt the given turn. Best-effort; an already-finished turn
    /// yields an error from codex which we swallow because the
    /// user-facing action ("cancel") is idempotent.
    pub async fn interrupt(&self, turn_id: &str) -> Result<(), BridgeError> {
        let result: Result<serde_json::Value, BridgeError> = self
            .client
            .request(wire::turn_interrupt_request(
                next_request_id(),
                &self.thread_id,
                turn_id,
            ))
            .await;
        if let Err(e) = result {
            debug!(
                error = %e,
                turn_id,
                "turn/interrupt returned error (likely already finished)"
            );
        }
        clear_pending_for_turn(&self.pending, turn_id).await;
        Ok(())
    }

    /// Reply to an outstanding approval. `call_id` is the codex
    /// `item_id` surfaced as [`Item::ApprovalRequest`]'s `id`.
    pub async fn respond_approval(
        &self,
        call_id: &str,
        decision: ApprovalDecision,
    ) -> Result<(), BridgeError> {
        let pending = {
            let mut guard = self.pending.lock().await;
            guard
                .remove(call_id)
                .ok_or_else(|| BridgeError::UnknownApproval {
                    call_id: call_id.to_string(),
                })?
        };
        let result_value = match pending.kind {
            PendingKind::ExecApproval => {
                let decision = match decision {
                    ApprovalDecision::Allow => "accept",
                    ApprovalDecision::AllowForSession => "acceptForSession",
                    ApprovalDecision::Deny => "decline",
                };
                serde_json::json!({ "decision": decision })
            }
            PendingKind::FileChangeApproval => {
                let decision = match decision {
                    ApprovalDecision::Allow => "accept",
                    ApprovalDecision::AllowForSession => "acceptForSession",
                    ApprovalDecision::Deny => "decline",
                };
                serde_json::json!({ "decision": decision })
            }
            PendingKind::PermissionApproval => {
                let scope = match decision {
                    ApprovalDecision::Allow => "turn",
                    ApprovalDecision::AllowForSession => "session",
                    ApprovalDecision::Deny => "turn",
                };
                let permissions = match decision {
                    ApprovalDecision::Allow | ApprovalDecision::AllowForSession => pending
                        .requested_permissions
                        .clone()
                        .unwrap_or_else(|| serde_json::json!({})),
                    ApprovalDecision::Deny => serde_json::json!({}),
                };
                serde_json::json!({
                    "permissions": permissions,
                    "scope": scope,
                    "strictAutoReview": null,
                })
            }
            PendingKind::UserInput => {
                return Err(BridgeError::UnknownApproval {
                    call_id: call_id.to_string(),
                });
            }
        };
        self.send_resolve(pending.jsonrpc_request_id, result_value)
            .await
    }

    /// Reply to an outstanding `request_user_input`. v1 only handles the
    /// first question; extras log a warning when the request arrives.
    pub async fn respond_user_input(
        &self,
        call_id: &str,
        reply: String,
    ) -> Result<(), BridgeError> {
        let pending = {
            let mut guard = self.pending.lock().await;
            guard
                .remove(call_id)
                .ok_or_else(|| BridgeError::UnknownApproval {
                    call_id: call_id.to_string(),
                })?
        };
        if pending.kind != PendingKind::UserInput {
            return Err(BridgeError::UnknownApproval {
                call_id: call_id.to_string(),
            });
        }
        let question_id = pending
            .user_input_question_id
            .clone()
            .unwrap_or_else(|| "answer".to_string());
        let result_value = serde_json::json!({
            "answers": {
                question_id: {
                    "answers": [reply],
                }
            }
        });
        self.send_resolve(pending.jsonrpc_request_id, result_value)
            .await
    }

    /// Drain the event pump and drop the request handle. Bounded.
    pub async fn shutdown(mut self) -> Result<(), BridgeError> {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        if let Some(handle) = self.pump_task.take()
            && let Err(e) = handle.await
        {
            warn!(error = ?e, "codex-bridge pump task join error");
        }
        self.client.shutdown().await;
        Ok(())
    }

    /// Currently-active thread id. Stable across the bridge's lifetime.
    pub fn thread_id(&self) -> &str {
        &self.thread_id
    }

    /// Resolve a pending approval or input request through the external client.
    async fn send_resolve(
        &self,
        request_id: RequestId,
        result: serde_json::Value,
    ) -> Result<(), BridgeError> {
        self.client.resolve(request_id, result).await
    }
}

impl Drop for CodexAppServer {
    fn drop(&mut self) {
        // Best-effort: if dropped without async shutdown, nudge pump.
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
    }
}

async fn run_external_event_pump(
    event_rx: &mut mpsc::Receiver<external::ExternalServerEvent>,
    pending: Arc<Mutex<HashMap<String, PendingServerRequest>>>,
    emit: Arc<dyn ItemEmitter>,
    shutdown_rx: &mut oneshot::Receiver<()>,
) {
    let mut text_buffers: HashMap<String, String> = HashMap::new();
    loop {
        tokio::select! {
            _ = &mut *shutdown_rx => {
                debug!("codex-bridge external pump received shutdown signal");
                break;
            }
            event = event_rx.recv() => {
                let Some(event) = event else {
                    debug!("codex-bridge external pump: event stream closed");
                    break;
                };
                match event {
                    external::ExternalServerEvent::Notification(notification) => {
                        handle_notification(notification, &pending, &emit, &mut text_buffers).await;
                    }
                    external::ExternalServerEvent::Request(request) => {
                        handle_server_request(request, &pending, &emit).await;
                    }
                }
            }
        }
    }
}

async fn handle_notification(
    notification: ServerNotification,
    pending: &Arc<Mutex<HashMap<String, PendingServerRequest>>>,
    emit: &Arc<dyn ItemEmitter>,
    text_buffers: &mut HashMap<String, String>,
) {
    let turn_end = turn_end_from_notification(&notification);
    let nudge_timeline = mappers::is_project_mutating_completion(&notification);
    for item in map_notification(&notification, text_buffers) {
        emit.emit_item(item);
    }
    // Nudge the desktop to refetch project.otio.json after any mutating
    // montage-MCP tool completes. The MCP server lives in a subprocess and
    // can't talk to Tauri's event bus itself, so the bridge is the only hop
    // that knows when OTIO has just been rewritten under the React view.
    if nudge_timeline {
        emit.emit_timeline_changed();
    }
    if let Some(turn_end) = turn_end {
        clear_pending_for_turn(pending, &turn_end.turn_id).await;
        emit.emit_turn_end(turn_end.turn_id, turn_end.error);
    }
}

/// If this notification marks the end of a turn, return the per-turn
/// error result (`None` for a clean finish; `Some(msg)` for failed).
/// The outer `Option` distinguishes "not a turn-end" from "is a
/// turn-end".
#[derive(Debug, Clone, PartialEq, Eq)]
struct TurnEnd {
    turn_id: String,
    error: Option<String>,
}

fn turn_end_from_notification(notification: &ServerNotification) -> Option<TurnEnd> {
    match notification.method.as_str() {
        "turn/completed" => Some(TurnEnd {
            turn_id: notification.params["turn"]["id"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            error: notification.params["turn"]["error"]["message"]
                .as_str()
                .map(ToString::to_string),
        }),
        "error" => Some(TurnEnd {
            turn_id: notification.params["turnId"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            error: Some(
                notification.params["error"]["message"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            ),
        }),
        _ => None,
    }
}

async fn clear_pending_for_turn(
    pending: &Arc<Mutex<HashMap<String, PendingServerRequest>>>,
    turn_id: &str,
) {
    pending
        .lock()
        .await
        .retain(|_, request| request.turn_id != turn_id);
}

/// Insert one server-request into `pending` and surface the right
/// approval/awaiting-input [`Item`].
async fn handle_server_request(
    request: ServerRequest,
    pending: &Arc<Mutex<HashMap<String, PendingServerRequest>>>,
    emit: &Arc<dyn ItemEmitter>,
) {
    match request.method.as_str() {
        "item/commandExecution/requestApproval" => {
            let item_id = string_param(&request.params, "itemId");
            let summary = request.params["command"]
                .as_str()
                .unwrap_or("(unspecified command)")
                .to_string();
            let metadata = mappers::build_capability_metadata_for_exec(&summary);
            pending.lock().await.insert(
                item_id.clone(),
                PendingServerRequest {
                    jsonrpc_request_id: request.id,
                    kind: PendingKind::ExecApproval,
                    thread_id: string_param(&request.params, "threadId"),
                    turn_id: string_param(&request.params, "turnId"),
                    user_input_question_id: None,
                    requested_permissions: None,
                },
            );
            emit.emit_item(Item::ApprovalRequest {
                id: montage_desktop_protocol::Id::new(item_id),
                phase: montage_desktop_protocol::ItemLifecycle::Started,
                tool_name: "bash".into(),
                args_summary: summary,
                capability_metadata: metadata,
                // Exec approvals don't carry a structured `reasoning`
                // arg today; the agent's rationale (if any) is captured
                // on the matching MCP tool-call path.
                rationale: None,
            });
        }
        "item/fileChange/requestApproval" => {
            let item_id = string_param(&request.params, "itemId");
            let reason = request.params["reason"].as_str();
            let metadata = mappers::build_capability_metadata_for_file_change(reason);
            let summary = reason.unwrap_or("apply file changes").to_string();
            pending.lock().await.insert(
                item_id.clone(),
                PendingServerRequest {
                    jsonrpc_request_id: request.id,
                    kind: PendingKind::FileChangeApproval,
                    thread_id: string_param(&request.params, "threadId"),
                    turn_id: string_param(&request.params, "turnId"),
                    user_input_question_id: None,
                    requested_permissions: None,
                },
            );
            emit.emit_item(Item::ApprovalRequest {
                id: montage_desktop_protocol::Id::new(item_id),
                phase: montage_desktop_protocol::ItemLifecycle::Started,
                tool_name: "apply_patch".into(),
                args_summary: summary,
                capability_metadata: metadata,
                // FileChange approvals carry a `reason` (already folded
                // into `args_summary`) but no structured `reasoning`
                // arg — agent rationale rides the MCP path instead.
                rationale: None,
            });
        }
        "item/permissions/requestApproval" => {
            let item_id = string_param(&request.params, "itemId");
            let reason = request.params["reason"].as_str();
            let summary = reason
                .unwrap_or("request additional permissions")
                .to_string();
            let metadata = mappers::build_capability_metadata_for_file_change(reason);
            pending.lock().await.insert(
                item_id.clone(),
                PendingServerRequest {
                    jsonrpc_request_id: request.id,
                    kind: PendingKind::PermissionApproval,
                    thread_id: string_param(&request.params, "threadId"),
                    turn_id: string_param(&request.params, "turnId"),
                    user_input_question_id: None,
                    requested_permissions: Some(request.params["permissions"].clone()),
                },
            );
            emit.emit_item(Item::ApprovalRequest {
                id: montage_desktop_protocol::Id::new(item_id),
                phase: montage_desktop_protocol::ItemLifecycle::Started,
                tool_name: "permissions".into(),
                args_summary: summary,
                capability_metadata: metadata,
                // Permissions approvals have no agent-supplied
                // `reasoning` arg; the user-facing `reason` is folded
                // into `args_summary`.
                rationale: None,
            });
        }
        "item/tool/requestUserInput" => {
            let item_id = string_param(&request.params, "itemId");
            let questions = request.params["questions"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            let first = questions.first();
            if questions.len() > 1 {
                warn!(
                    item_id = %item_id,
                    extra = questions.len() - 1,
                    "request_user_input with >1 questions; only the first is rendered in v1"
                );
            }
            let question_text = first
                .map(|q| string_param(q, "question"))
                .unwrap_or_default();
            let question_id = first.map(|q| string_param(q, "id"));
            let options = first.and_then(|q| {
                q["options"].as_array().map(|opts| {
                    opts.iter()
                        .map(|option| string_param(option, "label"))
                        .collect()
                })
            });
            pending.lock().await.insert(
                item_id.clone(),
                PendingServerRequest {
                    jsonrpc_request_id: request.id,
                    kind: PendingKind::UserInput,
                    thread_id: string_param(&request.params, "threadId"),
                    turn_id: string_param(&request.params, "turnId"),
                    user_input_question_id: question_id,
                    requested_permissions: None,
                },
            );
            emit.emit_item(Item::AwaitingUserInput {
                id: montage_desktop_protocol::Id::new(item_id),
                phase: montage_desktop_protocol::ItemLifecycle::Started,
                question: question_text,
                options,
            });
        }
        _ => {
            debug!(id = ?request.id, method = %request.method, "codex-bridge: unhandled ServerRequest variant");
        }
    }
}

fn string_param(value: &serde_json::Value, key: &str) -> String {
    value[key].as_str().unwrap_or_default().to_string()
}

/// Monotonic client-side request id counter. App-server only needs
/// uniqueness within one connection; this counter resets per process.
fn next_request_id() -> i64 {
    use std::sync::atomic::{AtomicI64, Ordering};
    static COUNTER: AtomicI64 = AtomicI64::new(100);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}

/// Compose the per-turn text the agent sees: optional recent feedback,
/// optional compact skills catalog, then the user input.
///
/// Recent feedback is behavioural ("don't repeat these rejections"), and
/// earlier prompt text is treated as higher priority by the model, so it
/// leads. When absent, we send the user's input as-is.
///
/// Extracted from `CodexAppServer::start_turn` so the unit test can pin
/// the exact composition without launching the external app-server.
fn compose_turn_input(
    recent_feedback: Option<&str>,
    skills_catalog: Option<&str>,
    input: &str,
) -> String {
    let mut parts: Vec<&str> = Vec::with_capacity(3);
    if let Some(fb) = recent_feedback {
        parts.push(fb);
    }
    if let Some(skills) = skills_catalog {
        parts.push(skills);
    }
    parts.push(input);
    parts.join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn turn_end_from_notification_includes_turn_id() {
        let notification = ServerNotification {
            method: "turn/completed".to_string(),
            params: json!({
                "threadId": "thread-1",
                "turn": {
                    "id": "turn-1",
                    "error": null
                }
            }),
        };

        assert_eq!(
            turn_end_from_notification(&notification),
            Some(TurnEnd {
                turn_id: "turn-1".to_string(),
                error: None,
            })
        );
    }

    #[test]
    fn turn_end_from_error_includes_turn_id() {
        let notification = ServerNotification {
            method: "error".to_string(),
            params: json!({
                "threadId": "thread-1",
                "turnId": "turn-err",
                "willRetry": false,
                "error": {
                    "message": "failed"
                }
            }),
        };

        assert_eq!(
            turn_end_from_notification(&notification),
            Some(TurnEnd {
                turn_id: "turn-err".to_string(),
                error: Some("failed".to_string()),
            })
        );
    }

    #[tokio::test]
    async fn clear_pending_for_turn_removes_only_matching_turn() {
        let pending = Arc::new(Mutex::new(HashMap::from([
            (
                "call-1".to_string(),
                PendingServerRequest {
                    jsonrpc_request_id: json!(1),
                    kind: PendingKind::ExecApproval,
                    thread_id: "thread-1".to_string(),
                    turn_id: "turn-1".to_string(),
                    user_input_question_id: None,
                    requested_permissions: None,
                },
            ),
            (
                "call-2".to_string(),
                PendingServerRequest {
                    jsonrpc_request_id: json!(2),
                    kind: PendingKind::UserInput,
                    thread_id: "thread-1".to_string(),
                    turn_id: "turn-2".to_string(),
                    user_input_question_id: Some("question-1".to_string()),
                    requested_permissions: None,
                },
            ),
        ])));

        clear_pending_for_turn(&pending, "turn-1").await;

        let pending = pending.lock().await;
        assert!(!pending.contains_key("call-1"));
        assert!(pending.contains_key("call-2"));
    }
}

#[cfg(test)]
mod compose_tests {
    use super::compose_turn_input;

    #[test]
    fn turn_input_includes_skills_catalog_after_feedback() {
        let out = compose_turn_input(Some("<FB>"), Some("<SK>"), "hello");
        assert_eq!(out, "<FB>\n\n<SK>\n\nhello");
    }

    #[test]
    fn simple_chat_turn_stays_plain_user_input() {
        let out = compose_turn_input(None, None, "how are you");
        assert_eq!(out, "how are you");
        assert!(
            !out.contains("AGENTS.md")
                && !out.contains("<skills_catalog>")
                && !out.contains("Project conventions"),
            "plain chat turns must not replay session context in the user input: {out}"
        );
    }

    #[test]
    fn no_fragments_returns_input_only() {
        let out = compose_turn_input(None, None, "hello");
        assert_eq!(out, "hello");
    }

    #[test]
    fn feedback_only_prefixes_input() {
        let out = compose_turn_input(Some("<FB>"), None, "hello");
        assert_eq!(out, "<FB>\n\nhello");
    }
}
