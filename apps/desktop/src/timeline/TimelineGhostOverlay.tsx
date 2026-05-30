// DOM overlay for the timeline canvas's ghost-clip review surface
// (Wave 3 C2). Sits over the canvas as a sibling, sharing the same
// coordinate space. Handles:
//
//   - Hover affordance — Accept / Reject buttons floating just above
//     each ghost clip.
//   - Tooltip — rationale + medium + source pill, shown on hover.
//   - Keyboard shortcuts — Tab/Shift+Tab cycle, ⌘↵ accepts focused,
//     Esc rejects focused, ? toggles the cheatsheet.
//   - Cheatsheet overlay — small popover listing the shortcuts.
//
// Coordinate model: canvas + overlay share `pps` / `laneHeight` /
// `containerWidth` from TimelineSurface's layout state, so everything
// here renders in CSS pixels relative to the same origin.

import { useEffect, useMemo, useState } from "react";
import { useBriefProposalsStore } from "../state/briefProposals.ts";
import { Button } from "../ui";
import {
  buildGhostRanges,
  collapseToAnchors,
  type GhostAnchor,
  type GhostRange,
} from "./ghostClipRanges.ts";
import { RULER_HEIGHT } from "./layout.ts";
import { usePendingProposals } from "./pendingProposals.ts";
import { useTimelineProposalFocus } from "./timelineProposalFocus.ts";
import type { TimelineSnapshot } from "./store.ts";

export interface TimelineGhostOverlayProps {
  snapshot: TimelineSnapshot;
  containerWidth: number;
  pps: number;
  laneHeight: number;
}

export function TimelineGhostOverlay({
  snapshot,
  containerWidth,
  pps,
  laneHeight,
}: TimelineGhostOverlayProps) {
  const pending = usePendingProposals((s) => s.pending);
  const focusedId = useTimelineProposalFocus((s) => s.focusedId);
  const cheatsheetOpen = useTimelineProposalFocus((s) => s.cheatsheetOpen);
  const focus = useTimelineProposalFocus((s) => s.focus);
  const cycle = useTimelineProposalFocus((s) => s.cycle);
  const clearFocus = useTimelineProposalFocus((s) => s.clear);
  const toggleCheatsheet = useTimelineProposalFocus((s) => s.toggleCheatsheet);
  const closeCheatsheet = useTimelineProposalFocus((s) => s.closeCheatsheet);
  const accept = useBriefProposalsStore((s) => s.accept);
  const reject = useBriefProposalsStore((s) => s.reject);

  // Cut-medium proposals only. Other mediums stay in the Brief stack
  // per task spec — color/audio/captions don't get timeline ghosts.
  const cutProposals = useMemo(
    () => pending.filter((p) => p.medium === "cut"),
    [pending],
  );

  const ranges: GhostRange[] = useMemo(
    () => buildGhostRanges(cutProposals, snapshot),
    [cutProposals, snapshot],
  );

  // Anchors deduplicate per proposal so each gets one floating
  // button-bar even when its diff hints span multiple clips.
  const anchors: GhostAnchor[] = useMemo(
    () => collapseToAnchors(ranges),
    [ranges],
  );

  const proposalIds = useMemo(() => anchors.map((a) => a.proposalId), [anchors]);

  // Whenever the proposal list shrinks (one was accepted/rejected) and
  // the focused id no longer exists, drop the focus ring so the next
  // Tab starts fresh rather than landing on a stale id.
  useEffect(() => {
    if (focusedId && !proposalIds.includes(focusedId)) {
      clearFocus();
    }
  }, [proposalIds, focusedId, clearFocus]);

  // Keyboard shortcuts. Window-level so the user doesn't have to
  // click into the canvas first — but gated against text-input focus
  // so typing in the composer doesn't accept a proposal.
  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if (isEditableTarget(e.target)) return;
      // Cheatsheet first — Esc inside it closes it without rejecting.
      if (cheatsheetOpen && e.key === "Escape") {
        e.preventDefault();
        closeCheatsheet();
        return;
      }
      // ? toggles cheatsheet (even with no pending proposals).
      if (e.key === "?" && !e.metaKey && !e.ctrlKey) {
        e.preventDefault();
        toggleCheatsheet();
        return;
      }
      // Tab / Shift+Tab cycles focus through cut proposals.
      if (e.key === "Tab" && proposalIds.length > 0) {
        e.preventDefault();
        cycle(proposalIds, e.shiftKey ? "backward" : "forward");
        return;
      }
      // ⌘↵ accepts the focused proposal; if none focused but exactly
      // one ghost exists, accept it (smart fallback).
      if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
        const targetId = focusedId ?? (proposalIds.length === 1 ? proposalIds[0]! : null);
        if (!targetId) return;
        e.preventDefault();
        void accept(targetId).catch((err) =>
          // eslint-disable-next-line no-console
          console.warn("ghost accept failed", targetId, err),
        );
        return;
      }
      // Esc rejects the focused proposal.
      if (e.key === "Escape" && focusedId) {
        e.preventDefault();
        void reject(focusedId).catch((err) =>
          // eslint-disable-next-line no-console
          console.warn("ghost reject failed", focusedId, err),
        );
      }
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [
    accept,
    cheatsheetOpen,
    closeCheatsheet,
    cycle,
    focusedId,
    proposalIds,
    reject,
    toggleCheatsheet,
  ]);

  if (anchors.length === 0 && !cheatsheetOpen) return null;

  return (
    <div
      className="timeline-ghost-overlay"
      style={{
        position: "absolute",
        inset: 0,
        width: containerWidth,
        pointerEvents: "none",
        zIndex: 3,
      }}
    >
      {anchors.map((anchor) => (
        <GhostAnchorOverlay
          key={anchor.proposalId}
          anchor={anchor}
          proposal={
            cutProposals.find((p) => p.callId === anchor.proposalId)!
          }
          focused={focusedId === anchor.proposalId}
          pps={pps}
          laneHeight={laneHeight}
          onFocus={() => focus(anchor.proposalId)}
          onAccept={() =>
            accept(anchor.proposalId).catch((err) =>
              // eslint-disable-next-line no-console
              console.warn("ghost accept failed", anchor.proposalId, err),
            )
          }
          onReject={() =>
            reject(anchor.proposalId).catch((err) =>
              // eslint-disable-next-line no-console
              console.warn("ghost reject failed", anchor.proposalId, err),
            )
          }
        />
      ))}
      {cheatsheetOpen ? <CheatsheetOverlay onClose={closeCheatsheet} /> : null}
    </div>
  );
}

interface GhostAnchorOverlayProps {
  anchor: GhostAnchor;
  proposal: {
    callId: string;
    summary: string;
    rationale?: string;
    medium: string;
    source: { source: "agent" | "user" };
  };
  focused: boolean;
  pps: number;
  laneHeight: number;
  onFocus: () => void;
  onAccept: () => void;
  onReject: () => void;
}

function GhostAnchorOverlay({
  anchor,
  proposal,
  focused,
  pps,
  laneHeight,
  onFocus,
  onAccept,
  onReject,
}: GhostAnchorOverlayProps) {
  const [hovered, setHovered] = useState(false);
  const left = Math.round(anchor.startS * pps);
  const width = Math.max(40, Math.round((anchor.endS - anchor.startS) * pps));
  const top = RULER_HEIGHT + anchor.trackIndex * laneHeight + 4;
  const height = laneHeight - 8;
  // Buttons float above the lane (8px above the top of the ghost),
  // tooltip floats above the buttons so it never collides.
  const show = hovered || focused;
  const sourceLabel = proposal.source.source === "agent" ? "Agent" : "User";

  return (
    <div
      // Pointer-events-auto on the hit target so hover registers, but
      // it's invisible by default — purely a hover/focus anchor. The
      // canvas underneath still receives clicks for scrub/select since
      // we exit pointer-events through transparency where possible.
      style={{
        position: "absolute",
        left,
        top,
        width,
        height,
        pointerEvents: "auto",
        background: "transparent",
        cursor: "pointer",
      }}
      onMouseEnter={() => setHovered(true)}
      onMouseLeave={() => setHovered(false)}
      onClick={(e) => {
        // Focus, but don't swallow propagation — the canvas should
        // still receive the click for seek-on-tap behaviour. We stop
        // propagation only on the button widgets.
        e.preventDefault();
        onFocus();
      }}
      role="button"
      tabIndex={-1}
      aria-label={`Pending cut proposal: ${proposal.summary || "Proposed edit"}`}
    >
      {show ? (
        <div
          // Floating button-bar — sits just above the ghost so the
          // ghost itself stays uncluttered.
          style={{
            position: "absolute",
            bottom: "calc(100% + 6px)",
            left: 0,
            display: "flex",
            gap: 6,
            pointerEvents: "auto",
          }}
          onClick={(e) => e.stopPropagation()}
        >
          <Button
            variant="primary"
            size="sm"
            onClick={(e) => {
              e.stopPropagation();
              onAccept();
            }}
            aria-label="Accept proposal"
          >
            Accept
            <kbd
              style={{
                marginLeft: 6,
                fontSize: 9,
                opacity: 0.78,
                fontFamily: "ui-monospace, monospace",
              }}
            >
              ⌘↵
            </kbd>
          </Button>
          <Button
            variant="ghost"
            size="sm"
            onClick={(e) => {
              e.stopPropagation();
              onReject();
            }}
            aria-label="Reject proposal"
          >
            Reject
            <kbd
              style={{
                marginLeft: 6,
                fontSize: 9,
                opacity: 0.78,
                fontFamily: "ui-monospace, monospace",
              }}
            >
              Esc
            </kbd>
          </Button>
        </div>
      ) : null}
      {hovered ? (
        <div
          // Dark popover tooltip — rationale + medium + source pill.
          // Positioned above the button-bar (when the bar is showing)
          // by accumulating their heights; here we just sit on top of
          // the bar with a large bottom-offset.
          style={{
            position: "absolute",
            bottom: "calc(100% + 42px)",
            left: 0,
            maxWidth: 320,
            background: "rgba(15, 15, 17, 0.96)",
            border: "1px solid var(--color-border-subtle, #1F1F22)",
            borderRadius: 6,
            padding: "8px 10px",
            color: "var(--color-text-primary, #E6E6E6)",
            fontSize: 11,
            lineHeight: 1.4,
            boxShadow: "0 6px 16px rgba(0, 0, 0, 0.5)",
            pointerEvents: "none",
            whiteSpace: "normal",
          }}
        >
          <div
            style={{
              display: "flex",
              gap: 6,
              alignItems: "center",
              marginBottom: proposal.rationale ? 6 : 0,
            }}
          >
            <span
              style={{
                fontSize: 9,
                fontWeight: 600,
                textTransform: "uppercase",
                letterSpacing: "0.08em",
                padding: "1px 6px",
                borderRadius: 3,
                background: "rgba(34,211,238,0.12)",
                color: "#67E8F9",
                border: "1px solid rgba(34,211,238,0.32)",
              }}
            >
              Cut
            </span>
            <span
              style={{
                fontSize: 9,
                fontWeight: 600,
                textTransform: "uppercase",
                letterSpacing: "0.08em",
                padding: "1px 6px",
                borderRadius: 3,
                background: "rgba(148,163,184,0.16)",
                color: "var(--color-text-muted, #9CA3AF)",
                border: "1px solid var(--color-border-subtle, #1F1F22)",
              }}
            >
              {sourceLabel}
            </span>
          </div>
          {proposal.rationale ? (
            <div style={{ fontStyle: "italic", opacity: 0.95 }}>
              {proposal.rationale}
            </div>
          ) : (
            <div style={{ opacity: 0.7, fontStyle: "italic" }}>
              {proposal.summary || "Proposed edit"}
            </div>
          )}
        </div>
      ) : null}
    </div>
  );
}

function CheatsheetOverlay({ onClose }: { onClose: () => void }) {
  return (
    <div
      style={{
        position: "absolute",
        top: RULER_HEIGHT + 12,
        right: 12,
        maxWidth: 280,
        background: "rgba(15, 15, 17, 0.96)",
        border: "1px solid var(--color-border-subtle, #1F1F22)",
        borderRadius: 6,
        padding: 12,
        color: "var(--color-text-primary, #E6E6E6)",
        fontSize: 11,
        lineHeight: 1.5,
        boxShadow: "0 8px 24px rgba(0, 0, 0, 0.55)",
        pointerEvents: "auto",
        zIndex: 10,
      }}
      role="dialog"
      aria-label="Keyboard shortcuts"
    >
      <div
        style={{
          display: "flex",
          justifyContent: "space-between",
          alignItems: "center",
          marginBottom: 8,
        }}
      >
        <strong style={{ fontSize: 12 }}>Review shortcuts</strong>
        <button
          type="button"
          aria-label="Close shortcuts"
          onClick={onClose}
          style={{
            background: "transparent",
            border: 0,
            color: "var(--color-text-muted, #9CA3AF)",
            cursor: "pointer",
            fontSize: 14,
            lineHeight: 1,
          }}
        >
          ×
        </button>
      </div>
      <ul style={{ listStyle: "none", padding: 0, margin: 0 }}>
        <CheatsheetRow keys={["Tab"]} label="Next pending proposal" />
        <CheatsheetRow keys={["Shift", "Tab"]} label="Previous proposal" />
        <CheatsheetRow keys={["⌘", "↵"]} label="Accept focused proposal" />
        <CheatsheetRow keys={["Esc"]} label="Reject focused proposal" />
        <CheatsheetRow keys={["?"]} label="Toggle this cheatsheet" />
      </ul>
    </div>
  );
}

function CheatsheetRow({
  keys,
  label,
}: {
  keys: ReadonlyArray<string>;
  label: string;
}) {
  return (
    <li
      style={{
        display: "flex",
        justifyContent: "space-between",
        alignItems: "center",
        gap: 12,
        marginBottom: 4,
      }}
    >
      <span style={{ opacity: 0.85 }}>{label}</span>
      <span style={{ display: "flex", gap: 3 }}>
        {keys.map((k, i) => (
          <kbd
            key={`${k}-${i}`}
            style={{
              fontFamily: "ui-monospace, monospace",
              fontSize: 10,
              padding: "1px 4px",
              borderRadius: 3,
              background: "var(--color-surface-card, #18181A)",
              border: "1px solid var(--color-border-subtle, #1F1F22)",
              color: "var(--color-text-primary, #E6E6E6)",
            }}
          >
            {k}
          </kbd>
        ))}
      </span>
    </li>
  );
}

function isEditableTarget(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  if (target.isContentEditable) return true;
  const tag = target.tagName.toLowerCase();
  return tag === "input" || tag === "textarea" || tag === "select";
}
