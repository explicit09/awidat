// ClipInspector — thin shell that wraps PropertiesPane as the default
// right-rail Inspector tab content when no proposal is active.
//
// When a clip is selected, renders PropertiesPane (which handles all
// sections: Identity, Audio, Visual, Timing, etc.).  When nothing is
// selected and the playhead is not over a clip, shows a calm empty-state
// hint rather than the panel's own "No active clip" header.

import { useTimelineSelectionStore } from "../properties/store";
import { PropertiesPane } from "../properties/PropertiesPane";

export function ClipInspector() {
  const selectedClipKey = useTimelineSelectionStore((s) => s.selectedClipKey);

  if (!selectedClipKey) {
    return (
      <div className="flex h-full items-center justify-center p-6 text-center">
        <p className="max-w-[200px] text-[var(--color-text-muted)] leading-relaxed text-sm">
          Select a clip on the timeline to inspect its properties.
        </p>
      </div>
    );
  }

  return <PropertiesPane />;
}
