/**
 * BriefSurface — the strategic center pane of Wave 3 (B1+B2+B3+B4).
 *
 * Two render paths:
 *
 *   1. Empty state (no pending proposals):
 *      - Headline + subline
 *      - Prepare a starting cut (primary) + Open AGENTS.md (ghost)
 *      - Evidence chip row
 *
 *   2. Stack state (>=1 pending proposal):
 *      - Header strip with count + Prepare button
 *      - Vertical list of ProposalCard rows (newest first)
 *      - Evidence chip row at the bottom
 *
 * The center-pane tab toggle lives in `CenterModeTabs` (separate component
 * so App.tsx can render the tabs above whatever surface is active).
 */

import type { BriefProposal } from "../state/briefProposals";
import { useBriefProposalsStore } from "../state/briefProposals";
import { useAgentsMdEditor } from "../state/agentsMdEditor";
import { Button, Stack } from "../ui";
import { ProposalCard } from "./brief/ProposalCard";
import { EvidenceChipRow } from "./brief/EvidenceChipRow";
import { PrepareButton } from "./brief/PrepareButton";
import type { CenterMode } from "../state/centerMode";

export interface BriefSurfaceProps {
  /**
   * Optional notification hook fired after a "Review →" click — the
   * focus controller already drove the tab switch and entity focus by
   * the time this runs (Wave 4 W4.6). Demo screens / tests can still
   * pass it in to observe the route. Omit it in production wiring.
   */
  onReviewProposal?: (mode: CenterMode, proposal: BriefProposal) => void;
}

export function BriefSurface({ onReviewProposal }: BriefSurfaceProps) {
  // Subscribe to the approvals map so re-renders fire when ingest runs.
  useBriefProposalsStore((s) => s.approvals);
  const pending = useBriefProposalsStore.getState().pending();

  const isEmpty = pending.length === 0;

  return (
    <div className="flex h-full w-full flex-col overflow-hidden bg-[var(--color-surface-panel)]">
      <div className="flex-1 min-h-0 overflow-y-auto px-6 py-5">
        {isEmpty ? <EmptyState /> : <ProposalStack pending={pending} onReview={onReviewProposal} />}
      </div>

      <div className="shrink-0 border-t border-[var(--color-border-subtle)] bg-[var(--color-surface-panel)] px-6 py-3">
        <div className="flex items-center justify-between gap-3">
          <div className="text-[10px] font-medium uppercase tracking-[0.1em] text-[var(--color-text-muted)]">
            Evidence
          </div>
        </div>
        <div className="mt-2">
          <EvidenceChipRow />
        </div>
      </div>
    </div>
  );
}

function EmptyState() {
  const openAgentsMd = useAgentsMdEditor((s) => s.open);
  return (
    <div className="flex h-full min-h-[200px] items-center justify-center">
      <Stack gap="3" className="max-w-[42ch] text-center">
        <div className="text-[15px] font-semibold text-[var(--color-text-primary)]">
          No proposals yet.
        </div>
        <p className="text-[var(--text-body-sm)] text-[var(--color-text-secondary)]">
          Ask the agent for an edit, or hit Prepare to have it suggest a starting cut.
        </p>
        <div className="mt-1 flex flex-wrap justify-center gap-2">
          <PrepareButton />
          <Button variant="ghost" onClick={openAgentsMd}>
            Open AGENTS.md
          </Button>
        </div>
      </Stack>
    </div>
  );
}

function ProposalStack({
  pending,
  onReview,
}: {
  pending: BriefProposal[];
  onReview?: (mode: CenterMode, proposal: BriefProposal) => void;
}) {
  // pending() is already newest-first per the store contract.
  return (
    <div className="flex flex-col gap-3">
      <header className="flex items-center justify-between gap-3">
        <div className="flex items-baseline gap-2">
          <span className="text-[13px] font-semibold text-[var(--color-text-primary)]">
            {pending.length} pending
          </span>
          <span className="text-[11px] uppercase tracking-[0.1em] text-[var(--color-text-muted)]">
            {summarizeMediumCounts(pending)}
          </span>
        </div>
        <PrepareButton size="sm" variant="secondary" label="Prepare another pass" />
      </header>
      <div className="flex flex-col gap-1.5" role="list" aria-label="Pending proposals">
        {pending.map((p) => (
          <div key={`${p.source}:${p.id}`} role="listitem">
            <ProposalCard proposal={p} onReview={onReview} />
          </div>
        ))}
      </div>
    </div>
  );
}

/** Short "3 cuts · 1 color" summary for the stack header. */
function summarizeMediumCounts(pending: BriefProposal[]): string {
  const counts = new Map<string, number>();
  for (const p of pending) {
    counts.set(p.medium, (counts.get(p.medium) ?? 0) + 1);
  }
  const parts: string[] = [];
  for (const [medium, n] of counts) {
    const label = mediumPluralLabel(medium, n);
    parts.push(`${n} ${label}`);
  }
  return parts.join(" · ");
}

function mediumPluralLabel(medium: string, n: number): string {
  const base: Record<string, string> = {
    cut: "cut",
    color: "color",
    broll: "b-roll",
    audio: "audio",
    transition: "transition",
    title: "title",
    mixed: "mixed",
    other: "other",
  };
  const root = base[medium] ?? medium;
  // Simple plural for the common cases; "audio" / "b-roll" / "mixed" /
  // "other" don't take an "s" naturally so we leave them.
  if (n === 1) return root;
  if (root === "cut" || root === "color" || root === "transition" || root === "title") {
    return `${root}s`;
  }
  return root;
}
