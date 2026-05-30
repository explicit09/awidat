/**
 * ScenesDrillDown — grid of scene boundaries from the scenedetect indexer.
 *
 * Each tile shows start-end timecode + a placeholder thumbnail rectangle
 * (the indexer doesn't ship thumbnails today; when it does, swap the
 * placeholder for `<img src={scene.thumbnail_path} />`). Click jumps
 * the timeline cursor to the scene's start.
 */

import { useScenes } from "../../../state/evidenceAccessors";
import { DrillDownEmpty, formatTimecode, jumpTimelineTo } from "./DrillDownModal";

interface ScenesDrillDownProps {
  onRunIndexers?: () => void;
}

export function ScenesDrillDown({ onRunIndexers }: ScenesDrillDownProps) {
  const scenes = useScenes();

  if (scenes.length === 0) {
    return (
      <DrillDownEmpty
        message="No scenes detected yet. Run the scenedetect indexer to populate this view."
        onRunIndexers={onRunIndexers}
      />
    );
  }

  return (
    <div
      className="grid gap-2"
      style={{
        gridTemplateColumns: "repeat(auto-fill, minmax(140px, 1fr))",
      }}
      role="list"
      aria-label={`${scenes.length} scenes`}
    >
      {scenes.map((scene) => (
        <button
          key={scene.id}
          type="button"
          role="listitem"
          onClick={() => jumpTimelineTo(scene.start_s)}
          className="flex flex-col gap-1 p-1.5 rounded-[var(--radius-sm)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-card)] text-left hover:bg-[var(--color-surface-card-hover)] hover:border-[var(--color-border)] transition-colors"
          title={`Jump to ${formatTimecode(scene.start_s)}`}
        >
          {scene.thumbnail_path ? (
            <img
              src={scene.thumbnail_path}
              alt=""
              className="aspect-video w-full rounded-[var(--radius-xs)] object-cover bg-[var(--color-surface-input)]"
              loading="lazy"
            />
          ) : (
            <div
              className="aspect-video w-full rounded-[var(--radius-xs)] bg-[var(--color-surface-input)]"
              aria-hidden="true"
            />
          )}
          <div className="flex justify-between gap-1 px-0.5 font-mono text-[var(--text-caption)] tabular-nums text-[var(--color-text-secondary)]">
            <span>{formatTimecode(scene.start_s)}</span>
            <span className="text-[var(--color-text-muted)]">
              {formatTimecode(scene.end_s)}
            </span>
          </div>
        </button>
      ))}
    </div>
  );
}
