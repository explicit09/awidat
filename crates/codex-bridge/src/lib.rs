//! In-process bridge between the codex app-server and the desktop's
//! [`montage_desktop_protocol::Item`] event stream.
//!
//! # What this crate owns
//!
//! * [`CodexAppServer`] — per-project lifecycle around a single
//!   [`codex_app_server_client::InProcessAppServerClient`] and one
//!   `thread_id`. Spawns an internal event-pump task that owns the
//!   `&mut self`-requiring `next_event` loop.
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
//! * Persistence — `state_db` is left as `None` for v1.
//! * The MCP server binary path — callers pass it in as an
//!   `Option<PathBuf>`. The desktop knows how to find it on macOS
//!   bundle layouts; the bridge does not.

pub mod mappers;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use codex_app_server_client::AppServerRequestHandle;
use codex_app_server_client::EnvironmentManager;
use codex_app_server_client::ExecServerRuntimePaths;
use codex_app_server_client::InProcessAppServerClient;
use codex_app_server_client::InProcessClientStartArgs;
use codex_app_server_client::InProcessServerEvent;
use codex_app_server_protocol::ClientRequest;
use codex_app_server_protocol::CommandExecutionApprovalDecision;
use codex_app_server_protocol::CommandExecutionRequestApprovalResponse;
use codex_app_server_protocol::FileChangeApprovalDecision;
use codex_app_server_protocol::FileChangeRequestApprovalResponse;
use codex_app_server_protocol::GrantedPermissionProfile;
use codex_app_server_protocol::JSONRPCErrorError;
use codex_app_server_protocol::PermissionGrantScope;
use codex_app_server_protocol::PermissionsRequestApprovalResponse;
use codex_app_server_protocol::RequestId;
use codex_app_server_protocol::RequestPermissionProfile;
use codex_app_server_protocol::ServerNotification;
use codex_app_server_protocol::ServerRequest;
use codex_app_server_protocol::ThreadResumeParams;
use codex_app_server_protocol::ThreadResumeResponse;
use codex_app_server_protocol::ThreadStartParams;
use codex_app_server_protocol::ThreadStartResponse;
use codex_app_server_protocol::ToolRequestUserInputAnswer;
use codex_app_server_protocol::ToolRequestUserInputResponse;
use codex_app_server_protocol::TurnInterruptParams;
use codex_app_server_protocol::TurnInterruptResponse;
use codex_app_server_protocol::TurnStartParams;
use codex_app_server_protocol::TurnStartResponse;
use codex_app_server_protocol::UserInput;
use codex_arg0::Arg0DispatchPaths;
use codex_config::CloudRequirementsLoader;
use codex_config::LoaderOverrides;
use codex_core::config::ConfigBuilder;
use codex_feedback::CodexFeedback;
use codex_protocol::protocol::SessionSource;
use montage_desktop_protocol::Item;
use tokio::sync::Mutex;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tracing::debug;
use tracing::warn;

use crate::mappers::map_notification;

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
    pub requested_permissions: Option<RequestPermissionProfile>,
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
    /// Failed to construct the codex Config or start the in-process server.
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

/// One unit of work for the resolve channel: either a successful
/// resolve or a rejection. The pump-side worker owns the in-process
/// client and calls the matching method.
///
/// `Reject` is unused by v1 of the public surface (the desktop only
/// surfaces Allow/Deny which both round-trip through `Resolve`) but
/// the pump wires it up so a future integration step that adds a
/// "force-cancel approval" affordance has the plumbing ready.
#[allow(dead_code)]
enum ResolveCommand {
    Resolve {
        request_id: RequestId,
        result: serde_json::Value,
        ack: oneshot::Sender<Result<(), String>>,
    },
    Reject {
        request_id: RequestId,
        error: JSONRPCErrorError,
        ack: oneshot::Sender<Result<(), String>>,
    },
}

/// Top-level handle to one project's codex session. Rebuild when the
/// desktop switches projects.
pub struct CodexAppServer {
    /// Cloneable handle for typed requests (`turn/start`, `turn/interrupt`).
    request_handle: AppServerRequestHandle,
    /// Stable codex thread id (one per project lifecycle).
    thread_id: String,
    /// Pre-rendered L1 recent-feedback fragment (Wave 5 C3). Lists the
    /// user's most recent proposal rejections so the agent doesn't repeat
    /// patterns the user just turned down. Prepended to each turn because it
    /// is small behavioural context. `None` when the rejection log is missing
    /// or empty.
    recent_feedback: Option<String>,
    /// Pending server-requests waiting for a desktop reply, keyed by
    /// the codex `item_id` we emitted to the renderer.
    pending: Arc<Mutex<HashMap<String, PendingServerRequest>>>,
    /// Channel used by `respond_*` to ask the pump task to call
    /// `resolve_server_request` (which needs `&InProcessAppServerClient`,
    /// not `&AppServerRequestHandle`).
    resolve_tx: mpsc::Sender<ResolveCommand>,
    /// Drop the bridge → drop this → pump exits.
    shutdown_tx: Option<oneshot::Sender<()>>,
    /// Joined by [`Self::shutdown`].
    pump_task: Option<JoinHandle<()>>,
}

impl CodexAppServer {
    /// Spawn the in-process app-server for `project_root` and start its
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
        // 1. Build the CLI overrides FIRST so they're baked into the
        //    Config we hand off to the in-process app-server. The
        //    server's ConfigManager only re-applies overrides on later
        //    config reloads — the first turn uses the Config we pass
        //    directly, so the mcp_servers.montage entries MUST be in
        //    there already. Codex `.env_clear()`s on MCP spawn (see
        //    vendor/codex-rs/rmcp-client/src/stdio_server_launcher.rs:259),
        //    so MONTAGE_PROJECT_ROOT has to ride the per-server env
        //    override. DO NOT regress af731e69 / 2889dc59.
        let mut cli_overrides: Vec<(String, toml::Value)> = Vec::new();

        // Auto-compaction. Codex's compact machinery
        // (vendor/codex-rs/core/src/compact.rs) only fires when this
        // config is set — default is None, meaning the agent runs
        // until it hits the model's hard context limit and stalls.
        // We hit exactly that in a session today: 24 polls + 12 video
        // jobs ate the full 258k window before the edit finished.
        // 200_000 trips compaction at ~78% utilization, leaving the
        // post-compaction turn room to actually do something.
        cli_overrides.push((
            "model_auto_compact_token_limit".to_string(),
            toml::Value::Integer(200_000),
        ));

        if let Some(mcp_path) = mcp_server_path {
            cli_overrides.push((
                "mcp_servers.montage.command".to_string(),
                toml::Value::String(mcp_path.display().to_string()),
            ));
            cli_overrides.push((
                "mcp_servers.montage.env.MONTAGE_PROJECT_ROOT".to_string(),
                toml::Value::String(project_root.display().to_string()),
            ));
        } else {
            warn!("montage-mcp-server path not provided; codex will run without Montage tools");
        }

        // 2. Build the app-server Config WITH the overrides applied,
        //    anchored at project_root.
        let config = ConfigBuilder::default()
            .fallback_cwd(Some(project_root.clone()))
            .cli_overrides(cli_overrides.clone())
            .build()
            .await
            .map_err(|e| BridgeError::Startup(format!("ConfigBuilder::build: {e}")))?;

        // 3. Mirror vendor/codex-rs/exec/src/lib.rs:530-555. arg0 +
        //    EnvironmentManager + ExecServerRuntimePaths drive the
        //    sandbox / re-exec dance.
        //
        // Default's codex_self_exe is None — that's fine when codex
        // owns the process (`arg0_dispatch_or_else` populates it from
        // argv[0]), but the bridge runs in-process inside the Tauri
        // app where argv[0] is the host binary. Stamp it from
        // `current_exe()` so ExecServerRuntimePaths::from_optional_paths
        // doesn't reject the startup. (Risk 6.1 from the planning doc.)
        let mut arg0_paths = Arg0DispatchPaths::default();
        if arg0_paths.codex_self_exe.is_none() {
            arg0_paths.codex_self_exe = std::env::current_exe().ok();
        }
        let local_runtime_paths = ExecServerRuntimePaths::from_optional_paths(
            arg0_paths.codex_self_exe.clone(),
            arg0_paths.codex_linux_sandbox_exe.clone(),
        )
        .map_err(|e| BridgeError::Startup(format!("ExecServerRuntimePaths: {e}")))?;
        let environment_manager = EnvironmentManager::from_codex_home(
            config.codex_home.clone(),
            Some(local_runtime_paths),
        )
        .await
        .map_err(|e| BridgeError::Startup(format!("EnvironmentManager: {e}")))?;

        // 4. Build the InProcessClientStartArgs. Tracks
        //    vendor/codex-rs/exec/src/lib.rs:536-555.
        let start_args = InProcessClientStartArgs {
            arg0_paths,
            config: Arc::new(config),
            cli_overrides,
            loader_overrides: LoaderOverrides::default(),
            strict_config: false,
            cloud_requirements: CloudRequirementsLoader::default(),
            feedback: CodexFeedback::new(),
            log_db: None,
            // v1: skip persistence; the desktop integration step can
            // wire state_db if resume lands here.
            state_db: None,
            environment_manager: Arc::new(environment_manager),
            config_warnings: Vec::new(),
            session_source: SessionSource::Exec,
            enable_codex_api_key_env: true,
            client_name: "montage-desktop".to_string(),
            client_version: env!("CARGO_PKG_VERSION").to_string(),
            experimental_api: true,
            opt_out_notification_methods: Vec::new(),
            channel_capacity: codex_app_server_client::DEFAULT_IN_PROCESS_CHANNEL_CAPACITY,
        };

        // 5. Start the in-process server.
        let mut client = InProcessAppServerClient::start(start_args)
            .await
            .map_err(|e| BridgeError::Startup(format!("InProcessAppServerClient::start: {e}")))?;
        let request_handle = AppServerRequestHandle::InProcess(client.request_handle());

        // 6. Start (or resume) the thread. The Montage per-format addendum
        //    (system_prompt.rs::PODCAST_ADDENDUM and friends) and skills
        //    catalog ride on `developer_instructions` so editorial context is
        //    not resent or persisted as user text on every turn.
        //
        //    Resume path: when the desktop asks to continue an existing
        //    chat we call `thread/resume` against the stored thread_id.
        //    Codex's `ThreadResumeParams` loads the rollout from disk
        //    (vendor/codex-rs/app-server-protocol/src/protocol/v2/thread.rs)
        //    and the resulting thread keeps the same id, so subsequent
        //    `turn/start` calls thread off the persisted history.
        let developer_instructions =
            compose_session_instructions(developer_instructions, skills_catalog);
        let thread_id = if let Some(resume_id) = resume_thread_id {
            let resume_response: ThreadResumeResponse = request_handle
                .request_typed(ClientRequest::ThreadResume {
                    request_id: RequestId::Integer(1),
                    params: ThreadResumeParams {
                        thread_id: resume_id.clone(),
                        cwd: Some(project_root.display().to_string()),
                        developer_instructions,
                        ..ThreadResumeParams::default()
                    },
                })
                .await
                .map_err(|e| BridgeError::Startup(format!("thread/resume: {e}")))?;
            resume_response.thread.id
        } else {
            let thread_response: ThreadStartResponse = request_handle
                .request_typed(ClientRequest::ThreadStart {
                    request_id: RequestId::Integer(1),
                    params: ThreadStartParams {
                        cwd: Some(project_root.display().to_string()),
                        developer_instructions,
                        ..ThreadStartParams::default()
                    },
                })
                .await
                .map_err(|e| BridgeError::Startup(format!("thread/start: {e}")))?;
            thread_response.thread.id
        };

        // 7. Set up the pump's communication channels.
        let pending: Arc<Mutex<HashMap<String, PendingServerRequest>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let pump_pending = Arc::clone(&pending);
        let pump_emit = Arc::clone(&emit);
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();
        let (resolve_tx, mut resolve_rx) = mpsc::channel::<ResolveCommand>(32);
        let pump_task = tokio::spawn(async move {
            run_event_pump(
                &mut client,
                pump_pending,
                pump_emit,
                &mut shutdown_rx,
                &mut resolve_rx,
            )
            .await;
        });

        Ok(Self {
            request_handle,
            thread_id,
            recent_feedback,
            pending,
            resolve_tx,
            shutdown_tx: Some(shutdown_tx),
            pump_task: Some(pump_task),
        })
    }

    /// Start a new turn with `input` as the user message. Returns the
    /// codex-assigned `turn_id`. `model` overrides the per-turn model.
    pub async fn start_turn(
        &self,
        input: String,
        model: Option<String>,
    ) -> Result<String, BridgeError> {
        let text = compose_turn_input(self.recent_feedback.as_deref(), &input);
        let response: TurnStartResponse = self
            .request_handle
            .request_typed(ClientRequest::TurnStart {
                request_id: RequestId::Integer(next_request_id()),
                params: TurnStartParams {
                    thread_id: self.thread_id.clone(),
                    input: vec![UserInput::Text {
                        text,
                        text_elements: Vec::new(),
                    }],
                    model,
                    ..TurnStartParams::default()
                },
            })
            .await
            .map_err(|e| BridgeError::Request(format!("turn/start: {e}")))?;
        Ok(response.turn.id)
    }

    /// Interrupt the given turn. Best-effort; an already-finished turn
    /// yields an error from codex which we swallow because the
    /// user-facing action ("cancel") is idempotent.
    pub async fn interrupt(&self, turn_id: &str) -> Result<(), BridgeError> {
        let result: Result<TurnInterruptResponse, _> = self
            .request_handle
            .request_typed(ClientRequest::TurnInterrupt {
                request_id: RequestId::Integer(next_request_id()),
                params: TurnInterruptParams {
                    thread_id: self.thread_id.clone(),
                    turn_id: turn_id.to_string(),
                },
            })
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
                    ApprovalDecision::Allow => CommandExecutionApprovalDecision::Accept,
                    ApprovalDecision::AllowForSession => {
                        CommandExecutionApprovalDecision::AcceptForSession
                    }
                    ApprovalDecision::Deny => CommandExecutionApprovalDecision::Decline,
                };
                serde_json::to_value(CommandExecutionRequestApprovalResponse { decision })
                    .map_err(|e| BridgeError::Resolve(format!("serialize exec response: {e}")))?
            }
            PendingKind::FileChangeApproval => {
                let decision = match decision {
                    ApprovalDecision::Allow => FileChangeApprovalDecision::Accept,
                    ApprovalDecision::AllowForSession => {
                        FileChangeApprovalDecision::AcceptForSession
                    }
                    ApprovalDecision::Deny => FileChangeApprovalDecision::Decline,
                };
                serde_json::to_value(FileChangeRequestApprovalResponse { decision })
                    .map_err(|e| BridgeError::Resolve(format!("serialize file response: {e}")))?
            }
            PendingKind::PermissionApproval => {
                let scope = match decision {
                    ApprovalDecision::Allow => PermissionGrantScope::Turn,
                    ApprovalDecision::AllowForSession => PermissionGrantScope::Session,
                    ApprovalDecision::Deny => PermissionGrantScope::Turn,
                };
                let permissions = match decision {
                    ApprovalDecision::Allow | ApprovalDecision::AllowForSession => pending
                        .requested_permissions
                        .as_ref()
                        .map(granted_from_requested)
                        .unwrap_or_default(),
                    ApprovalDecision::Deny => GrantedPermissionProfile::default(),
                };
                serde_json::to_value(PermissionsRequestApprovalResponse {
                    permissions,
                    scope,
                    strict_auto_review: None,
                })
                .map_err(|e| BridgeError::Resolve(format!("serialize perm response: {e}")))?
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
        let mut answers = HashMap::new();
        answers.insert(
            question_id,
            ToolRequestUserInputAnswer {
                answers: vec![reply],
            },
        );
        let response = ToolRequestUserInputResponse { answers };
        let result_value = serde_json::to_value(response)
            .map_err(|e| BridgeError::Resolve(format!("serialize input response: {e}")))?;
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
        Ok(())
    }

    /// Currently-active thread id. Stable across the bridge's lifetime.
    pub fn thread_id(&self) -> &str {
        &self.thread_id
    }

    /// Forward a resolve to the pump task and await the ack.
    async fn send_resolve(
        &self,
        request_id: RequestId,
        result: serde_json::Value,
    ) -> Result<(), BridgeError> {
        let (ack_tx, ack_rx) = oneshot::channel();
        self.resolve_tx
            .send(ResolveCommand::Resolve {
                request_id,
                result,
                ack: ack_tx,
            })
            .await
            .map_err(|_| BridgeError::Resolve("resolve channel closed".to_string()))?;
        ack_rx
            .await
            .map_err(|_| BridgeError::Resolve("resolve ack channel dropped".to_string()))?
            .map_err(BridgeError::Resolve)
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

/// Body of the spawned event-pump task. Owns the only `&mut self`-grade
/// handle to the in-process client, and is also the only path that
/// owns `&InProcessAppServerClient` for `resolve_server_request` —
/// we route resolves through this task via `resolve_rx`.
async fn run_event_pump(
    client: &mut InProcessAppServerClient,
    pending: Arc<Mutex<HashMap<String, PendingServerRequest>>>,
    emit: Arc<dyn ItemEmitter>,
    shutdown_rx: &mut oneshot::Receiver<()>,
    resolve_rx: &mut mpsc::Receiver<ResolveCommand>,
) {
    let mut text_buffers: HashMap<String, String> = HashMap::new();
    loop {
        tokio::select! {
            _ = &mut *shutdown_rx => {
                debug!("codex-bridge pump received shutdown signal");
                break;
            }
            event = client.next_event() => {
                let Some(event) = event else {
                    debug!("codex-bridge pump: event stream closed by server");
                    break;
                };
                handle_pump_event(event, &pending, &emit, &mut text_buffers).await;
            }
            resolve = resolve_rx.recv() => {
                match resolve {
                    Some(ResolveCommand::Resolve { request_id, result, ack }) => {
                        let outcome = client
                            .resolve_server_request(request_id, result)
                            .await
                            .map_err(|e| format!("{e}"));
                        let _ = ack.send(outcome);
                    }
                    Some(ResolveCommand::Reject { request_id, error, ack }) => {
                        let outcome = client
                            .reject_server_request(request_id, error)
                            .await
                            .map_err(|e| format!("{e}"));
                        let _ = ack.send(outcome);
                    }
                    None => {
                        // Sender dropped (caller dropped CodexAppServer);
                        // we'll exit on shutdown_rx soon.
                    }
                }
            }
        }
    }
    let _ = client;
}

/// Dispatch one [`InProcessServerEvent`].
async fn handle_pump_event(
    event: InProcessServerEvent,
    pending: &Arc<Mutex<HashMap<String, PendingServerRequest>>>,
    emit: &Arc<dyn ItemEmitter>,
    text_buffers: &mut HashMap<String, String>,
) {
    match event {
        InProcessServerEvent::Lagged { skipped } => {
            warn!(skipped, "codex-bridge pump lagged");
        }
        InProcessServerEvent::ServerNotification(notification) => {
            let turn_end = turn_end_from_notification(&notification);
            let nudge_timeline = mappers::is_project_mutating_completion(&notification);
            for item in map_notification(&notification, text_buffers) {
                emit.emit_item(item);
            }
            // Nudge the desktop to refetch project.otio.json after any
            // mutating montage-MCP tool completes. The MCP server lives
            // in a subprocess and can't talk to Tauri's event bus
            // itself, so the bridge is the only hop that knows when
            // OTIO has just been rewritten under the React view.
            if nudge_timeline {
                emit.emit_timeline_changed();
            }
            if let Some(turn_end) = turn_end {
                clear_pending_for_turn(pending, &turn_end.turn_id).await;
                emit.emit_turn_end(turn_end.turn_id, turn_end.error);
            }
        }
        InProcessServerEvent::ServerRequest(request) => {
            handle_server_request(request, pending, emit).await;
        }
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
    match notification {
        ServerNotification::TurnCompleted(n) => Some(TurnEnd {
            turn_id: n.turn.id.clone(),
            error: n.turn.error.as_ref().map(|e| e.message.clone()),
        }),
        ServerNotification::Error(n) => Some(TurnEnd {
            turn_id: n.turn_id.clone(),
            error: Some(n.error.message.clone()),
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
    match request {
        ServerRequest::CommandExecutionRequestApproval { request_id, params } => {
            let item_id = params.item_id.clone();
            let summary = params
                .command
                .clone()
                .unwrap_or_else(|| "(unspecified command)".into());
            let metadata = mappers::build_capability_metadata_for_exec(&summary);
            pending.lock().await.insert(
                item_id.clone(),
                PendingServerRequest {
                    jsonrpc_request_id: request_id,
                    kind: PendingKind::ExecApproval,
                    thread_id: params.thread_id.clone(),
                    turn_id: params.turn_id.clone(),
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
        ServerRequest::FileChangeRequestApproval { request_id, params } => {
            let item_id = params.item_id.clone();
            let metadata =
                mappers::build_capability_metadata_for_file_change(params.reason.as_deref());
            let summary = params
                .reason
                .clone()
                .unwrap_or_else(|| "apply file changes".into());
            pending.lock().await.insert(
                item_id.clone(),
                PendingServerRequest {
                    jsonrpc_request_id: request_id,
                    kind: PendingKind::FileChangeApproval,
                    thread_id: params.thread_id.clone(),
                    turn_id: params.turn_id.clone(),
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
        ServerRequest::PermissionsRequestApproval { request_id, params } => {
            let item_id = params.item_id.clone();
            let summary = params
                .reason
                .clone()
                .unwrap_or_else(|| "request additional permissions".into());
            let metadata =
                mappers::build_capability_metadata_for_file_change(params.reason.as_deref());
            pending.lock().await.insert(
                item_id.clone(),
                PendingServerRequest {
                    jsonrpc_request_id: request_id,
                    kind: PendingKind::PermissionApproval,
                    thread_id: params.thread_id.clone(),
                    turn_id: params.turn_id.clone(),
                    user_input_question_id: None,
                    requested_permissions: Some(params.permissions.clone()),
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
        ServerRequest::ToolRequestUserInput { request_id, params } => {
            let item_id = params.item_id.clone();
            let first = params.questions.first();
            if params.questions.len() > 1 {
                warn!(
                    item_id = %item_id,
                    extra = params.questions.len() - 1,
                    "request_user_input with >1 questions; only the first is rendered in v1"
                );
            }
            let question_text = first.map(|q| q.question.clone()).unwrap_or_default();
            let question_id = first.map(|q| q.id.clone());
            let options = first.and_then(|q| {
                q.options
                    .as_ref()
                    .map(|opts| opts.iter().map(|o| o.label.clone()).collect())
            });
            pending.lock().await.insert(
                item_id.clone(),
                PendingServerRequest {
                    jsonrpc_request_id: request_id,
                    kind: PendingKind::UserInput,
                    thread_id: params.thread_id.clone(),
                    turn_id: params.turn_id.clone(),
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
        other => {
            debug!(id = ?other.id(), "codex-bridge: unhandled ServerRequest variant");
        }
    }
}

/// Promote a requested permission profile to the granted shape we
/// hand back on Allow/Session. We grant exactly what was asked for —
/// scope (Turn vs Session) lives on the response, not the profile.
fn granted_from_requested(requested: &RequestPermissionProfile) -> GrantedPermissionProfile {
    GrantedPermissionProfile {
        network: requested.network.clone(),
        file_system: requested.file_system.clone(),
    }
}

/// Monotonic client-side request id counter. App-server only needs
/// uniqueness within one connection; this counter resets per process.
fn next_request_id() -> i64 {
    use std::sync::atomic::{AtomicI64, Ordering};
    static COUNTER: AtomicI64 = AtomicI64::new(100);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}

/// Compose the session instructions the agent sees once at thread
/// start/resume. The skills catalog belongs here, not inside every user
/// turn, otherwise simple prompts become large internal prompts and get
/// persisted as user text in Codex rollouts.
fn compose_session_instructions(
    developer_instructions: Option<String>,
    skills_catalog: Option<String>,
) -> Option<String> {
    match (developer_instructions, skills_catalog) {
        (Some(dev), Some(skills)) => Some(format!("{}\n\n{}", dev.trim_end(), skills.trim())),
        (Some(dev), None) => Some(dev),
        (None, Some(skills)) => Some(skills),
        (None, None) => None,
    }
}

/// Compose the per-turn text the agent sees: optional recent feedback
/// followed by the user input.
///
/// Recent feedback is behavioural ("don't repeat these rejections"), and
/// earlier prompt text is treated as higher priority by the model, so it
/// leads. When absent, we send the user's input as-is.
///
/// Extracted from `CodexAppServer::start_turn` so the unit test can pin
/// the exact composition without standing up the in-process app-server.
fn compose_turn_input(recent_feedback: Option<&str>, input: &str) -> String {
    let mut parts: Vec<&str> = Vec::with_capacity(2);
    if let Some(fb) = recent_feedback {
        parts.push(fb);
    }
    parts.push(input);
    parts.join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_app_server_protocol::{
        ErrorNotification, Turn, TurnCompletedNotification, TurnError, TurnItemsView, TurnStatus,
    };

    #[test]
    fn turn_end_from_notification_includes_turn_id() {
        let notification = ServerNotification::TurnCompleted(TurnCompletedNotification {
            thread_id: "thread-1".to_string(),
            turn: Turn {
                id: "turn-1".to_string(),
                items: Vec::new(),
                items_view: TurnItemsView::Full,
                status: TurnStatus::Completed,
                error: None,
                started_at: None,
                completed_at: None,
                duration_ms: None,
            },
        });

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
        let notification = ServerNotification::Error(ErrorNotification {
            thread_id: "thread-1".to_string(),
            turn_id: "turn-err".to_string(),
            will_retry: false,
            error: TurnError {
                message: "failed".to_string(),
                codex_error_info: None,
                additional_details: None,
            },
        });

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
                    jsonrpc_request_id: RequestId::Integer(1),
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
                    jsonrpc_request_id: RequestId::Integer(2),
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
    use super::{compose_session_instructions, compose_turn_input};

    #[test]
    fn session_instructions_include_skills_once() {
        let out = compose_session_instructions(Some("<DEV>".to_string()), Some("<SK>".to_string()))
            .expect("instructions");
        assert_eq!(out, "<DEV>\n\n<SK>");
    }

    #[test]
    fn turn_input_does_not_include_skills_catalog() {
        let out = compose_turn_input(Some("<FB>"), "hello");
        assert!(
            !out.contains("<SK>"),
            "skills must not ride per-turn: {out}"
        );
        assert_eq!(out, "<FB>\n\nhello");
    }

    #[test]
    fn simple_chat_turn_stays_plain_user_input() {
        let out = compose_turn_input(None, "how are you");
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
        let out = compose_turn_input(None, "hello");
        assert_eq!(out, "hello");
    }

    #[test]
    fn feedback_only_prefixes_input() {
        let out = compose_turn_input(Some("<FB>"), "hello");
        assert_eq!(out, "<FB>\n\nhello");
    }
}
