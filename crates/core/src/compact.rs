//! Conversation compaction at iteration-cap pressure.
//!
//! The Codex / Claude Code pattern, ported. When a turn approaches
//! the inner-loop iteration cap, instead of just warning the agent
//! ("you have 12 iterations left, wrap up") we run a separate
//! Haiku call that summarizes the conversation so far into a
//! handoff text. We replace history with `[original user prompt,
//! "<COMPACTED:>" + summary, latest tool_result if any]`. The
//! tier-1 cache prefix (system + tools) survives — only the per-turn
//! tier-2 prefix invalidates. Headroom restored.
//!
//! See `harnesses/codex/codex-rs/core/templates/compact/{prompt,summary_prefix}.md`
//! for the upstream prompt this is based on.

use crate::anthropic::{
    Client, ContentBlock, Message, MessagesRequest, Role, StopReason, StreamEvent,
};
use futures::StreamExt as _;
use thiserror::Error;
use tokio_util::sync::CancellationToken;

/// Errors compaction can return. The caller surfaces them as a
/// `SessionEvent::Error`; compaction failures are recoverable —
/// the loop just continues without compacting.
#[derive(Debug, Error)]
pub enum CompactError {
    /// User cancelled mid-summary.
    #[error("compaction cancelled")]
    Cancelled,
    /// Underlying API error from `Client::messages_stream`.
    #[error("compaction client error: {0}")]
    Client(#[from] crate::anthropic::ClientError),
}

/// The Haiku-cheap model id we summarize with. Hardcoded — swap if
/// it ever stops being cheapest. Compaction runs once per
/// trip-trigger so even Sonnet-priced compaction is fine; Haiku
/// just makes it negligible.
const COMPACTION_MODEL: &str = "claude-haiku-4-5-20251001";

/// Maximum tokens the compaction call may produce. Keep tight — the
/// whole point is to shrink history, not write an essay.
const COMPACTION_MAX_TOKENS: u32 = 2048;

/// System prompt for the compaction call. Lifted (and trimmed) from
/// `harnesses/codex/codex-rs/core/templates/compact/prompt.md`.
pub const COMPACTION_PROMPT: &str = "\
You are performing a CONTEXT CHECKPOINT COMPACTION. The conversation \
that follows is between a video-editing agent and a user; the agent \
is mid-task and is running out of sampling iterations in this turn. \
Create a handoff summary for another LLM (Claude) that will resume \
the task in the same turn.\n\n\
Include:\n\
- The user's original request, verbatim or paraphrased tightly.\n\
- Current progress and the editorial decisions already made.\n\
- Findings from indexer queries (find_moment / find_beat / read_index \
results) — keep concrete: timestamps, moment_ids, transcript phrases, \
clip names.\n\
- Any apply_edl ops that landed (with their committed status), and \
any that failed (with the error).\n\
- What remains to be done — clear next steps in priority order.\n\
- Any constraints, user preferences, or ground rules surfaced.\n\n\
Be concise, structured (use bullets), and focused on helping the \
next LLM seamlessly continue the work without redoing tool calls.\
";

/// Banner the compacted-summary message starts with so the resumed
/// model recognizes the handoff.
pub const COMPACTION_BANNER: &str = "[awidat-compaction] \
Another model started this task and produced a handoff summary. \
Use this to continue without re-running tool calls. Here's the summary:";

/// Run a one-shot Haiku call against `messages` with the compaction
/// prompt. Returns the summary text the next iteration's history
/// should carry.
///
/// Tools list is intentionally NOT passed — the compaction call is
/// pure summarization, no actions.
pub async fn compact_history(
    client: &Client,
    history: &[Message],
    cancel: &CancellationToken,
) -> Result<String, CompactError> {
    // Filter the history to user/assistant text — compaction doesn't
    // need the full tool_use/tool_result blocks; those bloat the
    // request without adding signal beyond what the assistant text
    // around them already captures.
    let trimmed: Vec<Message> = history
        .iter()
        .filter_map(|m| {
            let mut blocks = Vec::new();
            for b in &m.content {
                match b {
                    ContentBlock::Text { text, .. } if !text.trim().is_empty() => {
                        blocks.push(ContentBlock::text(text.clone()));
                    }
                    ContentBlock::ToolResult { content, .. } => {
                        // Inline a brief truncated text-shape of tool
                        // results so the summary call sees what the
                        // tools returned without a multi-block array.
                        if let Some(s) = content.as_str() {
                            blocks.push(ContentBlock::text(truncate_for_compact(s)));
                        }
                    }
                    _ => {}
                }
            }
            if blocks.is_empty() {
                None
            } else {
                Some(Message {
                    role: m.role,
                    content: blocks,
                })
            }
        })
        .collect();

    if trimmed.is_empty() {
        return Ok(String::new());
    }

    let req = MessagesRequest::new(COMPACTION_MODEL, trimmed)
        .with_max_tokens(COMPACTION_MAX_TOKENS)
        .with_system(COMPACTION_PROMPT);

    // No tools, no tool_choice — pure text summarization.
    let stream = client.messages_stream(req);
    tokio::pin!(stream);

    let mut summary = String::new();
    let mut stop_reason: Option<StopReason> = None;
    loop {
        tokio::select! {
            biased;
            () = cancel.cancelled() => return Err(CompactError::Cancelled),
            next = stream.next() => {
                let Some(item) = next else { break };
                let event = item?;
                match event {
                    StreamEvent::TextDelta(s) => summary.push_str(&s),
                    StreamEvent::Done { stop_reason: sr, .. } => {
                        stop_reason = sr;
                        break;
                    }
                    _ => {} // ignore tool-use blocks etc. — none expected
                }
            }
        }
    }
    let _ = stop_reason; // future telemetry
    Ok(summary.trim().to_string())
}

/// Replace `history` with the compacted form: keep the first user
/// message (the original prompt — preserves cache tier-1 hits when
/// possible) + a single user-role message containing the banner +
/// summary. Returns the new history.
///
/// We avoid keeping the latest assistant or tool_result; the summary
/// is supposed to capture what those said, and keeping them risks
/// the model re-running tools in the resumed turn.
pub fn replace_history_with_summary(history: Vec<Message>, summary: String) -> Vec<Message> {
    let original_user = history
        .into_iter()
        .find(|m| m.role == Role::User)
        .unwrap_or_else(|| Message::user_text("(no original user message recovered)"));
    let handoff = Message::user_text(format!("{COMPACTION_BANNER}\n\n{summary}"));
    vec![original_user, handoff]
}

fn truncate_for_compact(s: &str) -> String {
    const CAP: usize = 600;
    if s.chars().count() <= CAP {
        s.to_string()
    } else {
        let head: String = s.chars().take(CAP / 2).collect();
        let tail: String = s.chars().rev().take(CAP / 2).collect::<String>();
        let tail: String = tail.chars().rev().collect();
        format!("{head}…[truncated]…{tail}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_for_compact_keeps_short_strings() {
        let s = "hello world";
        assert_eq!(truncate_for_compact(s), s);
    }

    #[test]
    fn truncate_for_compact_shrinks_long_strings_with_ellipsis() {
        let s = "a".repeat(2000);
        let out = truncate_for_compact(&s);
        assert!(out.contains("[truncated]"));
        assert!(out.chars().count() < s.chars().count());
    }

    #[test]
    fn replace_history_keeps_original_user_message() {
        let history = vec![
            Message::user_text("delete the bravo clip"),
            Message::assistant_text("calling find_moment..."),
            Message::assistant_text("calling apply_edl..."),
        ];
        let new = replace_history_with_summary(history, "summary text".into());
        assert_eq!(new.len(), 2);
        assert_eq!(new[0].role, Role::User);
        // The original prompt should be the first message.
        assert!(matches!(
            &new[0].content[0],
            ContentBlock::Text { text, .. } if text == "delete the bravo clip"
        ));
        // Handoff message is user-role too (so the model sees it as
        // an instruction-level statement, not as content it produced).
        assert_eq!(new[1].role, Role::User);
        assert!(matches!(
            &new[1].content[0],
            ContentBlock::Text { text, .. } if text.contains(COMPACTION_BANNER) && text.contains("summary text")
        ));
    }

    #[test]
    fn replace_history_with_no_user_message_falls_back() {
        // Pathological case — handler shouldn't panic.
        let new = replace_history_with_summary(vec![], "s".into());
        assert_eq!(new.len(), 2);
        assert_eq!(new[0].role, Role::User);
    }
}
