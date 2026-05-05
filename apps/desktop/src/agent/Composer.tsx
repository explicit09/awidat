// Text composer pinned to the bottom of the chat pane. Submits user
// input via `start_turn`. Esc cancels the in-flight turn (also
// available via the Stop button next to the input).

import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useAgentStore } from "./store";

type Props = {
  /** Disabled when no project is loaded (set_project_root not yet called). */
  projectReady: boolean;
};

export function Composer({ projectReady }: Props) {
  const running = useAgentStore((s) => s.running);
  const setRunning = useAgentStore((s) => s.setRunning);
  const setTurnError = useAgentStore((s) => s.setTurnError);
  const [text, setText] = useState("");
  const inputRef = useRef<HTMLTextAreaElement | null>(null);

  // Auto-focus on mount and when the running flag flips back to false.
  useEffect(() => {
    if (!running && projectReady) {
      inputRef.current?.focus();
    }
  }, [running, projectReady]);

  // Esc cancels the in-flight turn, regardless of where focus is.
  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape" && running) {
        e.preventDefault();
        cancel();
      }
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [running]);

  async function send() {
    const trimmed = text.trim();
    if (!trimmed || running || !projectReady) return;
    setText("");
    setTurnError(null);
    setRunning(true);
    try {
      await invoke("start_turn", { input: trimmed });
    } catch (err) {
      setTurnError(String(err));
      setRunning(false);
    }
  }

  async function cancel() {
    try {
      await invoke("cancel_turn");
    } catch (err) {
      // eslint-disable-next-line no-console
      console.warn("cancel_turn failed", err);
    }
  }

  function onKeyDown(e: React.KeyboardEvent<HTMLTextAreaElement>) {
    // Cmd/Ctrl-Enter submits. Plain Enter inserts a newline so users
    // can compose multi-line prompts without tripping submit.
    if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
      e.preventDefault();
      send();
    }
  }

  return (
    <footer className="composer">
      <textarea
        ref={inputRef}
        value={text}
        onChange={(e) => setText(e.target.value)}
        onKeyDown={onKeyDown}
        placeholder={
          projectReady
            ? "Message awidat — ⌘⏎ to send, Esc to cancel a running turn"
            : "Set a project root first…"
        }
        rows={2}
        disabled={running || !projectReady}
      />
      {running ? (
        <button onClick={cancel} className="composer-stop">
          Stop
        </button>
      ) : (
        <button onClick={send} disabled={!projectReady || !text.trim()}>
          Send
        </button>
      )}
    </footer>
  );
}
