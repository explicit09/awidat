// Inline prompt for `request_user_input`. Free-form input or a list
// of options, depending on what the agent supplied.

import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { Item } from "../protocol";

type Props = {
  /** The AwaitingUserInput item this card represents. */
  item: Extract<Item, { kind: "awaiting_user_input" }>;
};

export function UserInputCard({ item }: Props) {
  const [reply, setReply] = useState("");
  const [submitted, setSubmitted] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function send(value: string) {
    if (submitted) return;
    setSubmitted(true);
    try {
      await invoke("respond_user_input", {
        callId: item.id,
        reply: value,
      });
    } catch (err) {
      setError(String(err));
      setSubmitted(false);
    }
  }

  return (
    <article className="item item-input glass-content">
      <div className="item-meta">
        awaiting input
        <span className="input-chip">input</span>
      </div>
      <div className="item-body">{item.question}</div>
      {item.options ? (
        <div className="input-options">
          {item.options.map((opt, i) => (
            <button
              key={`${i}-${opt}`}
              className="glass-ghost"
              onClick={() => send(opt)}
              disabled={submitted}
            >
              {opt}
            </button>
          ))}
        </div>
      ) : (
        <form
          className="input-form"
          onSubmit={(e) => {
            e.preventDefault();
            const v = reply.trim();
            if (v) send(v);
          }}
        >
          <input
            type="text"
            value={reply}
            onChange={(e) => setReply(e.target.value)}
            disabled={submitted}
            autoFocus
          />
          <button
            type="submit"
            className="glass-cta"
            disabled={submitted || !reply.trim()}
          >
            Send
          </button>
        </form>
      )}
      {error && <div className="result-err">{error}</div>}
    </article>
  );
}
