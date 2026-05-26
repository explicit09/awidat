//! `Session` — the agent's per-conversation state, plus the two-loop
//! turn driver.
//!
//! Per the corpus survey of `harnesses/codex/codex-rs/core/src/session/turn.rs`:
//! - **Outer turn loop**: one iteration per user input. Builds a request,
//!   runs the inner sampling loop, decides if we need another iteration
//!   (yes if the model emitted tool_use blocks; no otherwise).
//! - **Inner sampling loop**: opens one streaming response, drains it
//!   into [`SessionEvent`]s. Tool calls are dispatched as they arrive
//!   and their results appended to history for the next iteration.
//!
//! Cancellation is plumbed via `tokio_util::sync::CancellationToken`
//! everywhere (Codex pattern). We use `tokio::select!` with the cancel
//! token at every await point in the loop.
//!
//! Events are broadcast via `tokio::sync::broadcast` so the week-5 TUI
//! can subscribe alongside the week-3 REPL.

use std::path::PathBuf;
use std::sync::Arc;

use awidat_proto::project::Project;

use tokio::sync::{Mutex, broadcast, mpsc};
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use crate::FunctionCallError;
use crate::anthropic::{
    Client, ContentBlock, Message, MessagesRequest, Role, StopReason, StreamEvent, ToolChoice,
    Usage,
};
use crate::tool::{
    ApprovalCache, ApprovalRequest, ModelFamily, SandboxMode, ToolContext, ToolInvocation,
    ToolRegistry, UserInputRequest,
};

const ENABLE_ITERATION_COMPACTION: bool = false;

// `SessionEvent` and `SessionError` were lifted to `crate::events` in step
// 8e so they outlive the legacy session/orchestrator modules. Re-exported
// here so unqualified `SessionEvent::…` / `SessionError::…` uses in this
// file's loop body keep compiling unchanged.
pub use crate::events::{SessionError, SessionEvent};

/// Per-conversation state. Cheaply cloneable (`Arc`-wrapped internals).
pub struct Session {
    client: Client,
    registry: ToolRegistry,
    system_prompt: Option<String>,
    model: String,
    project_root: PathBuf,
    history: Arc<Mutex<Vec<Message>>>,
    events_tx: broadcast::Sender<SessionEvent>,
    /// Channel for surfacing `request_user_input` prompts to the REPL/TUI.
    /// `None` if the session was constructed without one — calls to
    /// `request_user_input` then return `RespondToModel`.
    user_input_tx: Option<mpsc::Sender<UserInputRequest>>,
    /// Shared render-job manager handed to every tool via `ToolContext`.
    job_manager: awidat_render::JobManager,
    /// Channel the loop uses to ask the front-end to approve mutating tool
    /// calls. `None` ⇒ loop defaults to allow (tests, batch CLI, MCP).
    approval_tx: Option<mpsc::Sender<ApprovalRequest>>,
    /// Session-scoped operation-keyed approval cache owned by the tool
    /// orchestrator.
    approval_cache: Arc<Mutex<ApprovalCache>>,
    /// Session-scoped MCP host. Tools that need warm ML processes
    /// (clip_search → clip-mcp's query_text) call into this rather
    /// than spawning per-call. Lazy: a registered server is only
    /// launched on first use, and lives for the rest of the session.
    mcp_host: crate::mcp_host::McpHost,
    /// Skills discovered at session start. The L1 catalog (one
    /// line per skill) is emitted as per-turn context; this Arc is
    /// surfaced through `ToolContext` so the `load_skill` tool can
    /// retrieve a skill's full L2 body on demand.
    skills: Arc<crate::skills::SkillRegistry>,
    /// Append-only JSONL session log. `None` ⇒ recording disabled
    /// (tests, MCP server, ad-hoc batch jobs). Mounted by
    /// [`Session::with_recorder`] or [`Session::resume`].
    recorder: Option<crate::rollout::Recorder>,
    /// Sub-agent return slot. `Some` only when this session was
    /// constructed by `delegate` to act as a sub-agent — the
    /// `attempt_completion` tool writes here, and the parent's
    /// `delegate` reads after the sub-session ends. The dispatcher
    /// threads this through `ToolContext.subagent_return`.
    subagent_return: Option<Arc<Mutex<Option<String>>>>,
}

impl Session {
    /// Build a fresh session rooted at `project_root`. Tools that need a
    /// project directory (most of week 4) read it from the
    /// [`ToolContext`] handed to their `handle()`.
    ///
    /// Discovery: if `AGENTS.md` files exist (per
    /// [`crate::awidat_md::discover`]), their concatenated contents
    /// are appended to the supplied `system_prompt` so the model
    /// inherits the producer's editorial conventions every session.
    /// This appended block stays inside the tier-1 prompt cache, so
    /// the cost is paid once per session.
    pub fn new(
        client: Client,
        registry: ToolRegistry,
        model: impl Into<String>,
        system_prompt: Option<String>,
        project_root: impl Into<PathBuf>,
    ) -> Self {
        // Channel must be large enough to absorb a turn's worth of
        // streaming deltas without forcing slow subscribers (the TUI
        // paint loop, in particular) to lag and drop events. A long
        // structural-read prompt against a fully-indexed project can
        // emit 200+ TextDeltas plus tool-call/tool-result pairs in
        // bursts. 128 was the original budget — too tight on long
        // turns, and lag-drops surface as silent gaps in the
        // transcript. 4096 buys us ~100KB of resident memory in the
        // worst case (most events are small) and gives the paint
        // loop generous slack.
        let (events_tx, _) = broadcast::channel(4096);
        let project_root = project_root.into();

        // Discover AGENTS.md (user + project + local override) and
        // splice into the system prompt under a fixed heading. dirs
        // gives us the user home; if it's missing we just skip the
        // user-scope layer and keep going.
        let home_dir = dirs::home_dir();
        let docs = crate::awidat_md::discover(&project_root, home_dir.as_deref());

        // Render the episode-map (Aider repomap pattern, for video).
        // Cheap to render — walks existing sidecars on disk + the
        // OTIO timeline. Only included when the project actually
        // reads cleanly; on a fresh-init/empty project this returns
        // an empty string.
        let episode_map_text = match Project::read(&project_root) {
            Ok(project) => {
                let map = crate::episode_map::render(&project, &project_root);
                if map.trim().is_empty() {
                    None
                } else {
                    Some(map)
                }
            }
            Err(_) => None,
        };

        // Discover skills from the bundled tree + user config dir.
        // The L1 catalog is NOT in the static system prompt — it's
        // emitted per-turn as a `<skills_instructions>`-tagged
        // fragment via `crate::context::AvailableSkillsFragment`,
        // which keeps tier-1 cache stable and lets the compaction
        // pass identify injected catalog blocks via the marker tags.
        let bundled_skills = awidat_config::defaults::skills_root();
        let user_skills = awidat_config::defaults::user_skills_roots();
        let (skill_registry, skill_errors) = crate::skills::SkillRegistry::discover_many(
            bundled_skills.as_deref(),
            user_skills.iter().map(std::path::PathBuf::as_path),
        );
        for err in &skill_errors {
            tracing::warn!(error = %err, "failed to load skill");
        }
        let skill_registry = Arc::new(skill_registry);

        // Read the distilled `learned-style.md` (if `awidat lessons learn`
        // has been run) so the agent inherits the user's accept/reject
        // patterns from past sessions. Stays inside the tier-1 cache
        // prefix; the file changes only between sessions, so the
        // cache stays warm across turns. Missing file = empty string.
        let learned_style = crate::lessons::default_output_path()
            .as_deref()
            .and_then(crate::lessons::read_learned_style);

        // Compose the final system prompt: base prompt → episode map
        // → AGENTS.md → learned style. All extras stay in the tier-1
        // cache prefix so the cost is paid once per session. Skills
        // catalog is per-turn (see above).
        let mut sections: Vec<String> = Vec::new();
        if let Some(base) = system_prompt {
            sections.push(base);
        }
        if let Some(map) = episode_map_text {
            sections.push(format!("## This episode\n\n{map}"));
        }
        if let Some(style) = learned_style {
            sections.push(format!("## Learned style\n\n{style}"));
        }
        if !docs.text.is_empty() {
            sections.push(format!(
                "## Project conventions (AGENTS.md)\n\n{}",
                docs.text
            ));
        }
        let final_system_prompt = if sections.is_empty() {
            None
        } else {
            Some(sections.join("\n\n"))
        };

        // Build the session-scoped MCP host. Reads `[[mcp.servers]]`
        // from the merged user+project config; servers are registered
        // but NOT spawned — first tool call into a server lazily
        // launches it. If config load fails (malformed TOML, etc.) we
        // fall back to an empty host: vision tools that need a server
        // will return UnknownServer, and pure-Rust tools keep working.
        // Best-effort vedit session-start tag. Stamps a `session-start`
        // ref at the current HEAD so `vedit_diff` defaults to
        // session-start..HEAD without needing args from the agent.
        // Failures here are non-fatal — a fresh-init project may not
        // have `project.otio.json` yet, in which case the first
        // `vedit_commit` will create the repo on demand instead.
        match crate::vc::open_or_init(&project_root) {
            Ok(repo) => {
                if let Err(e) = crate::vc::ensure_session_tag(&repo) {
                    tracing::warn!(error = %e, "vedit: ensure_session_tag failed (non-fatal)");
                }
            }
            Err(e) => {
                tracing::debug!(
                    error = %e,
                    "vedit: open_or_init at session start failed (non-fatal — \
                     deferred until first vedit_commit)"
                );
            }
        }

        let mcp_host = match awidat_config::Config::load(Some(&project_root)) {
            Ok(cfg) => crate::mcp_host::McpHost::from_servers(
                &cfg.mcp.servers,
                &project_root,
                awidat_mcp::ClientInfo {
                    name: "awidat".into(),
                    version: env!("CARGO_PKG_VERSION").into(),
                },
            ),
            Err(e) => {
                tracing::warn!(error = %e, "config load failed; MCP host will be empty");
                crate::mcp_host::McpHost::new(awidat_mcp::ClientInfo {
                    name: "awidat".into(),
                    version: env!("CARGO_PKG_VERSION").into(),
                })
            }
        };

        Self {
            client,
            registry,
            system_prompt: final_system_prompt,
            model: model.into(),
            project_root,
            history: Arc::new(Mutex::new(Vec::new())),
            events_tx,
            user_input_tx: None,
            job_manager: awidat_render::JobManager::new(),
            approval_tx: None,
            approval_cache: Arc::new(Mutex::new(ApprovalCache::default())),
            mcp_host,
            skills: skill_registry,
            recorder: None,
            subagent_return: None,
        }
    }

    /// Mount a sub-agent return slot. Used by `delegate` when spawning
    /// a sub-session — the slot is the channel by which the sub-agent's
    /// `attempt_completion` call sends its result back to the parent.
    /// Calling this on a non-sub-session is a no-op semantically (the
    /// `attempt_completion` tool isn't mounted) but harmless.
    #[must_use]
    pub fn with_subagent_return(mut self, slot: Arc<Mutex<Option<String>>>) -> Self {
        self.subagent_return = Some(slot);
        self
    }

    /// Mount a JSONL recorder rooted at `state_root`. The recorder writes
    /// the `SessionMeta` line synchronously; if disk I/O fails the
    /// session continues un-recorded (logged as `warn!`) rather than
    /// aborting — losing the agent loop on a disk hiccup is worse than
    /// losing the rollout.
    #[must_use]
    pub fn with_recorder(mut self, state_root: &std::path::Path) -> Self {
        // Recorder::create handles registry insertion internally;
        // here we just plug in the recorder and log the path.
        match crate::rollout::Recorder::create(
            state_root,
            self.project_root.clone(),
            self.model.clone(),
        ) {
            Ok(rec) => {
                debug!(path = %rec.path().display(), "rollout: recording session");
                self.recorder = Some(rec);
            }
            Err(e) => {
                warn!(error = %e, "rollout: failed to create recorder; continuing un-recorded");
            }
        }
        self
    }

    /// Resume a prior session from a rollout JSONL log. Replays the
    /// log into history. A *new* JSONL file is opened for the resumed
    /// run with `resumed_from = <prior session id>` in its meta — this
    /// keeps each file append-only-once and makes lineage explicit.
    pub async fn resume(
        client: Client,
        registry: ToolRegistry,
        log_path: &std::path::Path,
        state_root: &std::path::Path,
    ) -> Result<Self, SessionError> {
        let (meta, history) = crate::rollout::Recorder::resume(log_path)
            .map_err(|e| SessionError::Other(format!("rollout resume failed: {e}")))?;
        let prompt = crate::system_prompt::assemble_for_project(&meta.project_root);
        let mut session = Self::new(
            client,
            registry,
            meta.model.clone(),
            Some(prompt),
            meta.project_root.clone(),
        );
        *session.history.lock().await = history;
        // Open a fresh recorder with lineage stamped in. The recorder
        // takes care of inserting its own registry row; we follow up
        // by tagging the *previous* session row Completed so the
        // chat-history UI stops showing it as "active".
        let resumed_from_id = meta.id.clone();
        match crate::rollout::Recorder::create_resumed(
            state_root,
            meta.project_root,
            meta.model,
            meta.id,
        ) {
            Ok(rec) => {
                if let Ok(registry) = crate::session_registry::SessionRegistry::open(state_root) {
                    if let Err(e) = registry.set_status(
                        &resumed_from_id,
                        crate::session_registry::SessionStatus::Completed,
                    ) {
                        warn!(error = %e, "session_registry: marking resumed-from session completed failed");
                    }
                }
                session.recorder = Some(rec);
            }
            Err(e) => {
                warn!(error = %e, "rollout: failed to open recorder for resume; continuing un-recorded");
            }
        }
        Ok(session)
    }

    /// Wire an approval channel. The REPL/TUI receives one
    /// [`ApprovalRequest`] per mutating tool call (per
    /// [`crate::ToolHandler::is_mutating`]) and replies with an
    /// [`ApprovalDecision`]. Without this the loop defaults to
    /// [`ApprovalDecision::Allow`] — appropriate for tests, batch CLI,
    /// and the MCP server which run unattended.
    #[must_use]
    pub fn with_approval_channel(mut self, tx: mpsc::Sender<ApprovalRequest>) -> Self {
        self.approval_tx = Some(tx);
        self
    }

    /// Wire a user-input channel. The REPL/TUI receives one
    /// [`UserInputRequest`] per `request_user_input` tool call and replies
    /// via the embedded `oneshot`. Without this, that tool returns
    /// `RespondToModel("interactive input not available in this runtime")`.
    #[must_use]
    pub fn with_user_input_channel(mut self, tx: mpsc::Sender<UserInputRequest>) -> Self {
        self.user_input_tx = Some(tx);
        self
    }

    /// Subscribe to event broadcast. Multiple subscribers are supported
    /// (REPL + TUI in week 5+).
    pub fn subscribe(&self) -> broadcast::Receiver<SessionEvent> {
        self.events_tx.subscribe()
    }

    /// Read-only view of the current conversation history.
    pub async fn history(&self) -> Vec<Message> {
        self.history.lock().await.clone()
    }

    /// Snapshot the current registered tool count. Useful for diagnostics.
    pub fn tool_count(&self) -> usize {
        self.registry.len()
    }

    /// Project root the session was opened against. The TUI's timeline
    /// pane reads `project.otio.json` from here on every `apply_edl`
    /// commit so what's painted stays authoritative.
    pub fn project_root(&self) -> &std::path::Path {
        &self.project_root
    }

    /// Flush the rollout recorder and tell its writer task to drain
    /// and exit. Required when the host swaps to a different session
    /// or shuts down — without this the writer task can still be
    /// holding queued writes when a new session starts pointing at
    /// the same log file, producing duplicate `SessionMeta` lines
    /// and partial messages that break resume.
    ///
    /// Safe to call even when no recorder is attached (returns
    /// immediately). Safe to call after shutdown (subsequent writes
    /// are silent no-ops).
    pub async fn shutdown_recorder(&self) {
        let Some(recorder) = self.recorder.as_ref() else {
            return;
        };
        // Best-effort flush before shutdown so the on-disk log is
        // consistent for the next resume.
        if let Err(err) = recorder.flush().await {
            tracing::warn!(error = %err, "session: recorder flush failed during shutdown");
        }
        // Mark the session Completed in the registry before tearing
        // down the writer task. Best-effort; we don't have the
        // state_root path on Session directly, so we infer it from
        // the rollout's `<state>/sessions/<Y>/<M>/<D>/<file>` layout
        // by walking up four parents.
        if let Some(state_root) = state_root_from_log_path(recorder.path()) {
            if let Ok(registry) = crate::session_registry::SessionRegistry::open(&state_root) {
                let _ = registry.set_status(
                    &recorder.meta().id,
                    crate::session_registry::SessionStatus::Completed,
                );
            }
        }
        recorder.shutdown().await;
    }

    // `execute_structured_plan` was removed in step 8e/Z together with the
    // legacy `structured_plan` and `podcast_workflow` modules. The whole
    // legacy harness (this `session.rs`, `orchestrator.rs`, `anthropic/`,
    // `awidat_md.rs`) is scheduled for deletion in step 8e/W; this method
    // was the only remaining caller of `structured_plan` and had to go
    // first so the workspace stayed green after Z.

    /// Run one turn: append `user_input` to history, drive the two-loop
    /// engine until the model stops (or cancellation, or fatal error).
    pub async fn run_turn(
        &self,
        user_input: impl Into<String>,
        cancel: CancellationToken,
    ) -> Result<(), SessionError> {
        self.run_turn_with_iteration_limit(user_input, cancel, None)
            .await
    }

    /// Like [`Session::run_turn`] but with a caller-supplied cap on
    /// outer-loop iterations. Used by `delegate` to cap sub-agent runs
    /// (cline's `SubagentBuilder` pattern: research sub-agents are
    /// bounded — long sub-sessions are usually a confused-prompt smell,
    /// not a real research need).
    pub async fn run_turn_capped(
        &self,
        user_input: impl Into<String>,
        cancel: CancellationToken,
        max_iterations: usize,
    ) -> Result<(), SessionError> {
        self.run_turn_with_iteration_limit(user_input, cancel, Some(max_iterations))
            .await
    }

    async fn run_turn_with_iteration_limit(
        &self,
        user_input: impl Into<String>,
        cancel: CancellationToken,
        max_iterations: Option<usize>,
    ) -> Result<(), SessionError> {
        let _ = self.events_tx.send(SessionEvent::TurnStart);

        // Bracket this turn in the rollout with TurnStarted /
        // TurnComplete markers. Resume scans for an unmatched
        // TurnStarted and drops every message captured under it,
        // so a crash/kill mid-turn doesn't poison the next session's
        // history.
        let turn_id = self.recorder.as_ref().map(|rec| rec.record_turn_start());

        let user_msg = Message::user_text(user_input);
        if let Some(rec) = &self.recorder {
            rec.record_message(user_msg.clone());
        }
        {
            let mut h = self.history.lock().await;
            h.push(user_msg);
        }

        // Outer loop: keep sampling until the model says end_turn (or
        // we're cancelled).
        //
        let mut compacted = false;
        // Cross-iteration tracker used to detect tool-driven turns
        // that would otherwise end immediately after a tool result.
        // The final model iteration must include narrative text; if it
        // does not, we force one short text-only summary iteration.
        let mut any_tools_in_turn = false;
        let mut forced_summary = false;
        let mut iter = 0usize;
        loop {
            if let Some(max_inner_iterations) = max_iterations
                && iter >= max_inner_iterations
            {
                // Iteration cap reached — defensive against bounded
                // sub-agent loops. Normal desktop turns intentionally
                // do not use this cap; they follow the model/tool
                // protocol until the model reaches a terminal answer,
                // matching the Codex turn-loop shape.
                warn!(max_inner_iterations, "hit iteration cap; pausing turn");
                let message = format!(
                    "Turn paused after {max_inner_iterations} sampling iterations. \
                     I stopped before the next assistant response to avoid an uncontrolled tool loop. \
                     Send \"continue\" to resume from the current project state."
                );
                self.emit_local_turn_summary(&message).await;
                let _ = self.events_tx.send(SessionEvent::Error(message.clone()));
                if let (Some(rec), Some(id)) = (&self.recorder, turn_id) {
                    rec.record_turn_complete(id);
                }
                let _ = self.events_tx.send(SessionEvent::TurnEnd);
                return Err(SessionError::Other(message));
            }

            if cancel.is_cancelled() {
                let _ = self.events_tx.send(SessionEvent::Error("cancelled".into()));
                if let (Some(rec), Some(id)) = (&self.recorder, turn_id.clone()) {
                    rec.record_turn_complete(id);
                }
                return Err(SessionError::Cancelled);
            }

            // Build the request from current history.
            //
            // Two-tier prompt cache (mirrors aider/chat_chunks.py and
            // swe-agent/CacheControlHistoryProcessor):
            //
            //   tier-1 (static): system + tools, marked once on the
            //     last tool. Persists across the whole session.
            //   tier-2 (moving): a breakpoint on the most recent
            //     user-role message that's followed by an assistant
            //     turn — i.e. the latest "stable" history boundary.
            //     Walks forward each iteration; old breakpoints get
            //     cleared first or Anthropic rejects the request.
            //
            // Net effect: the prefix of (system + tools + all prior
            // turns) is cache-read at ~10% input price after the
            // first turn. Cache writes cost +25% on the first turn
            // through, which pays back after 1-2 reuses.
            let mut history_snapshot = self.history.lock().await.clone();
            apply_moving_cache_breakpoint(&mut history_snapshot);

            // Prepend the L1 skills catalog as a tagged user-role
            // fragment for THIS turn. Stays out of self.history so it
            // doesn't accumulate; lives in the per-turn cache tier so
            // tier-1 (system + tools) survives skill installs. Empty
            // registry → no fragment emitted.
            let history_with_context = inject_per_turn_fragments(&self.skills, history_snapshot);
            let mut req = MessagesRequest::new(self.model.clone(), history_with_context)
                .with_max_tokens(4096);
            if let Some(sys) = &self.system_prompt {
                req = req.with_system_cached(sys.clone());
            }
            let family = ModelFamily::from_model_id(&self.model);
            let schemas = self.registry.schemas_for_family(family);
            if !schemas.is_empty() {
                req = req.with_tools(schemas).with_tool_choice(ToolChoice::Auto);
                req.cache_last_tool();
            }

            // Historical note: this used to compact once at 80% of
            // the iteration cap. That caused editorial sessions to
            // restart from a lossy summary even when the context
            // window was not close to full.
            //
            // Compaction failures are non-fatal — we log and continue
            // without compacting; the only cost is hitting the hard
            // stop sooner.
            if max_iterations.is_some_and(|max| should_compact_on_iteration(compacted, iter, max)) {
                compacted = true;
                let history_for_summary = self.history.lock().await.clone();
                match crate::compact::compact_history(&self.client, &history_for_summary, &cancel)
                    .await
                {
                    Ok(summary) if !summary.is_empty() => {
                        let mut h = self.history.lock().await;
                        let replaced_count = h.len();
                        let owned = std::mem::take(&mut *h);
                        *h = crate::compact::replace_history_with_summary(owned, summary);
                        if let Some(rec) = &self.recorder {
                            rec.record_compaction(replaced_count);
                            // Mirror the new (summary-bearing) history
                            // into the log so resume reconstructs it.
                            for m in h.iter() {
                                rec.record_message(m.clone());
                            }
                        }
                        // Re-snapshot since we just rewrote history.
                        let snapshot = h.clone();
                        drop(h);
                        // Surface a TextDelta so the TUI shows the
                        // compaction happened — matters for trust.
                        let _ = self.events_tx.send(SessionEvent::TextDelta(
                            "\n[awidat-compaction] history compacted; resuming with summary.\n"
                                .to_string(),
                        ));
                        // Update the in-flight request with the new
                        // history before sampling. Re-inject the
                        // per-turn skills fragment too — compaction
                        // doesn't preserve it (the fragment is per-
                        // turn, not part of history).
                        let mut new_history = snapshot;
                        apply_moving_cache_breakpoint(&mut new_history);
                        let new_history = inject_per_turn_fragments(&self.skills, new_history);
                        req = MessagesRequest::new(self.model.clone(), new_history)
                            .with_max_tokens(4096);
                        if let Some(sys) = &self.system_prompt {
                            req = req.with_system_cached(sys.clone());
                        }
                        let schemas = self.registry.schemas_for_family(family);
                        if !schemas.is_empty() {
                            req = req.with_tools(schemas).with_tool_choice(ToolChoice::Auto);
                            req.cache_last_tool();
                        }
                    }
                    Ok(_) => {
                        // Empty summary — don't replace history with
                        // nothing. Continue with the original history.
                        warn!("compaction returned empty summary; continuing un-compacted");
                    }
                    Err(e) => {
                        warn!("compaction failed: {e}; continuing un-compacted");
                    }
                }
            }

            let outcome = match self.run_sampling(req, &cancel).await {
                Ok(outcome) => outcome,
                Err(err) => {
                    let _ = self.events_tx.send(SessionEvent::Error(err.to_string()));
                    if let (Some(rec), Some(id)) = (&self.recorder, turn_id.clone()) {
                        rec.record_turn_complete(id);
                    }
                    return Err(err);
                }
            };
            if matches!(outcome.stop_reason, Some(StopReason::ToolUse)) {
                any_tools_in_turn = true;
            }
            iter = iter.saturating_add(1);

            match outcome.stop_reason {
                Some(StopReason::ToolUse) => {
                    // Loop again: history now contains the assistant's
                    // tool_use message + our tool_result reply.
                    debug!("model emitted tool_use; iterating");
                    continue;
                }
                _ => {
                    // The model thinks it's done. If this final
                    // iteration produced no narrative text after prior
                    // tool results, the chat appears to stop on a tool
                    // row. Run one forced iteration that asks the model
                    // to summarize, then close the turn for real.
                    if should_force_summary(forced_summary, any_tools_in_turn, &outcome)
                        && !cancel.is_cancelled()
                    {
                        forced_summary = true;
                        debug!("turn ended without narrative text; forcing one summary iteration");
                        let nudge = Message {
                            role: Role::User,
                            content: vec![ContentBlock::text(
                                "Briefly summarize what you just did and what the user should look at next. \
                                 Do not call any more tools; reply with a short text message only.",
                            )],
                        };
                        {
                            let mut h = self.history.lock().await;
                            h.push(nudge);
                        }
                        continue;
                    }
                    // end_turn / max_tokens / stop_sequence / refusal /
                    // pause_turn — outer loop ends.
                    if should_emit_local_summary_fallback(
                        forced_summary,
                        any_tools_in_turn,
                        &outcome,
                    ) {
                        self.emit_local_turn_summary("Done. I completed the tool work above.")
                            .await;
                    }
                    if let (Some(rec), Some(id)) = (&self.recorder, turn_id.clone()) {
                        rec.record_turn_complete(id);
                    }
                    let _ = self.events_tx.send(SessionEvent::TurnEnd);
                    return Ok(());
                }
            }
        }
    }

    /// One streaming sampling iteration. Returns the inner-loop outcome
    /// (stop reason + usage) after consuming the entire stream.
    async fn run_sampling(
        &self,
        req: MessagesRequest,
        cancel: &CancellationToken,
    ) -> Result<SamplingOutcome, SessionError> {
        use futures::StreamExt;

        let mut stream = Box::pin(self.client.messages_stream(req));
        // Assistant content blocks accumulated this iteration. Appended
        // to history at iteration end so the next request includes them.
        let mut assistant_blocks: Vec<ContentBlock> = Vec::new();
        // Pending tool calls awaiting dispatch (queued during the stream;
        // dispatched after the stream ends so we don't fight the model
        // for the SSE channel).
        let mut pending_calls: Vec<PendingCall> = Vec::new();
        let mut current_text = String::new();
        let mut stop_reason = None;
        let mut usage = Usage::default();
        // Tracks whether this iteration produced any non-whitespace
        // narrative output. The outer loop uses it to detect silent
        // turns (tool calls only) and force one final summary pass.
        let mut emitted_text = false;

        loop {
            tokio::select! {
                biased;
                () = cancel.cancelled() => {
                    let _ = self.events_tx.send(SessionEvent::Error("cancelled".into()));
                    return Err(SessionError::Cancelled);
                }
                next = stream.next() => {
                    let Some(item) = next else { break; };
                    let event = item?;
                    match event {
                        StreamEvent::MessageStart { message_id, model } => {
                            let _ = self.events_tx.send(SessionEvent::MessageStart {
                                message_id, model,
                            });
                        }
                        StreamEvent::TextDelta(t) => {
                            if !t.trim().is_empty() {
                                emitted_text = true;
                            }
                            current_text.push_str(&t);
                            let _ = self.events_tx.send(SessionEvent::TextDelta(t));
                        }
                        StreamEvent::ToolCallStart { id, name } => {
                            // Flush any in-progress text block before the
                            // tool block so message order matches what the
                            // model emitted.
                            if !current_text.is_empty() {
                                assistant_blocks.push(ContentBlock::text(
                                    std::mem::take(&mut current_text),
                                ));
                            }
                            let _ = self.events_tx.send(SessionEvent::ToolCallStart {
                                id: id.clone(),
                                name: name.clone(),
                            });
                            pending_calls.push(PendingCall {
                                id, name, args: None, args_err: None,
                            });
                        }
                        StreamEvent::ToolCallEnd { id, name, result } => {
                            let pending = pending_calls
                                .iter_mut()
                                .find(|c| c.id == id);
                            match (pending, result) {
                                (Some(p), Ok(args)) => {
                                    p.args = Some(args.clone());
                                    let _ = self.events_tx.send(SessionEvent::ToolCallArgs {
                                        id, name, args,
                                    });
                                }
                                (Some(p), Err(msg)) => {
                                    p.args_err = Some(msg.clone());
                                    let _ = self.events_tx.send(SessionEvent::ToolResult {
                                        id, name,
                                        result: Err(msg),
                                    });
                                }
                                (None, _) => {
                                    warn!(call_id = %id, "ToolCallEnd without matching Start");
                                }
                            }
                        }
                        StreamEvent::Done { stop_reason: sr, usage: u } => {
                            stop_reason = sr;
                            usage = u;
                            break;
                        }
                    }
                }
            }
        }

        // Flush any final text into a block.
        if !current_text.is_empty() {
            assistant_blocks.push(ContentBlock::text(current_text));
        }
        // Append the assistant's tool_use blocks (with the input we parsed)
        // so the model's next view of history is complete.
        for call in &pending_calls {
            assistant_blocks.push(ContentBlock::ToolUse {
                id: call.id.clone(),
                name: call.name.clone(),
                input: call.args.clone().unwrap_or(serde_json::json!({})),
            });
        }
        let assistant_msg = if assistant_blocks.is_empty() {
            None
        } else {
            Some(Message {
                role: Role::Assistant,
                content: assistant_blocks,
            })
        };

        // Dispatch tool calls and append their results as a single user
        // message (Anthropic's tool-result protocol expects all tool_results
        // for one assistant message to land in one user message).
        if !pending_calls.is_empty() {
            let mut result_blocks = Vec::with_capacity(pending_calls.len());
            for call in pending_calls {
                let block = self.dispatch_tool(call, cancel).await?;
                result_blocks.push(block);
            }
            let tool_result_msg = Message {
                role: Role::User,
                content: result_blocks,
            };
            if let Some(assistant_msg) = assistant_msg {
                if let Some(rec) = &self.recorder {
                    rec.record_message(assistant_msg.clone());
                }
                let mut h = self.history.lock().await;
                h.push(assistant_msg);
            }
            if let Some(rec) = &self.recorder {
                rec.record_message(tool_result_msg.clone());
            }
            let mut h = self.history.lock().await;
            h.push(tool_result_msg);
        } else if let Some(assistant_msg) = assistant_msg {
            if let Some(rec) = &self.recorder {
                rec.record_message(assistant_msg.clone());
            }
            let mut h = self.history.lock().await;
            h.push(assistant_msg);
        }

        let _ = self.events_tx.send(SessionEvent::SamplingComplete {
            stop_reason,
            usage: usage.clone(),
        });
        Ok(SamplingOutcome {
            stop_reason,
            usage,
            emitted_text,
        })
    }

    /// Dispatch one tool call. Returns the `ToolResult` content block to
    /// feed back to the model. Recoverable failures (`RespondToModel`,
    /// args-parse error) become `is_error: true` results; `Fatal` aborts.
    async fn dispatch_tool(
        &self,
        call: PendingCall,
        cancel: &CancellationToken,
    ) -> Result<ContentBlock, SessionError> {
        // If args failed to parse, surface the parse error directly without
        // dispatching. We MUST emit a ToolResult event before returning,
        // otherwise the TUI's Running tool-call state stays stuck on
        // the spinner forever (caught by Week-8 demo bringup).
        if let Some(err) = call.args_err {
            let msg = format!("tool '{}' arguments failed to parse: {err}", call.name);
            let _ = self.events_tx.send(SessionEvent::ToolResult {
                id: call.id.clone(),
                name: call.name.clone(),
                result: Err(msg.clone()),
            });
            return Ok(ContentBlock::ToolResult {
                tool_use_id: call.id,
                content: crate::anthropic::tool_result::text(msg),
                is_error: Some(true),
            });
        }
        let args = call.args.unwrap_or(serde_json::json!({}));
        let Some(handler) = self.registry.get(&call.name) else {
            let _ = self.events_tx.send(SessionEvent::ToolResult {
                id: call.id.clone(),
                name: call.name.clone(),
                result: Err(format!("no such tool: {}", call.name)),
            });
            return Ok(ContentBlock::ToolResult {
                tool_use_id: call.id,
                content: crate::anthropic::tool_result::text(format!(
                    "tool '{}' is not registered. Available: {:?}",
                    call.name,
                    self.registry.names().collect::<Vec<_>>()
                )),
                is_error: Some(true),
            });
        };

        let invocation = ToolInvocation {
            call_id: call.id.clone(),
            name: call.name.clone(),
            args,
        };

        let ctx = ToolContext {
            project_root: self.project_root.clone(),
            events_tx: self.events_tx.clone(),
            user_input_tx: self.user_input_tx.clone(),
            job_manager: self.job_manager.clone(),
            approval_tx: self.approval_tx.clone(),
            sandbox_mode: SandboxMode::Default,
            mcp_host: self.mcp_host.clone(),
            skills: self.skills.clone(),
            subagent_return: self.subagent_return.clone(),
        };

        let orchestrator = crate::orchestrator::ToolOrchestrator::new(
            self.approval_tx.clone(),
            self.approval_cache.clone(),
            self.recorder.clone(),
        );
        let result = match orchestrator.run(handler, invocation, ctx, cancel).await {
            Ok(result) => result,
            Err(SessionError::ToolDenied { message, .. }) => {
                let _ = self.events_tx.send(SessionEvent::ToolResult {
                    id: call.id.clone(),
                    name: call.name.clone(),
                    result: Err(message.clone()),
                });
                return Ok(ContentBlock::ToolResult {
                    tool_use_id: call.id,
                    content: crate::anthropic::tool_result::text(message),
                    is_error: Some(true),
                });
            }
            Err(SessionError::Cancelled) => {
                // Emit a ToolResult event before bailing — otherwise
                // the TUI's Running spinner for this call sits there
                // forever after a Ctrl-C cancel.
                let _ = self.events_tx.send(SessionEvent::ToolResult {
                    id: call.id.clone(),
                    name: call.name.clone(),
                    result: Err("cancelled".into()),
                });
                return Err(SessionError::Cancelled);
            }
            Err(e) => return Err(e),
        };

        match result {
            Ok(out) => {
                // Build the wire content: plain string for text-only,
                // multi-block array when the tool attached images.
                let wire_content = if out.images.is_empty() {
                    crate::anthropic::tool_result::text(&out.content)
                } else {
                    crate::anthropic::tool_result::text_and_images(&out.content, &out.images)
                };
                // The event broadcast carries just the text part; the TUI
                // renders images via the file-cache path (week 5+).
                let _ = self.events_tx.send(SessionEvent::ToolResult {
                    id: call.id.clone(),
                    name: call.name.clone(),
                    result: Ok(out.content.clone()),
                });
                Ok(ContentBlock::ToolResult {
                    tool_use_id: call.id,
                    content: wire_content,
                    is_error: None,
                })
            }
            Err(FunctionCallError::Fatal(msg)) => {
                // Emit a terminal ToolResult so the TUI clears the
                // Running spinner for this specific call before we
                // surface the fatal error globally.
                let _ = self.events_tx.send(SessionEvent::ToolResult {
                    id: call.id.clone(),
                    name: call.name.clone(),
                    result: Err(format!("fatal: {msg}")),
                });
                let _ = self
                    .events_tx
                    .send(SessionEvent::Error(format!("fatal: {msg}")));
                Err(SessionError::Fatal(msg))
            }
            Err(other) => {
                let msg = other.to_string();
                let _ = self.events_tx.send(SessionEvent::ToolResult {
                    id: call.id.clone(),
                    name: call.name.clone(),
                    result: Err(msg.clone()),
                });
                Ok(ContentBlock::ToolResult {
                    tool_use_id: call.id,
                    content: crate::anthropic::tool_result::text(msg),
                    is_error: Some(true),
                })
            }
        }
    }

    async fn emit_local_turn_summary(&self, text: &str) {
        let _ = self
            .events_tx
            .send(SessionEvent::TextDelta(text.to_string()));
        let message = Message::assistant_text(text.to_string());
        if let Some(rec) = &self.recorder {
            rec.record_message(message.clone());
        }
        let mut h = self.history.lock().await;
        h.push(message);
    }
}

/// Recover the rollout's `<state_root>` from a session log path,
/// assuming the standard `<state_root>/sessions/<YYYY>/<MM>/<DD>/<file>.jsonl`
/// layout used by `Recorder::create`. Returns None if the path
/// doesn't conform (e.g. tests with custom roots can have shallower
/// layouts; this is best-effort).
fn state_root_from_log_path(log_path: &std::path::Path) -> Option<std::path::PathBuf> {
    // Layout: <state_root>/sessions/<YYYY>/<MM>/<DD>/<file>.jsonl
    //   parent(file)            -> <DD>
    //   then walk up 4 more     -> <state_root>
    let dd = log_path.parent()?;
    let mm = dd.parent()?;
    let yyyy = mm.parent()?;
    let sessions = yyyy.parent()?;
    sessions.parent().map(|p| p.to_path_buf())
}

#[cfg(test)]
fn summarize_args(args: &serde_json::Value) -> String {
    crate::tool::summarize_args(args)
}

/// Walk the history and place the *moving* tier-2 cache breakpoint.
///
/// Strategy mirrors aider's `chat_chunks.py:add_cache_control_headers`
/// and swe-agent's `CacheControlHistoryProcessor`:
///
/// 1. Clear cache marks from every prior message — Anthropic rejects
///    requests that have stale breakpoints scattered through history.
/// 2. Scan from the end backwards; mark the first User message we
///    find. That covers the entire prefix from the start of the
///    request up to and including that user turn.
///
/// On the first turn (history = `[Message::user_text(prompt)]`) the
/// loop marks the very prompt the user just sent — fine; that
/// becomes the next turn's tier-2 cache hit.
///
/// On later turns, marking the latest user message means everything
/// before the assistant's *current* turn (which we're about to
/// regenerate) is cached. The volatile bit — the new tool_result we
/// just appended, or the fresh assistant emission — sits past the
/// breakpoint and is fresh-billed. That's the right slice.
fn apply_moving_cache_breakpoint(messages: &mut [Message]) {
    for m in messages.iter_mut() {
        m.clear_cache_breakpoint();
    }
    if let Some(latest_user) = messages.iter_mut().rev().find(|m| m.role == Role::User) {
        latest_user.set_cache_breakpoint();
    }
}

/// Build the per-turn message list: prepend any contextual fragments
/// (e.g. the L1 skills catalog) to the persisted history. The result
/// is what we hand the API; `self.history` stays clean so fragments
/// don't accumulate session-over-session.
///
/// Today's only fragment is the skills catalog. Future-us can add
/// more (timeline-state delta, recent-edits log) by extending this
/// function — it's the single splice point for per-turn context.
fn inject_per_turn_fragments(
    skills: &crate::skills::SkillRegistry,
    history: Vec<Message>,
) -> Vec<Message> {
    use crate::context::ContextualUserFragment;
    let mut out = Vec::with_capacity(history.len() + 1);
    if let Some(frag) = skills.l1_fragment() {
        out.push(frag.into_message());
    }
    out.extend(history);
    out
}

/// One in-progress tool call accumulating during the stream.
struct PendingCall {
    id: String,
    name: String,
    /// Set when args fully parse.
    args: Option<serde_json::Value>,
    /// Set when args fail to parse.
    args_err: Option<String>,
}

/// What one inner-loop iteration produced.
struct SamplingOutcome {
    stop_reason: Option<StopReason>,
    /// Currently unused but threaded through for future telemetry.
    #[allow(dead_code)]
    usage: Usage,
    /// True when this iteration emitted at least one non-empty
    /// `TextDelta` to the user. Outer loop uses this to detect
    /// silent turns (only tool calls, no narrative reply) and to
    /// inject a one-shot "summarize what you just did" follow-up
    /// so chats never end in silence.
    emitted_text: bool,
}

fn should_force_summary(
    forced_summary: bool,
    any_tools_in_turn: bool,
    outcome: &SamplingOutcome,
) -> bool {
    !forced_summary
        && any_tools_in_turn
        && !outcome.emitted_text
        && !matches!(outcome.stop_reason, Some(StopReason::ToolUse))
}

fn should_emit_local_summary_fallback(
    forced_summary: bool,
    any_tools_in_turn: bool,
    outcome: &SamplingOutcome,
) -> bool {
    forced_summary
        && any_tools_in_turn
        && !outcome.emitted_text
        && !matches!(outcome.stop_reason, Some(StopReason::ToolUse))
}

fn should_compact_on_iteration(compacted: bool, iter: usize, max_inner_iterations: usize) -> bool {
    if !ENABLE_ITERATION_COMPACTION || compacted {
        return false;
    }
    let compact_at_iteration = (max_inner_iterations * 4) / 5;
    iter >= compact_at_iteration
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Smoke: Session can be constructed without panicking and reports
    /// zero registered tools by default.
    #[test]
    fn empty_session_construction() {
        // Use a bogus client (no network calls happen at construction).
        let client = Client::new("test-key", crate::anthropic::ClientConfig::default())
            .expect("client construction without network");
        let s = Session::new(
            client,
            ToolRegistry::new(),
            "claude-haiku-4-5-20251001",
            None,
            std::env::temp_dir(),
        );
        assert_eq!(s.tool_count(), 0);
    }

    /// Wiring smoke: `with_recorder` mounts a recorder and the JSONL
    /// path lands under the supplied state root. Doesn't run a turn —
    /// that needs a live API; this just exercises the constructor path.
    #[tokio::test]
    async fn with_recorder_creates_log_under_state_root() {
        let client =
            Client::new("test-key", crate::anthropic::ClientConfig::default()).expect("client");
        let dir = tempfile::tempdir().unwrap();
        let s = Session::new(
            client,
            ToolRegistry::new(),
            "claude-haiku-4-5-20251001",
            None,
            std::env::temp_dir(),
        )
        .with_recorder(dir.path());
        assert!(s.recorder.is_some(), "recorder must be mounted");
        let path = s.recorder.as_ref().unwrap().path();
        assert!(path.starts_with(dir.path().join("sessions")));
        assert!(path.exists(), "log file should be created");
    }

    #[tokio::test]
    async fn client_error_completes_recorded_turn_and_emits_error() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = [0_u8; 1024];
            let _ = socket.read(&mut buf).await;
            let body = r#"{"error":{"message":"synthetic failure"}}"#;
            let response = format!(
                "HTTP/1.1 500 Internal Server Error\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            socket.write_all(response.as_bytes()).await.unwrap();
        });

        let config = crate::anthropic::ClientConfig {
            base_url: format!("http://{addr}"),
            ..crate::anthropic::ClientConfig::default()
        };
        let client = Client::new("test-key", config).expect("client");
        let dir = tempfile::tempdir().unwrap();
        let s = Session::new(
            client,
            ToolRegistry::new(),
            "claude-haiku-4-5-20251001",
            None,
            std::env::temp_dir(),
        )
        .with_recorder(dir.path());
        let mut events = s.subscribe();

        let err = s
            .run_turn("hello", CancellationToken::new())
            .await
            .expect_err("API failure should abort the turn");
        assert!(err.to_string().contains("synthetic failure"));

        let mut saw_error = false;
        while let Ok(event) = events.try_recv() {
            if matches!(event, SessionEvent::Error(msg) if msg.contains("synthetic failure")) {
                saw_error = true;
                break;
            }
        }
        assert!(
            saw_error,
            "client failure should be visible in the item stream"
        );

        let recorder = s.recorder.as_ref().expect("recorder mounted");
        recorder.flush().await.unwrap();
        let log = std::fs::read_to_string(recorder.path()).unwrap();
        assert!(log.contains(r#""type":"turn_started""#));
        assert!(log.contains(r#""type":"turn_complete""#));
    }

    #[tokio::test]
    async fn iteration_cap_is_visible_and_not_success() {
        let client =
            Client::new("test-key", crate::anthropic::ClientConfig::default()).expect("client");
        let dir = tempfile::tempdir().unwrap();
        let s = Session::new(
            client,
            ToolRegistry::new(),
            "claude-haiku-4-5-20251001",
            None,
            std::env::temp_dir(),
        )
        .with_recorder(dir.path());
        let mut events = s.subscribe();

        let err = s
            .run_turn_capped("hello", CancellationToken::new(), 0)
            .await
            .expect_err("iteration cap should not report success");
        assert!(
            err.to_string()
                .contains("Turn paused after 0 sampling iterations")
        );

        let mut saw_text = false;
        let mut saw_error = false;
        while let Ok(event) = events.try_recv() {
            match event {
                SessionEvent::TextDelta(text) if text.contains("Turn paused after 0") => {
                    saw_text = true;
                }
                SessionEvent::Error(message) if message.contains("Turn paused after 0") => {
                    saw_error = true;
                }
                _ => {}
            }
        }
        assert!(saw_text, "cap pause should be visible as assistant text");
        assert!(saw_error, "cap pause should also surface as an error");

        let recorder = s.recorder.as_ref().expect("recorder mounted");
        recorder.flush().await.unwrap();
        let log = std::fs::read_to_string(recorder.path()).unwrap();
        assert!(log.contains("Turn paused after 0 sampling iterations"));
        assert!(log.contains(r#""type":"turn_complete""#));
    }

    /// Wiring smoke: `with_approval_channel` populates the field. The
    /// full approval round-trip is exercised by the TUI bringup test.
    #[test]
    fn with_approval_channel_wires_the_field() {
        let client =
            Client::new("test-key", crate::anthropic::ClientConfig::default()).expect("client");
        let (tx, _rx) = mpsc::channel(8);
        let s = Session::new(
            client,
            ToolRegistry::new(),
            "claude-haiku-4-5-20251001",
            None,
            std::env::temp_dir(),
        )
        .with_approval_channel(tx);
        assert!(s.approval_tx.is_some(), "approval_tx must be set");
    }

    #[test]
    fn summarize_args_truncates_long_payloads() {
        let v = serde_json::json!({"edl": "x".repeat(500)});
        let s = summarize_args(&v);
        // 200 chars of ASCII + a multi-byte '…' (3 bytes UTF-8). Use
        // chars() not bytes for the cap.
        assert!(
            s.chars().count() <= 201,
            "200 chars + ellipsis: {}",
            s.chars().count()
        );
        assert!(s.ends_with('…'));
    }

    #[test]
    fn summarize_args_squashes_whitespace() {
        let v = serde_json::json!({"edl": "line1\n\nline2\n   line3"});
        let s = summarize_args(&v);
        assert!(!s.contains('\n'), "newlines squashed: {s}");
    }

    #[test]
    fn forces_summary_when_final_iteration_after_tools_has_no_text() {
        let outcome = SamplingOutcome {
            stop_reason: None,
            usage: Usage::default(),
            emitted_text: false,
        };

        assert!(should_force_summary(false, true, &outcome));
    }

    #[test]
    fn does_not_force_summary_when_final_iteration_has_text() {
        let outcome = SamplingOutcome {
            stop_reason: None,
            usage: Usage::default(),
            emitted_text: true,
        };

        assert!(!should_force_summary(false, true, &outcome));
    }

    #[test]
    fn emits_local_summary_when_forced_summary_stays_silent() {
        let outcome = SamplingOutcome {
            stop_reason: None,
            usage: Usage::default(),
            emitted_text: false,
        };

        assert!(should_emit_local_summary_fallback(true, true, &outcome));
    }

    #[test]
    fn does_not_compact_from_iteration_count_alone() {
        assert!(!should_compact_on_iteration(false, 19, 24));
        assert!(!should_compact_on_iteration(false, 24, 24));
    }

    #[test]
    fn moving_cache_breakpoint_marks_latest_user_message_only() {
        let mut history = vec![
            Message::user_text("first user prompt"),
            Message::assistant_text("first reply"),
            Message::user_text("second user prompt"),
            Message::assistant_text("second reply"),
        ];
        apply_moving_cache_breakpoint(&mut history);

        fn marked(m: &Message) -> usize {
            m.content
                .iter()
                .filter(|b| {
                    matches!(
                        b,
                        crate::anthropic::ContentBlock::Text {
                            cache_control: Some(_),
                            ..
                        }
                    )
                })
                .count()
        }
        // Only the *last* user message should carry a breakpoint.
        assert_eq!(marked(&history[0]), 0, "first user not marked");
        assert_eq!(marked(&history[1]), 0, "first assistant not marked");
        assert_eq!(marked(&history[2]), 1, "second user marked");
        assert_eq!(marked(&history[3]), 0, "second assistant not marked");
    }

    #[test]
    fn per_turn_skill_catalog_is_metadata_only_and_not_persisted() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join("skills").join("sample-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: sample-skill\ndescription: short routing hint\n---\n\
             # Secret Body\n\nRun scripts/private_helper.py when needed.",
        )
        .unwrap();
        let (registry, errors) =
            crate::skills::SkillRegistry::discover(Some(&dir.path().join("skills")), None);
        assert!(errors.is_empty(), "{errors:?}");

        let history = vec![Message::user_text("please do regular editing")];
        let injected = inject_per_turn_fragments(&registry, history);
        assert_eq!(injected.len(), 2);

        let catalog = text_of_first_block(&injected[0]);
        assert!(catalog.contains("<skills_instructions>"));
        assert!(catalog.contains("sample-skill"));
        assert!(catalog.contains("short routing hint"));
        assert!(
            !catalog.contains("Secret Body"),
            "L1 must not include SKILL.md body text"
        );
        assert!(
            !catalog.contains("scripts/private_helper.py"),
            "L1 must not include L3 script paths"
        );

        let original_user = text_of_first_block(&injected[1]);
        assert_eq!(original_user, "please do regular editing");
    }

    #[test]
    fn moving_cache_breakpoint_clears_old_marks_before_remarking() {
        // Simulate a stale breakpoint left from a prior turn on an
        // earlier user message.
        let mut older = Message::user_text("old prompt");
        older.set_cache_breakpoint();
        let mut history = vec![
            older,
            Message::assistant_text("reply"),
            Message::user_text("new prompt"),
        ];
        apply_moving_cache_breakpoint(&mut history);

        // Old mark cleared.
        let stale_marked = matches!(
            &history[0].content[0],
            crate::anthropic::ContentBlock::Text {
                cache_control: Some(_),
                ..
            }
        );
        assert!(!stale_marked, "old breakpoint must be cleared");
        // New mark on the latest user message.
        let new_marked = matches!(
            &history[2].content[0],
            crate::anthropic::ContentBlock::Text {
                cache_control: Some(_),
                ..
            }
        );
        assert!(new_marked, "new breakpoint must be set");
    }

    fn text_of_first_block(message: &Message) -> &str {
        match &message.content[0] {
            crate::anthropic::ContentBlock::Text { text, .. } => text,
            other => panic!("expected text block; got {other:?}"),
        }
    }
}
