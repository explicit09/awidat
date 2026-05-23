import type { TimelineItem, TimelineSnapshot } from "./store.ts";
import { collectSnapTargets, snapTime } from "./snap.ts";

export const SNAP_TOLERANCE_S = 0.15;

export type UserTrimDrag = {
  hit: {
    clipUuid: string;
    side: "start" | "end";
    sourceStart: number;
    sourceEnd: number;
  };
  startX: number;
  currentX: number;
};

export type UserMoveDrag = {
  trackIndex: number;
  clipIndex: number;
  clipUuid: string;
  linkGroupId: string | null;
  startX: number;
  currentX: number;
  startY: number;
  currentY: number;
};

export function targetPositionForMove(
  items: TimelineItem[],
  movingIndex: number,
  targetStartS: number,
): number {
  const ordered = [...items].sort(
    (a, b) => a.track_start_s - b.track_start_s || a.index - b.index,
  );
  let target = 0;
  for (const item of ordered) {
    if (item.index === movingIndex) continue;
    const midpoint = item.track_start_s + item.duration_s / 2;
    if (targetStartS < midpoint) {
      return item.index;
    }
    target = item.index + 1;
  }
  return Math.max(0, target);
}

export function snapMoveDeltaS(
  snapshot: TimelineSnapshot,
  currentTime: number,
  drag: UserMoveDrag,
  pps: number,
): number {
  const dxS = (drag.currentX - drag.startX) / Math.max(0.001, pps);
  const primary = snapshot.tracks[drag.trackIndex]?.items.find(
    (item): item is Extract<TimelineItem, { kind: "clip" }> =>
      item.kind === "clip" && item.index === drag.clipIndex,
  );
  if (!primary) return dxS;
  const excluded =
    drag.linkGroupId !== null
      ? new Set(
          snapshot.tracks.flatMap((track) =>
            track.items
              .filter(
                (item): item is Extract<TimelineItem, { kind: "clip" }> =>
                  item.kind === "clip" && item.link_group_id === drag.linkGroupId,
              )
              .map((item) => item.clip_uuid),
          ),
        )
      : new Set([primary.clip_uuid]);
  const targets = collectSnapTargets(snapshot, {
    playheadS: currentTime,
    excludeClipUuids: excluded,
  });
  const snappedStartS = snapTime(
    primary.track_start_s + dxS,
    targets,
    SNAP_TOLERANCE_S,
  );
  return snappedStartS - primary.track_start_s;
}
