/**
 * SpeakersDrillDown — list of diarized speakers with segment count + total
 * speech time. Click logs the speaker id (transcript filter wiring is a
 * follow-up; the drill-down surfaces what we know today).
 */

import { useSpeakers } from "../../../state/evidenceAccessors";
import { DrillDownEmpty, formatTimecode } from "./DrillDownModal";

interface SpeakersDrillDownProps {
  onRunIndexers?: () => void;
}

export function SpeakersDrillDown({ onRunIndexers }: SpeakersDrillDownProps) {
  const speakers = useSpeakers();

  if (speakers.length === 0) {
    return (
      <DrillDownEmpty
        message="No diarized speakers yet. Run the whisper indexer with diarization enabled."
        onRunIndexers={onRunIndexers}
      />
    );
  }

  return (
    <div
      className="flex flex-col gap-1.5"
      role="list"
      aria-label={`${speakers.length} speakers`}
    >
      {speakers.map((sp) => (
        <button
          key={sp.id}
          type="button"
          role="listitem"
          onClick={() => {
            // TODO(C1): filter transcript pane to this speaker once a
            // shared `transcript.filter` action lands.
            // eslint-disable-next-line no-console
            console.log("speakers drill-down: filter transcript to", sp.id);
          }}
          className="flex items-center gap-3 p-2 rounded-[var(--radius-sm)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-card)] hover:bg-[var(--color-surface-card-hover)] text-left"
          title={`Focus transcript on ${sp.label}`}
        >
          {sp.portrait_path ? (
            <img
              src={sp.portrait_path}
              alt=""
              className="w-10 h-10 rounded-full object-cover bg-[var(--color-surface-input)]"
              loading="lazy"
            />
          ) : (
            <div
              className="w-10 h-10 rounded-full grid place-items-center bg-[var(--color-surface-input)] text-[var(--text-caption)] font-mono text-[var(--color-text-muted)]"
              aria-hidden="true"
            >
              {sp.label.slice(-2)}
            </div>
          )}
          <div className="flex-1 min-w-0">
            <div className="text-[var(--text-body-sm)] font-medium text-[var(--color-text-primary)] truncate">
              {sp.label}
            </div>
            <div className="text-[var(--text-caption)] text-[var(--color-text-muted)]">
              {sp.segment_count} segment{sp.segment_count === 1 ? "" : "s"} ·{" "}
              {formatTimecode(sp.total_speech_s)}
            </div>
          </div>
        </button>
      ))}
    </div>
  );
}
