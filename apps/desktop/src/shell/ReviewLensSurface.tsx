import { Eye, Headphones, MessageSquare, Pause, SquareDashed } from "lucide-react";
import { useState, type ReactNode } from "react";
import {
  Button,
  ChannelLanes,
  EvidenceChip,
  Inline,
  Stack,
  TranscriptSegment,
  cn,
  type ChannelLane,
} from "../ui";

/**
 * ReviewLensSurface — the Review lens view from concept Screen 4.
 *
 * Renders the transcript as a first-class editing surface: each
 * sentence is a TranscriptSegment with evidence chips and semantic
 * action affordances ("Keep this pause", "Edit around selection",
 * "Add note"). Below the transcript: a channel lane strip showing
 * sentiment / energy / confidence / speaker signals over time.
 */

export type ReviewTranscriptSegment = {
  id: string;
  speaker: string;
  speakerColor?: string;
  startTime: string;
  startS: number;
  endS: number;
  text: string;
  confidence?: number;
  evidence?: { id: string; label: string }[];
  state?: "default" | "selected" | "accepted" | "rejected" | "warning";
};

export type ReviewLensSurfaceProps = {
  segments?: ReviewTranscriptSegment[];
  channelLanes?: ChannelLane[];
  durationS?: number;
  currentTimeS?: number;
  selectedSegmentId?: string;
  onSelectSegment?: (segment: ReviewTranscriptSegment) => void;
  onKeepPause?: (segment: ReviewTranscriptSegment) => void;
  onEditAround?: (segment: ReviewTranscriptSegment) => void;
  onAddNote?: (segment: ReviewTranscriptSegment) => void;
  className?: string;
};

export function ReviewLensSurface({
  segments = [],
  channelLanes = [],
  durationS = 0,
  currentTimeS = 0,
  selectedSegmentId,
  onSelectSegment,
  onKeepPause,
  onEditAround,
  onAddNote,
  className,
}: ReviewLensSurfaceProps) {
  const [hoveredId, setHoveredId] = useState<string | null>(null);
  return (
    <div className={cn("flex h-full flex-col bg-[var(--color-surface-app)]", className)}>
      <div className="h-10 px-4 border-b border-[var(--color-border-subtle)] flex items-center justify-between shrink-0">
        <Inline gap="2" align="center">
          <Eye className="h-3.5 w-3.5 stroke-[1.75] text-[var(--color-brand)]" />
          <span className="text-[var(--text-label)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-secondary)]">
            Review · Transcript
          </span>
          <span className="text-[var(--text-caption)] text-[var(--color-text-muted)]">
            {segments.length} {segments.length === 1 ? "segment" : "segments"}
          </span>
        </Inline>
        <Inline gap="1" align="center">
          <FilterChip icon={<Headphones className="h-3 w-3 stroke-[1.75]" />} label="Audio cues" />
          <FilterChip icon={<MessageSquare className="h-3 w-3 stroke-[1.75]" />} label="Notes" />
        </Inline>
      </div>

      <div className="flex-1 overflow-y-auto p-3">
        {segments.length === 0 ? (
          <EmptyState />
        ) : (
          <Stack gap="2">
            {segments.map((seg) => {
              const isSelected = selectedSegmentId === seg.id;
              const isHovered = hoveredId === seg.id;
              const effState = isSelected ? "selected" : seg.state ?? "default";
              return (
                <div
                  key={seg.id}
                  onMouseEnter={() => setHoveredId(seg.id)}
                  onMouseLeave={() => setHoveredId((id) => (id === seg.id ? null : id))}
                >
                  <TranscriptSegment
                    speaker={seg.speaker}
                    speakerColor={seg.speakerColor}
                    startTime={seg.startTime}
                    text={seg.text}
                    confidence={seg.confidence}
                    state={effState}
                    onClick={() => onSelectSegment?.(seg)}
                    evidence={
                      seg.evidence && seg.evidence.length > 0
                        ? seg.evidence.map((ev) => (
                            <EvidenceChip key={ev.id} label={ev.label} />
                          ))
                        : undefined
                    }
                    actions={
                      isSelected || isHovered ? (
                        <>
                          <Button
                            variant="ghost"
                            size="xs"
                            onClick={(e) => {
                              e.stopPropagation();
                              onKeepPause?.(seg);
                            }}
                            leadingIcon={<Pause className="h-3 w-3 stroke-[1.75]" />}
                          >
                            Keep pause
                          </Button>
                          <Button
                            variant="ghost"
                            size="xs"
                            onClick={(e) => {
                              e.stopPropagation();
                              onEditAround?.(seg);
                            }}
                            leadingIcon={<SquareDashed className="h-3 w-3 stroke-[1.75]" />}
                          >
                            Edit around
                          </Button>
                          <Button
                            variant="ghost"
                            size="xs"
                            onClick={(e) => {
                              e.stopPropagation();
                              onAddNote?.(seg);
                            }}
                            leadingIcon={<MessageSquare className="h-3 w-3 stroke-[1.75]" />}
                          >
                            Note
                          </Button>
                        </>
                      ) : undefined
                    }
                  />
                </div>
              );
            })}
          </Stack>
        )}
      </div>

      {channelLanes.length > 0 ? (
        <div className="border-t border-[var(--color-border-subtle)] p-3 shrink-0">
          <ChannelLanes durationS={durationS} lanes={channelLanes} currentTimeS={currentTimeS} />
        </div>
      ) : null}
    </div>
  );
}

function FilterChip({ icon, label }: { icon: ReactNode; label: string }) {
  return (
    <button
      type="button"
      className="inline-flex items-center gap-1 h-6 px-2 rounded-[var(--radius-xs)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-card)] hover:border-[var(--color-border)] text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] text-[var(--text-caption)] font-medium transition-colors"
    >
      {icon}
      <span>{label}</span>
    </button>
  );
}

function EmptyState() {
  return (
    <div className="flex h-full items-center justify-center p-8">
      <Stack gap="2" align="center" className="text-center">
        <span className="text-[var(--text-h3)] font-semibold text-[var(--color-text-primary)]">
          No transcript yet
        </span>
        <span className="text-[var(--text-body-sm)] text-[var(--color-text-secondary)] max-w-md">
          Import media and run the transcript indexer to start reviewing.
          Every sentence becomes a first-class edit surface — keep a pause,
          edit around a selection, or add a note.
        </span>
      </Stack>
    </div>
  );
}
