// Inline approval card. Replaces the TUI's modal pattern — the user
// sees the proposed action in chat and approves / denies in place.
// Once any of the three buttons is pressed, the backend's pending
// oneshot is consumed; the card stays visible (now Completed) so the
// transcript shows what was decided.

import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { Item } from "../protocol";

type Props = {
  /** The ApprovalRequest item this card represents. */
  item: Extract<Item, { kind: "approval_request" }>;
};

export function ApprovalCard({ item }: Props) {
  const [decision, setDecision] = useState<
    "allow" | "allow_for_session" | "deny" | null
  >(null);
  const [error, setError] = useState<string | null>(null);

  async function respond(d: "allow" | "allow_for_session" | "deny") {
    if (decision !== null) return;
    setDecision(d);
    try {
      await invoke("respond_approval", { callId: item.id, decision: d });
    } catch (err) {
      setError(String(err));
      setDecision(null);
    }
  }

  return (
    <article className="item item-approval">
      <div className="item-meta">
        approval · <code>{item.tool_name}</code>
      </div>
      <div className="approval-summary">{item.args_summary}</div>
      <div className="approval-actions">
        <button
          onClick={() => respond("allow")}
          disabled={decision !== null}
          className={decision === "allow" ? "chosen" : ""}
        >
          Allow
        </button>
        <button
          onClick={() => respond("allow_for_session")}
          disabled={decision !== null}
          className={decision === "allow_for_session" ? "chosen" : ""}
        >
          Allow for session
        </button>
        <button
          onClick={() => respond("deny")}
          disabled={decision !== null}
          className={`deny ${decision === "deny" ? "chosen" : ""}`}
        >
          Deny
        </button>
      </div>
      {error && <div className="result-err">{error}</div>}
    </article>
  );
}
