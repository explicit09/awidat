// Floating toolbar shown above the timeline when a proposal is
// active. Three actions: Accept (Enter), Reject (Esc), and a
// "Show EDL" toggle that reveals the round-trippable EDL text in
// a small popover so users can see exactly what's being proposed.
//
// Keyboard shortcuts are window-level but gated against text-input
// focus, mirroring MediaPane's spacebar handler — typing in the
// composer must not accidentally accept a pending proposal.

import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useProposalStore } from "./proposal";

export function ProposalActions() {
  const proposal = useProposalStore((s) => s.active);
  const [showEdl, setShowEdl] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if (!proposal) return;
      // Don't capture when the user is typing in the composer or
      // a transcript input.
      const tag = (e.target as HTMLElement | null)?.tagName?.toLowerCase();
      if (tag === "textarea" || tag === "input") return;
      if (e.key === "Enter" && !e.metaKey && !e.ctrlKey && !e.shiftKey) {
        e.preventDefault();
        accept();
      } else if (e.key === "Escape") {
        e.preventDefault();
        reject();
      }
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [proposal]);

  if (!proposal) return null;

  async function accept() {
    if (!proposal || busy) return;
    setError(null);
    setBusy(true);
    try {
      await invoke("accept_proposal", { callId: proposal.callId });
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  async function reject() {
    if (!proposal || busy) return;
    setError(null);
    setBusy(true);
    try {
      await invoke("reject_proposal", { callId: proposal.callId });
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  const sourceLabel =
    proposal.source.source === "agent"
      ? `agent · ${proposal.source.tool_name}`
      : "your edit";

  return (
    <div className="proposal-actions">
      <div className="proposal-actions-info">
        <span className="proposal-actions-label">proposal</span>
        <span className="proposal-actions-source">{sourceLabel}</span>
        <span className="proposal-actions-summary">{proposal.summary}</span>
      </div>
      <button
        className="proposal-actions-secondary"
        onClick={() => setShowEdl((v) => !v)}
        title="Show the round-trippable EDL text"
      >
        {showEdl ? "Hide EDL" : "Show EDL"}
      </button>
      <button
        className="proposal-actions-secondary proposal-actions-reject"
        onClick={reject}
        disabled={busy}
        title="Reject (Esc)"
      >
        Reject
      </button>
      <button
        className="proposal-actions-primary"
        onClick={accept}
        disabled={busy}
        title="Accept (Enter)"
      >
        Accept ⏎
      </button>
      {error && <div className="proposal-actions-error">{error}</div>}
      {showEdl && (
        <div className="proposal-edl-popover">
          <pre>{proposal.edlText || "(no EDL text on adjusted proposals)"}</pre>
        </div>
      )}
    </div>
  );
}
