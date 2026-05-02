//! `request_user_input` tool. Lifted from
//! `harnesses/codex/codex-rs/core/src/tools/handlers/request_user_input.rs`
//! (~78 LOC).
//!
//! Suspends the in-flight tool call until the user replies. The handler
//! sends a [`crate::UserInputRequest`] to the runtime's user-input channel
//! (REPL or TUI), then awaits the embedded `oneshot::Sender<String>`. The
//! reply becomes the tool's `tool_result`.
//!
//! If no user-input channel is wired (e.g. one-shot non-interactive
//! invocations), the tool errors `RespondToModel` so the model can route
//! around it.
//!
//! Cancellation: if the turn is cancelled while we're awaiting, the
//! oneshot's sender drops; we surface
//! `RespondToModel("request_user_input was cancelled before receiving a
//! response")`.

use async_trait::async_trait;
use serde::Deserialize;
use tokio::sync::oneshot;

use crate::FunctionCallError;
use crate::anthropic::Tool as ToolSchema;
use crate::session::SessionEvent;
use crate::tool::{ToolContext, ToolHandler, ToolInvocation, ToolOutput, UserInputRequest};

/// The `request_user_input` tool.
pub struct RequestUserInputTool;

#[derive(Debug, Deserialize)]
struct RequestUserInputArgs {
    question: String,
    #[serde(default)]
    options: Option<Vec<String>>,
    #[serde(default)]
    default: Option<String>,
}

#[async_trait]
impl ToolHandler for RequestUserInputTool {
    fn name(&self) -> &'static str {
        "request_user_input"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "request_user_input".into(),
            description: DESCRIPTION.into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "question": {
                        "type": "string",
                        "description": "Concrete one-sentence question. Don't preamble — the user reads only the question."
                    },
                    "options": {
                        "type": "array",
                        "description": "Optional list of choices. If present, the user picks one; the reply will be one of these strings.",
                        "items": { "type": "string" }
                    },
                    "default": {
                        "type": "string",
                        "description": "Optional pre-filled default; the user can accept by hitting enter."
                    }
                },
                "required": ["question"]
            }),
        }
    }

    fn is_mutating(&self, _invocation: &ToolInvocation) -> bool {
        // Suspends the turn; not parallelizable with anything else.
        true
    }

    async fn handle(
        &self,
        invocation: ToolInvocation,
        ctx: ToolContext,
    ) -> Result<ToolOutput, FunctionCallError> {
        let args: RequestUserInputArgs = serde_json::from_value(invocation.args).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "request_user_input: invalid args ({e}). Required: {{ \"question\": <str> }}."
            ))
        })?;

        let Some(tx) = ctx.user_input_tx.as_ref() else {
            return Err(FunctionCallError::RespondToModel(
                "request_user_input: interactive input not available in this runtime. \
                 You're running under a non-interactive session (no TTY); proceed \
                 without asking, or surface the question in your final reply.".into(),
            ));
        };

        let (reply_tx, reply_rx) = oneshot::channel::<String>();
        let request = UserInputRequest {
            call_id: invocation.call_id.clone(),
            question: args.question.clone(),
            options: args.options.clone(),
            default: args.default,
            reply: reply_tx,
        };

        // Surface the prompt to the runtime + emit a SessionEvent so the
        // REPL/TUI knows to render the wait state.
        let _ = ctx.events_tx.send(SessionEvent::AwaitingUserInput {
            call_id: invocation.call_id,
            question: args.question,
            options: args.options,
        });
        if tx.send(request).await.is_err() {
            return Err(FunctionCallError::RespondToModel(
                "request_user_input: runtime input channel closed; \
                 cannot route the question.".into(),
            ));
        }

        match reply_rx.await {
            Ok(response) => Ok(ToolOutput::text(response)),
            Err(_) => Err(FunctionCallError::RespondToModel(
                "request_user_input was cancelled before receiving a response".into(),
            )),
        }
    }
}

const DESCRIPTION: &str = "\
Ask the human a question and wait for a reply. Use sparingly — every call \
suspends the turn until the user types something. Good uses: pick between \
two non-obvious editorial choices (hard cut vs 0.3s dissolve?), confirm a \
destructive operation, request a missing asset path. Bad uses: things you \
can figure out by calling another tool, things the user already told you.\
";

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::{broadcast, mpsc};

    fn ctx_with_input() -> (
        ToolContext,
        broadcast::Receiver<SessionEvent>,
        mpsc::Receiver<UserInputRequest>,
    ) {
        let (events_tx, events_rx) = broadcast::channel(8);
        let (input_tx, input_rx) = mpsc::channel(8);
        (
            ToolContext {
                project_root: std::env::temp_dir(),
                events_tx,
                user_input_tx: Some(input_tx),
                job_manager: awidat_render::JobManager::new(),
            },
            events_rx,
            input_rx,
        )
    }

    fn ctx_no_input() -> ToolContext {
        let (events_tx, _) = broadcast::channel(8);
        ToolContext {
            project_root: std::env::temp_dir(),
            events_tx,
            user_input_tx: None,
            job_manager: awidat_render::JobManager::new(),
        }
    }

    fn invoke(args: serde_json::Value) -> ToolInvocation {
        ToolInvocation {
            call_id: "c1".into(),
            name: "request_user_input".into(),
            args,
        }
    }

    #[tokio::test]
    async fn happy_path_returns_user_reply() {
        let (c, _events_rx, mut input_rx) = ctx_with_input();
        let tool = RequestUserInputTool;

        // Spawn the handler; meanwhile, drive the input channel as the
        // REPL would.
        let handler_fut = tokio::spawn(async move {
            tool.handle(invoke(serde_json::json!({"question": "dissolve?"})), c)
                .await
        });

        let req = input_rx.recv().await.expect("request received");
        assert_eq!(req.question, "dissolve?");
        req.reply.send("yes".into()).expect("send reply");

        let out = handler_fut.await.expect("join").expect("handler ok");
        assert_eq!(out.content, "yes");
    }

    #[tokio::test]
    async fn no_runtime_channel_is_respond_to_model() {
        let c = ctx_no_input();
        let err = RequestUserInputTool
            .handle(invoke(serde_json::json!({"question": "ok?"})), c)
            .await
            .unwrap_err();
        match err {
            FunctionCallError::RespondToModel(msg) => {
                assert!(msg.contains("interactive input not available"));
            }
            other => panic!("want RespondToModel, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn cancelled_when_reply_dropped() {
        let (c, _events_rx, mut input_rx) = ctx_with_input();
        let tool = RequestUserInputTool;

        let handler_fut = tokio::spawn(async move {
            tool.handle(invoke(serde_json::json!({"question": "?"})), c)
                .await
        });

        let req = input_rx.recv().await.expect("request");
        drop(req.reply); // simulate REPL cancelling the prompt

        let err = handler_fut.await.expect("join").unwrap_err();
        match err {
            FunctionCallError::RespondToModel(msg) => {
                assert!(msg.contains("cancelled before receiving a response"));
            }
            other => panic!("want RespondToModel, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn missing_question_is_respond_to_model() {
        let (c, _, _) = ctx_with_input();
        let err = RequestUserInputTool
            .handle(invoke(serde_json::json!({})), c)
            .await
            .unwrap_err();
        assert!(matches!(err, FunctionCallError::RespondToModel(_)));
    }

    #[tokio::test]
    async fn awaiting_event_is_emitted() {
        let (c, mut events_rx, mut input_rx) = ctx_with_input();
        let tool = RequestUserInputTool;

        let _handler_fut = tokio::spawn(async move {
            tool.handle(
                invoke(serde_json::json!({
                    "question": "hard cut?",
                    "options": ["yes", "no"]
                })),
                c,
            )
            .await
        });

        // Event arrives even before the runtime accepts the channel send.
        let ev = events_rx.recv().await.expect("event");
        match ev {
            SessionEvent::AwaitingUserInput { question, options, .. } => {
                assert_eq!(question, "hard cut?");
                assert_eq!(options, Some(vec!["yes".into(), "no".into()]));
            }
            other => panic!("want AwaitingUserInput, got {other:?}"),
        }
        // Drain the request so the spawned task can finish on drop.
        let _ = input_rx.recv().await;
    }
}
