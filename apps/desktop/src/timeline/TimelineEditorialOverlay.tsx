import { buildCutBadges, buildSplitOffsets } from "./editorialMarkers.ts";
import type { TimelineSnapshot } from "./store";

export function TimelineEditorialOverlay({
  snapshot,
  containerWidth,
  pps,
}: {
  snapshot: TimelineSnapshot;
  containerWidth: number;
  pps: number;
}) {
  if (containerWidth <= 0 || snapshot.tracks.length === 0) return null;
  const cutBadges = buildCutBadges(snapshot, pps);
  const splitOffsets = buildSplitOffsets(snapshot, pps);
  if (cutBadges.length === 0 && splitOffsets.length === 0) return null;
  return (
    <div
      className="timeline-editorial-overlay"
      style={{ width: containerWidth }}
      aria-label="Timeline editorial metadata"
    >
      {cutBadges.map((badge) => (
        <span
          key={badge.key}
          className="timeline-cut-badge"
          style={{ left: badge.x, top: badge.y }}
          title={badge.title}
        >
          {badge.label}
        </span>
      ))}
      {splitOffsets.map((marker) => (
        <span
          key={marker.key}
          className="timeline-split-offset"
          style={{ left: marker.x, top: marker.y }}
          title={marker.title}
        >
          {marker.label}
        </span>
      ))}
    </div>
  );
}
