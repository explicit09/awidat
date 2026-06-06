// ClipInspector — thin shell that wraps PropertiesPane as the default
// right-rail Inspector tab content when no proposal is active.
//
// When a clip is selected, renders PropertiesPane (which handles all
// sections: Identity, Audio, Visual, Timing, etc.).  When nothing is
// selected and the playhead is not over a clip, shows a calm empty-state
// hint rather than the panel's own "No active clip" header.

import { useTimelineSelectionStore } from "../properties/store";
import { PropertiesPane } from "../properties/PropertiesPane";
import type { EditorPublishingSummary } from "../editor/publishingBridge";

export function ClipInspector({
  publishing,
  onOpenDelivery,
}: {
  publishing?: EditorPublishingSummary;
  onOpenDelivery?: () => void;
}) {
  const selectedClipKey = useTimelineSelectionStore((s) => s.selectedClipKey);

  if (!selectedClipKey) {
    return (
      <div className="flex h-full flex-col items-center justify-center gap-4 p-6 text-center">
        <p className="max-w-[220px] text-[var(--color-text-muted)] leading-relaxed text-sm">
          Select a clip on the timeline to inspect its properties, or prepare the current edit for publishing.
        </p>
        <EditorPublishBridge publishing={publishing} onOpenDelivery={onOpenDelivery} />
      </div>
    );
  }

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="shrink-0 border-b border-[var(--color-border-subtle)] p-3">
        <EditorPublishBridge publishing={publishing} onOpenDelivery={onOpenDelivery} />
      </div>
      <div className="min-h-0 flex-1 overflow-hidden">
        <PropertiesPane />
      </div>
    </div>
  );
}

function EditorPublishBridge({
  publishing,
  onOpenDelivery,
}: {
  publishing?: EditorPublishingSummary;
  onOpenDelivery?: () => void;
}) {
  if (!publishing) return null;
  return (
    <div className="w-full rounded-[var(--radius-sm)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-card)] p-3 text-left">
      <div className="text-[var(--text-label)] font-semibold uppercase tracking-[var(--text-label--letter-spacing)] text-[var(--color-text-muted)]">
        Publish
      </div>
      <p className="mt-1 text-[var(--text-caption)] text-[var(--color-text-secondary)]">
        {publishing.copy}
      </p>
      <button
        type="button"
        onClick={onOpenDelivery}
        className="mt-2 w-full rounded-[var(--radius-sm)] border border-[rgba(255,122,24,0.42)] bg-[rgba(255,122,24,0.14)] px-2 py-1.5 text-[var(--text-caption)] font-semibold text-[#FFB073] hover:bg-[rgba(255,122,24,0.2)]"
      >
        Open Delivery
      </button>
    </div>
  );
}
