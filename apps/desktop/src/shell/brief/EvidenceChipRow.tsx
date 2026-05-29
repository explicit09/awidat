/**
 * EvidenceChipRow — horizontal chip row showing indexer counts (Wave 3 B3).
 *
 * One chip per indexer. Counts come from `evidenceAccessors`; availability
 * comes from `useEvidenceAvailability()`. Available chips are clickable
 * and open the per-kind drill-down panel from `drillDowns/` (Wave 3 C1).
 * Unavailable chips render greyed-out.
 */

import { useState } from "react";
import {
  useScenes,
  useSpeakers,
  useSilences,
  useTranscriptSegments,
  useCaptions,
  useColorStats,
  useMotionRegions,
  useFaces,
  useAudioSamples,
  useEvidenceAvailability,
  type EvidenceAvailability,
} from "../../state/evidenceAccessors";
import { cn } from "../../ui/cn";
import {
  EvidenceDrillDown,
  type DrillDownKind,
} from "./drillDowns/EvidenceDrillDown";

type ChipKey = keyof EvidenceAvailability;

// Display order left → right. Mirrors the indexer fan-out the agent reads.
const CHIP_ORDER: { key: ChipKey; label: string }[] = [
  { key: "scenes", label: "Scenes" },
  { key: "speakers", label: "Speakers" },
  { key: "silences", label: "Silences" },
  { key: "transcript", label: "Transcript" },
  { key: "captions", label: "Captions" },
  { key: "color", label: "Color" },
  { key: "motion", label: "Motion" },
  { key: "faces", label: "Faces" },
  { key: "audio", label: "Audio" },
];

export function EvidenceChipRow() {
  // Call every accessor so the row stays subscribed to all indexer
  // slices; each returns [] when data isn't loaded.
  const counts: Record<ChipKey, number> = {
    scenes: useScenes().length,
    speakers: useSpeakers().length,
    silences: useSilences().length,
    transcript: useTranscriptSegments().length,
    captions: useCaptions().length,
    color: useColorStats().length,
    motion: useMotionRegions().length,
    faces: useFaces().length,
    audio: useAudioSamples().length,
  };
  const availability = useEvidenceAvailability();
  const [drillDownFor, setDrillDownFor] = useState<{ key: ChipKey; label: string } | null>(null);

  return (
    <>
      <div className="flex flex-wrap items-center gap-1.5" role="list" aria-label="Indexer evidence">
        {CHIP_ORDER.map(({ key, label }) => {
          const isAvailable = availability[key];
          const count = counts[key];
          return (
            <button
              key={key}
              type="button"
              role="listitem"
              disabled={!isAvailable}
              onClick={() => isAvailable && setDrillDownFor({ key, label })}
              title={isAvailable ? `Drill into ${label} (${count})` : `${label}: indexer output not yet available`}
              className={cn(
                "inline-flex items-center gap-1.5 h-6 px-2",
                "rounded-[var(--radius-sm)] border bg-[var(--color-surface-card)]",
                "border-[var(--color-border-subtle)]",
                "text-[var(--text-caption)] leading-[1] font-medium whitespace-nowrap uppercase",
                "tracking-[var(--text-caption--letter-spacing)]",
                "transition-[background-color,color] duration-[120ms]",
                isAvailable
                  ? "text-[var(--color-text-secondary)] hover:bg-[var(--color-surface-card-hover)] hover:text-[var(--color-text-primary)] cursor-pointer"
                  : "text-[var(--color-text-disabled)] cursor-not-allowed opacity-60",
              )}
            >
              <span>{label}</span>
              <span className="font-mono tabular-nums text-[var(--color-text-muted)]">{count}</span>
            </button>
          );
        })}
      </div>

      {drillDownFor && (
        <EvidenceDrillDown
          kind={drillDownFor.key as DrillDownKind}
          label={drillDownFor.label}
          onClose={() => setDrillDownFor(null)}
        />
      )}
    </>
  );
}
