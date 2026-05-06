// Right-rail properties inspector — third pane in the workspace top
// row. Step 13 ships a read-only stub: when a clip is selected on
// the timeline, render its metadata; when nothing is selected (or
// the timeline is empty), render an empty-state hint.
//
// Future steps wire interactive controls (Step 15: volume + speed
// sliders, Step 16: text overlay editor) directly onto this pane.
// Each future control reads the same selectedClipKey + writes via
// propose_user_edit, so the pane mostly grows downward.

import { useTimelineStore } from "../timeline/store";
import { useTimelineSelectionStore } from "./store";

export function PropertiesPane() {
  const snapshot = useTimelineStore((s) => s.snapshot);
  const key = useTimelineSelectionStore((s) => s.selectedClipKey);

  const item =
    key === null
      ? null
      : snapshot.tracks[key.trackIndex]?.items[key.clipIndex] ?? null;

  if (!item || item.kind !== "clip") {
    return (
      <section className="properties-pane">
        <header className="properties-header">
          <span className="properties-label">Properties</span>
        </header>
        <div className="properties-empty">
          Click a clip on the timeline to inspect it.
        </div>
      </section>
    );
  }

  const sourceStart = item.source_start_s ?? 0;
  const sourceEnd = sourceStart + item.duration_s;
  const trackStart = item.track_start_s;
  const trackEnd = trackStart + item.duration_s;
  const trackName = snapshot.tracks[key!.trackIndex]?.name ?? "?";
  const trackKind = snapshot.tracks[key!.trackIndex]?.kind ?? "?";

  return (
    <section className="properties-pane">
      <header className="properties-header">
        <span className="properties-label">Properties</span>
      </header>
      <div className="properties-body">
        <Field label="Name">
          <span className="properties-value">{item.name}</span>
        </Field>
        <Field label="Track">
          <span className="properties-value">
            {trackName} <span className="properties-dim">· {trackKind}</span>
          </span>
        </Field>
        <Field label="Asset">
          <code className="properties-code" title={item.asset_id ?? ""}>
            {item.asset_id ?? "(none)"}
          </code>
        </Field>
        <Field label="Source">
          <span className="properties-value">
            {sourceStart.toFixed(2)}s → {sourceEnd.toFixed(2)}s
          </span>
        </Field>
        <Field label="Timeline">
          <span className="properties-value">
            {trackStart.toFixed(2)}s → {trackEnd.toFixed(2)}s
          </span>
        </Field>
        <Field label="Duration">
          <span className="properties-value">{item.duration_s.toFixed(2)}s</span>
        </Field>
        <Field label="Clip uuid">
          <code className="properties-code" title={item.clip_uuid}>
            {item.clip_uuid}
          </code>
        </Field>
      </div>
    </section>
  );
}

function Field({
  label,
  children,
}: {
  label: string;
  children: React.ReactNode;
}) {
  return (
    <div className="properties-field">
      <div className="properties-field-label">{label}</div>
      <div className="properties-field-value">{children}</div>
    </div>
  );
}
