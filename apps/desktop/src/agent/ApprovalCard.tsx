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
  const capability = readCapabilityMetadata(item.capability_metadata);

  return (
    <article className="item item-approval">
      <div className="item-meta">
        approval · <code>{item.tool_name}</code>
      </div>
      <div className="approval-summary">{item.args_summary}</div>
      {capability && (
        <dl className="approval-capabilities">
          <div>
            <dt>Graph</dt>
            <dd>{capability.graph_mutates ? "mutates" : "read-only"}</dd>
          </div>
          <div>
            <dt>Preview</dt>
            <dd>{formatSupport(capability.preview_supported)}</dd>
          </div>
          <div>
            <dt>Export</dt>
            <dd>{formatSupport(capability.export_supported)}</dd>
          </div>
          {capability.side_effects.length > 0 && (
            <div>
              <dt>Effects</dt>
              <dd>{capability.side_effects.join(", ")}</dd>
            </div>
          )}
        </dl>
      )}
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

type SupportLevel = "supported" | "not_supported" | "unknown";

type CapabilityMetadata = {
  graph_mutates: boolean;
  preview_supported: SupportLevel;
  export_supported: SupportLevel;
  side_effects: string[];
};

function readCapabilityMetadata(value: unknown): CapabilityMetadata | null {
  if (!value || typeof value !== "object") return null;
  const record = value as Record<string, unknown>;
  if (
    typeof record.graph_mutates !== "boolean" ||
    !isSupportLevel(record.preview_supported) ||
    !isSupportLevel(record.export_supported)
  ) {
    return null;
  }
  return {
    graph_mutates: record.graph_mutates,
    preview_supported: record.preview_supported,
    export_supported: record.export_supported,
    side_effects: Array.isArray(record.side_effects)
      ? record.side_effects.filter((entry): entry is string => typeof entry === "string")
      : [],
  };
}

function isSupportLevel(value: unknown): value is SupportLevel {
  return value === "supported" || value === "not_supported" || value === "unknown";
}

function formatSupport(value: SupportLevel): string {
  switch (value) {
    case "supported":
      return "supported";
    case "not_supported":
      return "not supported";
    case "unknown":
      return "unknown";
  }
}
