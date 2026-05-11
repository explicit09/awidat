//! Append-only JSONL session log + resume.
//!
//! Per `PLAN.md` §15 (Week 8 follow-up — task #151) and the cross-harness
//! survey: codex, cline, crush, and claude-agent-sdk all persist sessions
//! as append-only JSONL written from a background writer task. Shape ported
//! from `harnesses/codex/codex-rs/rollout/src/recorder.rs:74-99` (mpsc
//! `RolloutCmd::AddItems` → background writer → `BufWriter<File>` flushed
//! after every batch). Simplified for awidat: no thread DB, no archive, no
//! event-persistence policy — just record + resume.
//!
//! On disk:
//! `<state_root>/sessions/<YYYY>/<MM>/<DD>/rollout-<rfc3339>-<id12>.jsonl`
//! where `<state_root>` is `$AWIDAT_STATE_ROOT` if set, else
//! `~/.local/share/awidat` (matches install layout). The first line is
//! `RolloutItem::SessionMeta`; subsequent lines are `RolloutItem::Message`
//! and `RolloutItem::Compaction` events in append order.

use std::collections::HashSet;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tracing::{debug, warn};

use crate::anthropic::{ContentBlock, Message, Role};

/// One entry in the JSONL log. Each variant is one line.
///
/// Stable on-disk: renaming a variant breaks resume of older logs. New
/// variants must be additive — older awidat versions reading a newer log
/// must skip unknown lines (handled by `Recorder::resume`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RolloutItem {
    /// First line of every log. Identifies the session.
    SessionMeta(SessionMeta),
    /// One message in the conversation history (user, assistant).
    Message(Message),
    /// Marker that a compaction pass replaced history with a summary.
    /// `replaced_messages` is the count of pre-compaction messages so the
    /// reader knows what got dropped. The summary itself is the next
    /// `Message` line.
    Compaction {
        /// Number of pre-compaction messages that the live agent loop
        /// dropped when it spliced in the summary.
        replaced_messages: usize,
    },
    /// Captured user decision on a mutating tool call. Per #149: every
    /// approval modal outcome (Allow, AllowForSession, Deny) is logged
    /// here so the offline `awidat lessons learn` step can build
    /// patterns. The `args_summary` is the short truncated form already
    /// rendered to the user in the modal — not the full args, since
    /// EDLs can be large.
    EditorialDecision(EditorialDecision),
}

/// One captured editorial decision. Stable on disk: extending this
/// struct must be additive (new fields with `#[serde(default)]`), since
/// older session logs may already be on disk when a new awidat reads
/// them.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditorialDecision {
    /// Tool name (`apply_edl`, `start_render`, `bash`, …).
    pub tool: String,
    /// Same short summary the modal showed the user. Truncated; carries
    /// just enough signal for pattern extraction (e.g. `apply_edl: 3
    /// ops, score=0.72, kind=hook`). Caller-supplied — `Session`
    /// builds it from the args + tool-specific summarizer.
    pub args_summary: String,
    /// Operation-scoped approval keys considered by the orchestrator.
    /// Used to debug why an AllowForSession did or did not cover a
    /// later request.
    #[serde(default)]
    pub approval_keys: Vec<crate::tool::ApprovalKey>,
    /// Present for an explicit unsandboxed retry prompt after sandbox
    /// denial. Absence means this was the first-attempt approval.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_reason: Option<String>,
    /// What the user picked. String form for forward-compat: a future
    /// awidat may add new decision variants and we don't want a session
    /// log to be unreadable just because the enum grew.
    pub decision: String,
    /// When the modal was shown.
    pub timestamp: DateTime<Utc>,
}

/// Per-session metadata written as the first line of the JSONL log.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMeta {
    /// 12-char hex session id. Embedded in the filename and shown in
    /// `awidat resume` listings.
    pub id: String,
    /// Project root the session was opened against.
    pub project_root: PathBuf,
    /// Model name passed to the Anthropic API for this session.
    pub model: String,
    /// Wall-clock time `Recorder::create` ran.
    pub started_at: DateTime<Utc>,
    /// awidat version that wrote the log. Surfaced when resume detects a
    /// mismatch so the user knows what produced it.
    pub awidat_version: String,
    /// If this session was started via `awidat resume <path>`, the
    /// session id of the parent log. `None` for fresh sessions.
    /// Lets us trace conversation lineage across resumes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resumed_from: Option<String>,
}

/// Background writer command. Mirrors codex's `RolloutCmd` shape but
/// without the persist-mode sanitization step (we don't ship an extended
/// event stream — only the typed items defined here).
enum WriterCmd {
    Append(RolloutItem),
    Flush(oneshot::Sender<std::io::Result<()>>),
    Shutdown,
}

/// Append-only session recorder. Cheap to clone — clones share the same
/// background writer task via the channel.
///
/// Uses an UNBOUNDED mpsc channel intentionally. A bounded channel
/// caused dropped messages under sustained load: a real session
/// chains 50+ tool calls per turn, faster than the writer task can
/// flush 5KB JSONL lines to disk. With a bounded channel, `try_send`
/// failures silently lost ~95% of messages on a 5K-message stress
/// load (caught by `eval/src/stress.rs::large_rollout_resume`). The
/// memory cost of unbounded is paid by the conversation history we
/// already keep in `Vec<Message>` elsewhere — so the queue's worst
/// case is the same as already-resident memory.
#[derive(Clone)]
pub struct Recorder {
    tx: mpsc::UnboundedSender<WriterCmd>,
    path: Arc<PathBuf>,
    meta: Arc<SessionMeta>,
    /// Held so the writer task lives at least as long as any clone. The
    /// task exits cleanly on `Shutdown` or when the channel closes.
    _task: Arc<JoinHandle<()>>,
}

impl Recorder {
    /// Create a fresh log under `state_root`. The file is opened
    /// immediately (so disk errors surface synchronously) and the
    /// `SessionMeta` line is written before this returns.
    ///
    /// Errors here propagate; the caller decides whether to run un-
    /// recorded. `Session::new` chooses to log + continue rather than
    /// fail the whole session.
    pub fn create(
        state_root: &Path,
        project_root: PathBuf,
        model: String,
    ) -> std::io::Result<Self> {
        let started_at = Utc::now();
        let id = generate_session_id(started_at);
        let path = session_path(state_root, started_at, &id);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
        let meta = SessionMeta {
            id,
            project_root,
            model,
            started_at,
            awidat_version: env!("CARGO_PKG_VERSION").to_string(),
            resumed_from: None,
        };
        // Write the meta line synchronously so a failed disk surfaces
        // before we return a Recorder the caller will trust.
        write_line(&mut file, &RolloutItem::SessionMeta(meta.clone()))?;

        let (tx, rx) = mpsc::unbounded_channel::<WriterCmd>();
        let task = tokio::spawn(writer_loop(rx, file, path.clone()));

        Ok(Self {
            tx,
            path: Arc::new(path),
            meta: Arc::new(meta),
            _task: Arc::new(task),
        })
    }

    /// Like [`Recorder::create`] but stamps `resumed_from` in the
    /// new file's meta line so lineage is recoverable.
    pub fn create_resumed(
        state_root: &Path,
        project_root: PathBuf,
        model: String,
        resumed_from: String,
    ) -> std::io::Result<Self> {
        let mut r = Self::create(state_root, project_root, model)?;
        // The meta we already wrote has `resumed_from = None`. Patch
        // both the in-memory meta (for `meta()`) and append a second
        // `SessionMeta` line with the lineage. resume() uses the FIRST
        // SessionMeta line, so this lineage marker is informational —
        // surfaced via `Recorder::list` filtering and the future
        // `awidat history` view.
        let mut patched = (*r.meta).clone();
        patched.resumed_from = Some(resumed_from);
        r.meta = Arc::new(patched.clone());
        // Send through the writer task so ordering is preserved
        // relative to subsequent message lines.
        r.send(WriterCmd::Append(RolloutItem::SessionMeta(patched)));
        Ok(r)
    }

    /// Replay a log into a `(SessionMeta, Vec<Message>)` for `--resume`.
    /// Lines that fail to parse (older/newer schema, partial last line
    /// from an unflushed crash) are warned-and-skipped.
    pub fn resume(path: &Path) -> std::io::Result<(SessionMeta, Vec<Message>)> {
        let body = std::fs::read_to_string(path)?;
        let mut meta: Option<SessionMeta> = None;
        let mut messages: Vec<Message> = Vec::new();
        for (lineno, line) in body.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<RolloutItem>(line) {
                Ok(RolloutItem::SessionMeta(m)) => {
                    if meta.is_none() {
                        meta = Some(m);
                    }
                }
                Ok(RolloutItem::Message(m)) => messages.push(m),
                Ok(RolloutItem::Compaction { replaced_messages }) => {
                    // The next Message line is the summary — replay
                    // semantics: drop everything before this point and
                    // keep only the summary that follows. Mirror the
                    // live compaction path in session.rs.
                    debug!(
                        replaced_messages,
                        "rollout: compaction marker; clearing replayed history"
                    );
                    messages.clear();
                }
                Ok(RolloutItem::EditorialDecision(_)) => {
                    // Decisions are pure metadata — they don't affect
                    // the resumed conversation history. Read offline by
                    // `awidat lessons learn` (see #150).
                }
                Err(e) => {
                    warn!(
                        line = lineno + 1,
                        error = %e,
                        "rollout: skipping unparseable line"
                    );
                }
            }
        }
        let Some(meta) = meta else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("{}: no SessionMeta line found", path.display()),
            ));
        };
        Ok((meta, sanitize_resume_messages(messages)))
    }

    /// Read every `EditorialDecision` from every rollout under
    /// `state_root`. Used by `awidat lessons learn` (see #150) to build
    /// a histogram of accept/reject patterns. Order is best-effort
    /// chronological (newest-session-last); within a session the
    /// log-order is preserved. Files that fail to parse are skipped.
    pub fn collect_decisions(state_root: &Path) -> std::io::Result<Vec<EditorialDecision>> {
        let entries = Self::list(state_root)?;
        let mut out = Vec::new();
        // `list` returned newest-first; reverse so the resulting vec is
        // oldest-first, which makes "trend over time" computations
        // straightforward downstream.
        for (path, _meta) in entries.into_iter().rev() {
            let Ok(body) = std::fs::read_to_string(&path) else {
                continue;
            };
            for line in body.lines() {
                if line.trim().is_empty() {
                    continue;
                }
                if let Ok(RolloutItem::EditorialDecision(d)) =
                    serde_json::from_str::<RolloutItem>(line)
                {
                    out.push(d);
                }
            }
        }
        Ok(out)
    }

    /// List rollout files under `state_root`, newest first. Each entry
    /// is `(path, parsed-meta)`. Files that fail to parse (no meta
    /// line) are silently skipped.
    pub fn list(state_root: &Path) -> std::io::Result<Vec<(PathBuf, SessionMeta)>> {
        let sessions_dir = state_root.join("sessions");
        if !sessions_dir.exists() {
            return Ok(Vec::new());
        }
        let mut paths: Vec<PathBuf> = Vec::new();
        walk_jsonl(&sessions_dir, &mut paths)?;
        let mut out: Vec<(PathBuf, SessionMeta)> = paths
            .into_iter()
            .filter_map(|p| {
                std::fs::read_to_string(&p)
                    .ok()
                    .and_then(|body| body.lines().next().map(str::to_string))
                    .and_then(|first| serde_json::from_str::<RolloutItem>(&first).ok())
                    .and_then(|item| match item {
                        RolloutItem::SessionMeta(m) => Some((p, m)),
                        _ => None,
                    })
            })
            .collect();
        out.sort_by(|a, b| b.1.started_at.cmp(&a.1.started_at));
        Ok(out)
    }

    /// Append a message line. Fire-and-forget — disk I/O happens on the
    /// writer task. If the channel is full we drop the item with a warn
    /// rather than block the agent loop (unlikely at 256 buffer size).
    pub fn record_message(&self, m: Message) {
        self.send(WriterCmd::Append(RolloutItem::Message(m)));
    }

    /// Mark a compaction event in the log. Caller follows with a
    /// `record_message` for the summary.
    pub fn record_compaction(&self, replaced_messages: usize) {
        self.send(WriterCmd::Append(RolloutItem::Compaction {
            replaced_messages,
        }));
    }

    /// Append an editorial decision (#149). Called by `Session` after
    /// the user replies to the approval modal. Fire-and-forget on the
    /// writer task.
    pub fn record_decision(
        &self,
        tool: String,
        args_summary: String,
        approval_keys: Vec<crate::tool::ApprovalKey>,
        retry_reason: Option<String>,
        decision: String,
    ) {
        self.send(WriterCmd::Append(RolloutItem::EditorialDecision(
            EditorialDecision {
                tool,
                args_summary,
                approval_keys,
                retry_reason,
                decision,
                timestamp: Utc::now(),
            },
        )));
    }

    fn send(&self, cmd: WriterCmd) {
        // Unbounded channel — only fails when the receiver has been
        // dropped (writer task exited). In that case logging is over;
        // warn once but don't try to recover.
        if let Err(e) = self.tx.send(cmd) {
            warn!(error = %e, "rollout: writer task is gone; dropping item");
        }
    }

    /// Flush pending writes and wait for ack. Used at shutdown so the
    /// log on disk reflects the full session.
    pub async fn flush(&self) -> std::io::Result<()> {
        let (ack, rx) = oneshot::channel();
        if self.tx.send(WriterCmd::Flush(ack)).is_err() {
            return Ok(()); // writer already gone — nothing to flush
        }
        rx.await.unwrap_or(Ok(()))
    }

    /// Tell the writer task to drain and exit.
    pub async fn shutdown(&self) {
        let _ = self.tx.send(WriterCmd::Shutdown);
    }

    /// Path of the JSONL log on disk.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Session metadata written to the log header.
    pub fn meta(&self) -> &SessionMeta {
        &self.meta
    }
}

async fn writer_loop(
    mut rx: mpsc::UnboundedReceiver<WriterCmd>,
    mut file: std::fs::File,
    path: PathBuf,
) {
    while let Some(cmd) = rx.recv().await {
        match cmd {
            WriterCmd::Append(item) => {
                if let Err(e) = write_line(&mut file, &item) {
                    warn!(path = %path.display(), error = %e, "rollout: write failed");
                }
            }
            WriterCmd::Flush(ack) => {
                let res = file.flush();
                let _ = ack.send(res);
            }
            WriterCmd::Shutdown => {
                let _ = file.flush();
                break;
            }
        }
    }
}

fn write_line<W: std::io::Write>(w: &mut W, item: &RolloutItem) -> std::io::Result<()> {
    serde_json::to_writer(&mut *w, item)?;
    w.write_all(b"\n")?;
    w.flush()
}

fn session_path(state_root: &Path, ts: DateTime<Utc>, id: &str) -> PathBuf {
    let date = ts.format("%Y/%m/%d").to_string();
    let stamp = ts.format("%Y-%m-%dT%H-%M-%S").to_string();
    state_root
        .join("sessions")
        .join(date)
        .join(format!("rollout-{stamp}-{id}.jsonl"))
}

fn generate_session_id(ts: DateTime<Utc>) -> String {
    // 12 hex chars from (timestamp_nanos ^ pid). Good enough — codex uses
    // a UUIDv7, but uuid isn't a workspace dep yet and the only collision
    // risk is two sessions started in the same nanosecond on the same pid,
    // which doesn't happen.
    let nanos = ts.timestamp_nanos_opt().unwrap_or(0) as u64;
    let pid = std::process::id() as u64;
    let mixed = nanos.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(pid);
    format!("{mixed:012x}").chars().take(12).collect()
}

fn sanitize_resume_messages(messages: Vec<Message>) -> Vec<Message> {
    let mut out = Vec::with_capacity(messages.len());
    let mut i = 0;

    while i < messages.len() {
        let message = messages[i].clone();
        match message.role {
            Role::Assistant => {
                let tool_use_ids = tool_use_ids(&message);
                let followed_by_results = messages
                    .get(i + 1)
                    .is_some_and(|next| tool_results_cover(next, &tool_use_ids));
                if tool_use_ids.is_empty() || followed_by_results {
                    out.push(message);
                    i += 1;
                    continue;
                }

                let content: Vec<ContentBlock> = message
                    .content
                    .into_iter()
                    .filter(|block| !matches!(block, ContentBlock::ToolUse { .. }))
                    .collect();
                warn!(
                    count = tool_use_ids.len(),
                    "rollout: stripping orphaned assistant tool_use block(s) during resume"
                );
                if !content.is_empty() {
                    out.push(Message {
                        role: Role::Assistant,
                        content,
                    });
                }
            }
            Role::User if has_tool_result(&message) => {
                if previous_assistant_matches_tool_results(&out, &message) {
                    out.push(message);
                    i += 1;
                    continue;
                }

                let content: Vec<ContentBlock> = message
                    .content
                    .into_iter()
                    .filter(|block| !matches!(block, ContentBlock::ToolResult { .. }))
                    .collect();
                warn!("rollout: stripping orphaned user tool_result block(s) during resume");
                if !content.is_empty() {
                    out.push(Message {
                        role: Role::User,
                        content,
                    });
                }
            }
            _ => out.push(message),
        }
        i += 1;
    }

    out
}

fn previous_assistant_matches_tool_results(out: &[Message], message: &Message) -> bool {
    let Some(previous) = out.last() else {
        return false;
    };
    let tool_use_ids = tool_use_ids(previous);
    !tool_use_ids.is_empty() && tool_results_cover(message, &tool_use_ids)
}

fn tool_use_ids(message: &Message) -> HashSet<String> {
    if !matches!(message.role, Role::Assistant) {
        return HashSet::new();
    }
    message
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::ToolUse { id, .. } => Some(id.clone()),
            _ => None,
        })
        .collect()
}

fn tool_result_ids(message: &Message) -> HashSet<&str> {
    if !matches!(message.role, Role::User) {
        return HashSet::new();
    }
    message
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::ToolResult { tool_use_id, .. } => Some(tool_use_id.as_str()),
            _ => None,
        })
        .collect()
}

fn has_tool_result(message: &Message) -> bool {
    message
        .content
        .iter()
        .any(|block| matches!(block, ContentBlock::ToolResult { .. }))
}

fn tool_results_cover(message: &Message, tool_use_ids: &HashSet<String>) -> bool {
    let result_ids = tool_result_ids(message);
    tool_use_ids
        .iter()
        .all(|tool_use_id| result_ids.contains(tool_use_id.as_str()))
}

fn walk_jsonl(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            walk_jsonl(&path, out)?;
        } else if path.extension().and_then(|s| s.to_str()) == Some("jsonl") {
            out.push(path);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anthropic::Message;

    #[tokio::test]
    async fn create_writes_meta_line() {
        let dir = tempfile::tempdir().unwrap();
        let rec = Recorder::create(
            dir.path(),
            PathBuf::from("/tmp/proj"),
            "claude-opus-4-7".into(),
        )
        .unwrap();
        rec.flush().await.unwrap();
        let body = std::fs::read_to_string(rec.path()).unwrap();
        let first = body.lines().next().unwrap();
        let parsed: RolloutItem = serde_json::from_str(first).unwrap();
        match parsed {
            RolloutItem::SessionMeta(m) => {
                assert_eq!(m.model, "claude-opus-4-7");
                assert_eq!(m.project_root, PathBuf::from("/tmp/proj"));
                assert!(!m.id.is_empty());
            }
            _ => panic!("first line must be SessionMeta"),
        }
    }

    #[tokio::test]
    async fn record_message_appends_line() {
        let dir = tempfile::tempdir().unwrap();
        let rec = Recorder::create(dir.path(), PathBuf::from("/tmp/proj"), "m".into()).unwrap();
        rec.record_message(Message::user_text("hello"));
        rec.record_message(Message::user_text("world"));
        rec.flush().await.unwrap();

        let body = std::fs::read_to_string(rec.path()).unwrap();
        let lines: Vec<&str> = body.lines().collect();
        assert_eq!(lines.len(), 3, "meta + two messages");
        let m1: RolloutItem = serde_json::from_str(lines[1]).unwrap();
        assert!(matches!(m1, RolloutItem::Message(_)));
    }

    #[tokio::test]
    async fn resume_reconstructs_history() {
        let dir = tempfile::tempdir().unwrap();
        let path = {
            let rec = Recorder::create(dir.path(), PathBuf::from("/tmp/proj"), "m".into()).unwrap();
            rec.record_message(Message::user_text("first"));
            rec.record_message(Message::user_text("second"));
            rec.flush().await.unwrap();
            rec.path().to_path_buf()
        };

        let (meta, messages) = Recorder::resume(&path).unwrap();
        assert_eq!(meta.model, "m");
        assert_eq!(messages.len(), 2);
    }

    #[tokio::test]
    async fn resume_preserves_valid_tool_use_result_pair() {
        let dir = tempfile::tempdir().unwrap();
        let path = {
            let rec = Recorder::create(dir.path(), PathBuf::from("/tmp/proj"), "m".into()).unwrap();
            rec.record_message(Message {
                role: Role::Assistant,
                content: vec![ContentBlock::ToolUse {
                    id: "toolu_1".into(),
                    name: "bash".into(),
                    input: serde_json::json!({ "command": "true" }),
                }],
            });
            rec.record_message(Message {
                role: Role::User,
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "toolu_1".into(),
                    content: crate::anthropic::tool_result::text("ok"),
                    is_error: None,
                }],
            });
            rec.flush().await.unwrap();
            rec.path().to_path_buf()
        };

        let (_meta, messages) = Recorder::resume(&path).unwrap();
        assert_eq!(messages.len(), 2);
        assert!(matches!(
            messages[0].content.first(),
            Some(ContentBlock::ToolUse { id, .. }) if id == "toolu_1"
        ));
        assert!(matches!(
            messages[1].content.first(),
            Some(ContentBlock::ToolResult { tool_use_id, .. }) if tool_use_id == "toolu_1"
        ));
    }

    #[tokio::test]
    async fn resume_strips_orphaned_tool_use_blocks() {
        let dir = tempfile::tempdir().unwrap();
        let path = {
            let rec = Recorder::create(dir.path(), PathBuf::from("/tmp/proj"), "m".into()).unwrap();
            rec.record_message(Message {
                role: Role::Assistant,
                content: vec![
                    ContentBlock::text("I'll wait, then poll the render."),
                    ContentBlock::ToolUse {
                        id: "toolu_orphan".into(),
                        name: "bash".into(),
                        input: serde_json::json!({ "command": "sleep 20" }),
                    },
                ],
            });
            rec.record_message(Message::user_text("continue"));
            rec.flush().await.unwrap();
            rec.path().to_path_buf()
        };

        let (_meta, messages) = Recorder::resume(&path).unwrap();
        assert_eq!(messages.len(), 2);
        assert!(
            messages[0]
                .content
                .iter()
                .all(|block| { !matches!(block, ContentBlock::ToolUse { .. }) })
        );
        assert!(matches!(
            messages[0].content.first(),
            Some(ContentBlock::Text { text, .. }) if text.contains("poll the render")
        ));
    }

    #[tokio::test]
    async fn resume_drops_pre_compaction_history() {
        let dir = tempfile::tempdir().unwrap();
        let path = {
            let rec = Recorder::create(dir.path(), PathBuf::from("/tmp/proj"), "m".into()).unwrap();
            rec.record_message(Message::user_text("old-1"));
            rec.record_message(Message::user_text("old-2"));
            rec.record_compaction(2);
            rec.record_message(Message::user_text("[summary] ..."));
            rec.record_message(Message::user_text("post-compact"));
            rec.flush().await.unwrap();
            rec.path().to_path_buf()
        };

        let (_meta, messages) = Recorder::resume(&path).unwrap();
        assert_eq!(messages.len(), 2, "summary + 1 post-compact message");
    }

    #[tokio::test]
    async fn list_returns_newest_first() {
        let dir = tempfile::tempdir().unwrap();
        let _r1 = Recorder::create(dir.path(), PathBuf::from("/a"), "m".into()).unwrap();
        // Force a different started_at so ordering is deterministic.
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        let r2 = Recorder::create(dir.path(), PathBuf::from("/b"), "m".into()).unwrap();
        r2.flush().await.unwrap();

        let entries = Recorder::list(dir.path()).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].1.project_root, PathBuf::from("/b"));
    }

    #[tokio::test]
    async fn record_decision_appends_typed_line() {
        let dir = tempfile::tempdir().unwrap();
        let rec = Recorder::create(dir.path(), PathBuf::from("/proj"), "m".into()).unwrap();
        rec.record_decision(
            "apply_edl".into(),
            "3 ops, kind=hook".into(),
            vec![crate::tool::ApprovalKey::new("apply_edl", "edl:abc")],
            None,
            "Allow".into(),
        );
        rec.flush().await.unwrap();
        let body = std::fs::read_to_string(rec.path()).unwrap();
        let line = body
            .lines()
            .find(|l| l.contains("editorial_decision"))
            .expect("decision line on disk");
        let parsed: RolloutItem = serde_json::from_str(line).unwrap();
        match parsed {
            RolloutItem::EditorialDecision(d) => {
                assert_eq!(d.tool, "apply_edl");
                assert_eq!(d.decision, "Allow");
                assert!(d.args_summary.contains("kind=hook"));
                assert_eq!(d.approval_keys[0].operation, "edl:abc");
            }
            _ => panic!("expected EditorialDecision"),
        }
    }

    /// Regression for the bounded-channel data loss caught by the
    /// stress suite (`eval::stress::large_rollout_resume`). With the
    /// old 256-slot bounded channel, a tight 5K-message loop dropped
    /// ~95% of writes via `try_send`. Unbounded channel must replay
    /// every message intact.
    #[tokio::test]
    async fn writes_are_lossless_under_burst() {
        let dir = tempfile::tempdir().unwrap();
        let rec = Recorder::create(dir.path(), PathBuf::from("/p"), "m".into()).unwrap();
        const N: usize = 2_000;
        for i in 0..N {
            rec.record_message(crate::anthropic::Message::user_text(format!("msg-{i}")));
        }
        rec.flush().await.unwrap();
        let (_meta, msgs) = Recorder::resume(rec.path()).unwrap();
        assert_eq!(msgs.len(), N, "every burst-written message must survive");
    }

    #[tokio::test]
    async fn collect_decisions_walks_all_sessions() {
        let dir = tempfile::tempdir().unwrap();
        let r1 = Recorder::create(dir.path(), PathBuf::from("/p"), "m".into()).unwrap();
        r1.record_decision(
            "apply_edl".into(),
            "op A".into(),
            vec![],
            None,
            "Allow".into(),
        );
        r1.flush().await.unwrap();
        // Force second session a tick later for stable ordering.
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        let r2 = Recorder::create(dir.path(), PathBuf::from("/p"), "m".into()).unwrap();
        r2.record_decision(
            "apply_edl".into(),
            "op B".into(),
            vec![],
            None,
            "Deny".into(),
        );
        r2.flush().await.unwrap();

        let decisions = Recorder::collect_decisions(dir.path()).unwrap();
        assert_eq!(decisions.len(), 2);
        // collect_decisions returns oldest-session-first.
        assert_eq!(decisions[0].args_summary, "op A");
        assert_eq!(decisions[1].args_summary, "op B");
    }

    #[tokio::test]
    async fn resume_skips_unparseable_lines() {
        let dir = tempfile::tempdir().unwrap();
        let rec = Recorder::create(dir.path(), PathBuf::from("/tmp"), "m".into()).unwrap();
        rec.record_message(Message::user_text("ok"));
        rec.flush().await.unwrap();
        // Append a garbage line.
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(rec.path())
            .unwrap();
        f.write_all(b"this-is-not-json\n").unwrap();
        f.flush().unwrap();
        drop(f);

        let (_meta, messages) = Recorder::resume(rec.path()).unwrap();
        assert_eq!(messages.len(), 1);
    }
}
