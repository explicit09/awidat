/**
 * CaptionsDrillDown — caption-style rendering of the same whisper rows
 * the transcript drill-down shows. Visual differs from TranscriptDrillDown
 * to communicate that this is the SRT-shaped projection: one block per
 * line, mono font for the timestamp, body text rendered as a single
 * paragraph (no per-segment metadata).
 */

import { useCaptions } from "../../../state/evidenceAccessors";
import { DrillDownEmpty, formatTimecode, jumpTimelineTo } from "./DrillDownModal";

interface CaptionsDrillDownProps {
  onRunIndexers?: () => void;
}

export function CaptionsDrillDown({ onRunIndexers }: CaptionsDrillDownProps) {
  const captions = useCaptions();

  if (captions.length === 0) {
    return (
      <DrillDownEmpty
        message="No captions yet. Captions derive from the whisper sidecar — run whisper first."
        onRunIndexers={onRunIndexers}
      />
    );
  }

  return (
    <div
      className="flex flex-col gap-2"
      role="list"
      aria-label={`${captions.length} captions`}
    >
      {captions.map((cap, idx) => (
        <button
          key={cap.id}
          type="button"
          role="listitem"
          onClick={() => jumpTimelineTo(cap.start_s)}
          className="flex flex-col gap-1 p-2 rounded-[var(--radius-sm)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-card)] hover:bg-[var(--color-surface-card-hover)] text-left"
          title={`Jump to ${formatTimecode(cap.start_s)}`}
        >
          <div className="flex items-baseline justify-between gap-2 font-mono text-[var(--text-caption)] tabular-nums text-[var(--color-text-muted)]">
            <span>
              #{idx + 1} · {formatTimecode(cap.start_s)} → {formatTimecode(cap.end_s)}
            </span>
          </div>
          <div className="text-[var(--text-body-sm)] text-[var(--color-text-primary)]">
            {cap.text}
          </div>
        </button>
      ))}
    </div>
  );
}
