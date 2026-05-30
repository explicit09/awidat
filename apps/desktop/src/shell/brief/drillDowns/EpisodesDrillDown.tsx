/**
 * EpisodesDrillDown — chronological list of episode spans stamped on
 * the timeline by origin's first-class episodes feature (Wave 6 B1).
 *
 * Each card shows the episode number, name, time range, status, a
 * confidence bar, and the evidence count. Clicking jumps the timeline
 * cursor to the episode's start. Mirrors the visual contract of the
 * other Wave 3 C1 drill-down panels.
 */

import { useEpisodes, type EvidenceEpisode } from "../../../state/evidenceAccessors";
import { DrillDownEmpty, formatTimecode, jumpTimelineTo } from "./DrillDownModal";

interface EpisodesDrillDownProps {
  onRunIndexers?: () => void;
}

// Status badge styling — reuses the colour vocabulary from the inline
// episode chips in App.tsx so the visual language stays consistent.
const STATUS_STYLES: Record<
  EvidenceEpisode["status"],
  { label: string; bg: string; fg: string }
> = {
  accepted: {
    label: "Accepted",
    bg: "rgba(74,200,130,0.16)",
    fg: "#5EEAD4",
  },
  review_needed: {
    label: "Review",
    bg: "rgba(217,165,75,0.16)",
    fg: "#FCD34D",
  },
  rejected: {
    label: "Rejected",
    bg: "rgba(220,100,95,0.16)",
    fg: "#FCA5A5",
  },
};

export function EpisodesDrillDown({ onRunIndexers }: EpisodesDrillDownProps) {
  const episodes = useEpisodes();

  if (episodes.length === 0) {
    return (
      <DrillDownEmpty
        message="No episodes detected yet. Run the episodes indexer to populate this view."
        onRunIndexers={onRunIndexers}
      />
    );
  }

  return (
    <div
      className="flex flex-col gap-2"
      role="list"
      aria-label={`${episodes.length} episodes`}
    >
      {episodes.map((episode) => {
        const status = STATUS_STYLES[episode.status];
        const displayName = episode.name?.trim() || `Episode ${episode.order}`;
        const confidencePct = Math.max(
          0,
          Math.min(100, Math.round(episode.confidence * 100)),
        );
        return (
          <button
            key={episode.id}
            type="button"
            role="listitem"
            onClick={() => jumpTimelineTo(episode.startS)}
            className="flex flex-col gap-2 p-2.5 rounded-[var(--radius-sm)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-card)] text-left hover:bg-[var(--color-surface-card-hover)] hover:border-[var(--color-border)] transition-colors"
            title={`Jump to ${formatTimecode(episode.startS)}`}
          >
            <div className="flex items-start justify-between gap-2">
              <div className="flex items-baseline gap-2 min-w-0">
                <span
                  className="shrink-0 inline-flex items-center justify-center min-w-[1.5rem] h-5 px-1.5 rounded-[var(--radius-xs)] bg-[var(--color-surface-input)] font-mono tabular-nums text-[var(--text-caption)] text-[var(--color-text-muted)]"
                  aria-label={`Episode ${episode.order}`}
                >
                  {episode.order}
                </span>
                <span className="truncate text-[var(--text-body-sm)] font-semibold text-[var(--color-text-primary)]">
                  {displayName}
                </span>
              </div>
              <span
                className="shrink-0 inline-flex items-center px-1.5 py-0.5 rounded-[var(--radius-xs)] text-[10px] font-medium"
                style={{ background: status.bg, color: status.fg }}
              >
                {status.label}
              </span>
            </div>
            <div className="flex items-center justify-between gap-2">
              <span className="font-mono text-[var(--text-caption)] tabular-nums text-[var(--color-text-secondary)]">
                {formatTimecode(episode.startS)} → {formatTimecode(episode.endS)}{" "}
                <span className="text-[var(--color-text-muted)]">
                  ({formatTimecode(episode.durationS)})
                </span>
              </span>
              <span className="font-mono text-[var(--text-caption)] tabular-nums text-[var(--color-text-muted)]">
                {confidencePct}%
              </span>
            </div>
            <div
              className="h-1 w-full rounded-full bg-[var(--color-surface-input)] overflow-hidden"
              role="img"
              aria-label={`Confidence ${confidencePct}%`}
            >
              <span
                className="block h-full rounded-full bg-[var(--color-text-secondary)]"
                style={{ width: `${confidencePct}%` }}
              />
            </div>
            <span className="text-[var(--text-caption)] text-[var(--color-text-muted)]">
              {episode.evidenceCount}{" "}
              {episode.evidenceCount === 1 ? "evidence item" : "evidence items"}
            </span>
          </button>
        );
      })}
    </div>
  );
}
