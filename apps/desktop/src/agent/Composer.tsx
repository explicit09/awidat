// Text composer pinned to the bottom of the chat pane. Submits user
// input via `start_turn`. Esc cancels the in-flight turn (also
// available via the Stop button next to the input).

import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useAgentStore } from "./store";
import { composerAuthGateState } from "./composerAuthGate";
import { useAuth } from "../state/auth";

type Props = {
  /** Disabled when no project is loaded (set_project_root not yet called). */
  projectReady: boolean;
};

export function Composer({ projectReady }: Props) {
  const running = useAgentStore((s) => s.running);
  const setRunning = useAgentStore((s) => s.setRunning);
  const setActiveTurnId = useAgentStore((s) => s.setActiveTurnId);
  const setTurnError = useAgentStore((s) => s.setTurnError);
  const authStatus = useAuth((s) => s.status);
  const openAuth = useAuth((s) => s.open);
  const [text, setText] = useState("");
  const inputRef = useRef<HTMLTextAreaElement | null>(null);
  const gate = composerAuthGateState({ projectReady, running, authStatus, text });

  // Auto-focus on mount and when the running flag flips back to false.
  useEffect(() => {
    if (!gate.textareaDisabled) {
      inputRef.current?.focus();
    }
  }, [gate.textareaDisabled]);

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
    if (!trimmed || running || !projectReady || !gate.authReady) return;
    setText("");
    setTurnError(null);
    setRunning(true);
    try {
      const turnId = await invoke<string>("start_turn", { input: trimmed });
      setActiveTurnId(turnId);
    } catch (err) {
      if (String(err).includes("turn is already running")) {
        try {
          await invoke("cancel_turn");
          const turnId = await invoke<string>("start_turn", { input: trimmed });
          setActiveTurnId(turnId);
          return;
        } catch (retryErr) {
          setTurnError(String(retryErr));
          setRunning(false);
          setText(trimmed);
          return;
        }
      }
      setTurnError(String(err));
      setRunning(false);
      setText(trimmed);
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
        title="Command-Enter sends. Esc cancels a running turn."
        placeholder={
          gate.placeholder
        }
        rows={1}
        disabled={gate.textareaDisabled}
      />
      {running ? (
        <button onClick={cancel} className="composer-stop">
          Stop
        </button>
      ) : (
        <button
          onClick={gate.disabledReason ? openAuth : send}
          disabled={gate.sendDisabled}
        >
          {gate.sendLabel}
        </button>
      )}
    </footer>
  );
}
