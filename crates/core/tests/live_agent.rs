//! Live integration test: real Anthropic API + real `bash` tool.
//!
//! `#[ignore]` by default because it (a) requires `ANTHROPIC_API_KEY` and
//! (b) costs ~$0.001/run on Haiku. CI doesn't run it; you opt in via:
//!
//! ```text
//! cargo test -p awidat-core --test live_agent -- --ignored --nocapture
//! ```
//!
//! Asserts:
//! 1. The agent calls `bash` (we don't accept "I'd rather use a different
//!    approach" — the prompt is explicit).
//! 2. The bash output contains a known sentinel.
//! 3. The agent's final reply mentions the sentinel back to the user.

use std::sync::Arc;
use std::time::Duration;

use awidat_core::anthropic::{Client, ClientConfig, models};
use awidat_core::tools::bash::BashTool;
use awidat_core::{Session, SessionEvent, ToolRegistry};
use tokio_util::sync::CancellationToken;

const SENTINEL: &str = "awidat-live-test-sentinel-7f2a3b9c";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "live API call; requires ANTHROPIC_API_KEY and costs ~$0.001"]
async fn agent_uses_bash_to_echo_sentinel_and_reports_back() {
    let client = match Client::from_env_or_keychain(ClientConfig::default()) {
        Ok(c) => c,
        Err(e) => panic!("test requires ANTHROPIC_API_KEY in env or keychain: {e}"),
    };

    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(BashTool));

    let session = Arc::new(Session::new(
        client,
        registry,
        models::HAIKU,
        Some(
            "You are a helpful test agent. The user will give you a one-shot \
             task. Use the bash tool when asked. Reply concisely."
                .into(),
        ),
        std::env::temp_dir(),
    ));

    let mut events = session.subscribe();
    let cancel = CancellationToken::new();

    let prompt = format!(
        "Use the `bash` tool to run `echo {SENTINEL}` and then tell me, \
         in one short sentence, what the command printed. Do not skip the \
         bash call."
    );
    let session_clone = session.clone();
    let cancel_clone = cancel.clone();
    let turn = tokio::spawn(async move { session_clone.run_turn(prompt, cancel_clone).await });

    let mut tool_called = false;
    let mut tool_output_contains_sentinel = false;
    let mut accumulated_text = String::new();
    let deadline = tokio::time::sleep(Duration::from_secs(60));
    tokio::pin!(deadline);

    loop {
        tokio::select! {
            biased;
            () = &mut deadline => panic!("test timed out after 60s"),
            ev = events.recv() => {
                match ev {
                    Ok(SessionEvent::ToolCallStart { name, .. }) if name == "bash" => {
                        tool_called = true;
                    }
                    Ok(SessionEvent::ToolResult { result: Ok(out), name, .. }) if name == "bash" => {
                        if out.contains(SENTINEL) {
                            tool_output_contains_sentinel = true;
                        }
                    }
                    Ok(SessionEvent::TextDelta(t)) => accumulated_text.push_str(&t),
                    Ok(SessionEvent::TurnEnd) => break,
                    Ok(SessionEvent::Error(msg)) => panic!("session error: {msg}"),
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }
    let res = turn.await.expect("join");
    assert!(res.is_ok(), "turn returned error: {res:?}");

    assert!(tool_called, "agent never called the bash tool");
    assert!(
        tool_output_contains_sentinel,
        "bash output didn't contain the sentinel — bash failed?"
    );
    assert!(
        accumulated_text.contains(SENTINEL),
        "agent's reply didn't mention the sentinel back; reply was: {accumulated_text}"
    );
}
