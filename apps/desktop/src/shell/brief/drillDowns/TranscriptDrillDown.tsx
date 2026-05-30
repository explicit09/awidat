/**
 * TranscriptDrillDown — virtualization-free segment list (rendering 1k+
 * rows in the modal is fine; this surface is exploratory, not the main
 * transcript pane). Click a row to jump the timeline cursor to its start.
 */

import { useTranscriptSegments } from "../../../state/evidenceAccessors";
import { DrillDownEmpty, formatTimecode, jumpTimelineTo } from "./DrillDownModal";

interface TranscriptDrillDownProps {
  onRunIndexers?: () => void;
}

export function TranscriptDrillDown({ onRunIndexers }: TranscriptDrillDownProps) {
  const segments = useTranscriptSegments();

  if (segments.length === 0) {
    return (
      <DrillDownEmpty
        message="No transcript yet. Run the whisper indexer on this asset to populate."
        onRunIndexers={onRunIndexers}
      />
    );
  }

  return (
    <div
      className="flex flex-col gap-1"
      role="list"
      aria-label={`${segments.length} transcript segments`}
    >
      {segments.map((seg) => (
        <button
          key={seg.id}
          type="button"
          role="listitem"
          onClick={() => jumpTimelineTo(seg.start_s)}
          className="flex flex-col gap-1 p-2 rounded-[var(--radius-sm)] bg-[var(--color-surface-card)] hover:bg-[var(--color-surface-card-hover)] text-left"
          title={`Jump to ${formatTimecode(seg.start_s)}`}
        >
          <div className="flex items-baseline gap-2 font-mono text-[var(--text-caption)] tabular-nums">
            <span className="text-[var(--color-text-secondary)]">
              {formatTimecode(seg.start_s)}
            </span>
            {seg.speaker_id ? (
              <span className="text-[var(--color-text-muted)] uppercase tracking-[var(--text-caption--letter-spacing)]">
                {seg.speaker_id}
              </span>
            ) : null}
          </div>
          <div className="text-[var(--text-body-sm)] text-[var(--color-text-primary)]">
            {seg.text}
          </div>
        </button>
      ))}
    </div>
  );
}
