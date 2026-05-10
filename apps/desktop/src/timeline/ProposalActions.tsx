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
import { Tooltip } from "../components/ui/Tooltip";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "../components/ui/Popover";
import { MENU_COMMANDS, onMenuCommand } from "../app/menuCommands";

export function ProposalActions() {
  const proposal = useProposalStore((s) => s.active);
  const [edlOpen, setEdlOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    setEdlOpen(false);
    setBusy(false);
    setError(null);
  }, [proposal?.callId]);

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

  useEffect(() => {
    return onMenuCommand((id) => {
      if (id === MENU_COMMANDS.ACCEPT_PROPOSAL) {
        void accept();
      } else if (id === MENU_COMMANDS.REJECT_PROPOSAL) {
        void reject();
      }
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [proposal, busy]);

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
    // The `key={proposal.callId}` forces a fresh mount on each new
    // proposal so the slide-in animation re-runs (CSS animations
    // only fire on element creation, not on render).
    <div key={proposal.callId} className="proposal-actions">
      <div className="proposal-actions-info">
        <span className="proposal-actions-label">proposal</span>
        <span className="proposal-actions-source">{sourceLabel}</span>
        <span className="proposal-actions-summary">{proposal.summary}</span>
        {proposal.revision > 0 ? (
          <span className="proposal-actions-revision">
            adjusted ×{proposal.revision}
          </span>
        ) : null}
      </div>
      <Popover open={edlOpen} onOpenChange={setEdlOpen}>
        <Tooltip content="Show the round-trippable EDL text">
          <PopoverTrigger asChild>
            <button className="proposal-actions-secondary">
              {edlOpen ? "Hide EDL" : "Show EDL"}
            </button>
          </PopoverTrigger>
        </Tooltip>
        <PopoverContent className="proposal-edl-popover" align="end">
          <header className="proposal-edl-popover-header">
            <span>EDL preview</span>
            <button
              className="proposal-edl-popover-close"
              onClick={() => setEdlOpen(false)}
              aria-label="Close EDL preview"
            >
              ×
            </button>
          </header>
          <pre>
            {proposal.edlText
              ? proposal.edlText
              : "(no EDL text on adjusted proposals — drag-adjusted edits are tracked as locator deltas, not as a re-emitted text envelope)"}
          </pre>
        </PopoverContent>
      </Popover>
      <Tooltip
        content={
          <>
            Reject<kbd>Esc</kbd>
          </>
        }
      >
        <button
          className="proposal-actions-secondary proposal-actions-reject"
          onClick={reject}
          disabled={busy}
        >
          {busy ? "…" : "Reject"}
        </button>
      </Tooltip>
      <Tooltip
        content={
          <>
            Accept<kbd>Enter</kbd>
          </>
        }
      >
        <button
          className="proposal-actions-primary"
          onClick={accept}
          disabled={busy}
        >
          {busy ? "Applying…" : "Accept ⏎"}
        </button>
      </Tooltip>
      {error && <div className="proposal-actions-error">{error}</div>}
    </div>
  );
}
