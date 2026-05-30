/**
 * PatternsPanel — surface learned preferences from the rejection log
 * (Wave 5 C4).
 *
 * Lives above the History tab's entry list. Synthesizes the per-
 * project HistoryEntry slice into 4 stat tiles + a suggestion list
 * the user can paste into AGENTS.md.
 *
 * Why split out from HistorySurface: keeps HistorySurface under the
 * ~500-line soft cap and lets the panel evolve (different tile sets,
 * different empty states) without touching the row/restore code.
 *
 * Reads-only: pulls from `useProposalHistoryStore.entries` (already
 * project-scoped by the caller). No new state, no JSONL reads, no
 * LLM calls — the suggestions are hand-authored templates in
 * `state/learnedPatterns.ts`.
 */
import { useMemo, useState } from "react";
import { Lightbulb, Plus, Copy, Check } from "lucide-react";

import { cn } from "../ui";
import { MEDIUM_STYLE } from "./brief/ProposalCard";
import { synthesizeLearnedPatterns } from "../state/learnedPatterns";
import { useAgentsMdEditor } from "../state/agentsMdEditor";
import type { HistoryEntry } from "../state/proposalHistory";

interface PatternsPanelProps {
  /** Per-project entries — caller filters by projectPath. */
  entries: HistoryEntry[];
}

export function PatternsPanel({ entries }: PatternsPanelProps) {
  const patterns = useMemo(
    () => synthesizeLearnedPatterns(entries),
    [entries],
  );
  const openAgentsEditor = useAgentsMdEditor((s) => s.open);
  // Per-line copy state — keyed by index so multiple "Copy" buttons
  // can show their own ✓ flash without colliding.
  const [copiedIdx, setCopiedIdx] = useState<number | null>(null);

  async function copyLine(line: string, idx: number) {
    try {
      await navigator.clipboard.writeText(line);
      setCopiedIdx(idx);
      window.setTimeout(() => {
        setCopiedIdx((cur) => (cur === idx ? null : cur));
      }, 1500);
    } catch {
      // Clipboard API may be unavailable in tests / non-secure
      // contexts. Silent — the user can still copy by selection.
    }
  }

  function addAllToAgentsMd() {
    // The AgentsMd editor reads its own buffer on open; we just need
    // to surface the suggestions for the user to commit. Drop them on
    // the clipboard as a multi-line block so the user can paste them
    // into the editor that just opened — keeps this a one-line wire
    // change instead of plumbing a "pre-append" channel through the
    // editor's draft model.
    const payload = patterns.suggestedAgentsMdEdits.map((s) => `- ${s}`).join("\n");
    void navigator.clipboard?.writeText(payload).catch(() => {});
    openAgentsEditor();
  }

  if (patterns.totalRejects === 0) {
    return (
      <section
        className={cn(
          "border-b border-[var(--color-border-subtle)]",
          "bg-[var(--color-surface-app)] px-3 py-3",
        )}
        aria-label="Learned patterns"
      >
        <header className="mb-2 flex items-center gap-2 text-[var(--color-text-primary)]">
          <Lightbulb className="h-3.5 w-3.5" strokeWidth={1.75} />
          <span className="text-[12px] font-semibold">
            Learned patterns from your rejections
          </span>
        </header>
        <p className="text-[11px] text-[var(--color-text-muted)]">
          No patterns yet. Reject a proposal in the Brief and we'll
          surface trends here.
        </p>
      </section>
    );
  }

  const mostRejectedTile = patterns.mostRejectedMedium
    ? `${MEDIUM_STYLE[patterns.mostRejectedMedium.medium].label} (${patterns.mostRejectedMedium.pct}%)`
    : "—";
  const topReason = patterns.topRejectReasons[0];

  return (
    <section
      className={cn(
        "border-b border-[var(--color-border-subtle)]",
        "bg-[var(--color-surface-app)] px-3 py-3",
      )}
      aria-label="Learned patterns"
    >
      <header className="mb-2 flex items-center gap-2 text-[var(--color-text-primary)]">
        <Lightbulb className="h-3.5 w-3.5" strokeWidth={1.75} />
        <span className="text-[12px] font-semibold">
          Learned patterns from your rejections
        </span>
      </header>

      <div className="grid grid-cols-2 gap-2 md:grid-cols-4">
        <StatTile
          label="Most rejected"
          value={mostRejectedTile}
          mediumChip={patterns.mostRejectedMedium?.medium}
        />
        <StatTile
          label="Silent rejects"
          value={`${patterns.silentRejectPct}%`}
          sub="no reason given"
        />
        <StatTile
          label="Top reason"
          value={topReason ? `"${topReason.reason}"` : "—"}
          sub={
            topReason
              ? `${MEDIUM_STYLE[topReason.medium].label} × ${topReason.count}`
              : undefined
          }
        />
        <StatTile
          label="Total rejections"
          value={String(patterns.totalRejects)}
          sub="logged"
        />
      </div>

      {patterns.suggestedAgentsMdEdits.length > 0 && (
        <div className="mt-3">
          <div className="mb-1.5 flex items-center justify-between">
            <span className="text-[10px] font-medium uppercase tracking-wide text-[var(--color-text-muted)]">
              Suggested AGENTS.md edits
            </span>
            <button
              type="button"
              onClick={addAllToAgentsMd}
              className={cn(
                "inline-flex items-center gap-1 rounded-[var(--radius-sm)]",
                "border border-[var(--color-border-subtle)] bg-[var(--color-surface-app)]",
                "px-1.5 py-0.5 text-[10px] text-[var(--color-text-secondary)]",
                "hover:border-[var(--color-border-strong)] hover:text-[var(--color-text-primary)]",
              )}
              title="Open the AGENTS.md editor (copies suggestions to your clipboard)"
            >
              <Plus className="h-3 w-3" strokeWidth={1.75} />
              Add all to AGENTS.md
            </button>
          </div>
          <ul className="flex flex-col gap-1">
            {patterns.suggestedAgentsMdEdits.map((line, idx) => (
              <li
                key={`${idx}-${line}`}
                className={cn(
                  "flex items-start gap-2 rounded-[var(--radius-sm)]",
                  "border border-[var(--color-border-subtle)]",
                  "bg-[var(--color-surface-input)] px-2 py-1.5",
                )}
              >
                <span className="flex-1 text-[11px] leading-snug text-[var(--color-text-primary)]">
                  {line}
                </span>
                <button
                  type="button"
                  onClick={() => copyLine(line, idx)}
                  aria-label="Copy to clipboard"
                  className={cn(
                    "shrink-0 inline-flex items-center gap-1 rounded-[var(--radius-sm)]",
                    "border border-[var(--color-border-subtle)] px-1.5 py-0.5",
                    "text-[10px] text-[var(--color-text-secondary)]",
                    "hover:border-[var(--color-border-strong)] hover:text-[var(--color-text-primary)]",
                  )}
                  title="Copy this line"
                >
                  {copiedIdx === idx ? (
                    <>
                      <Check className="h-3 w-3" strokeWidth={2} />
                      Copied
                    </>
                  ) : (
                    <>
                      <Copy className="h-3 w-3" strokeWidth={1.75} />
                      Copy
                    </>
                  )}
                </button>
              </li>
            ))}
          </ul>
        </div>
      )}
    </section>
  );
}

function StatTile({
  label,
  value,
  sub,
  mediumChip,
}: {
  label: string;
  value: string;
  sub?: string;
  mediumChip?: keyof typeof MEDIUM_STYLE;
}) {
  return (
    <div
      className={cn(
        "rounded-[var(--radius-sm)] border border-[var(--color-border-subtle)]",
        "bg-[var(--color-surface-input)] px-2 py-1.5",
      )}
    >
      <p className="text-[10px] uppercase tracking-wide text-[var(--color-text-muted)]">
        {label}
      </p>
      <div className="mt-0.5 flex items-center gap-1.5">
        {mediumChip && (
          <span
            className={cn(
              "shrink-0 rounded-full border px-1.5 py-0.5 text-[10px] font-medium",
              MEDIUM_STYLE[mediumChip].className,
            )}
          >
            {MEDIUM_STYLE[mediumChip].label}
          </span>
        )}
        <p className="truncate text-[12px] font-semibold text-[var(--color-text-primary)]">
          {value}
        </p>
      </div>
      {sub && (
        <p className="mt-0.5 truncate text-[10px] text-[var(--color-text-muted)]">
          {sub}
        </p>
      )}
    </div>
  );
}
