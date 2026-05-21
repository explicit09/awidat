import type { TimelineSnapshot } from "./store";

export type SnapTargetKind = "clip_edge" | "marker" | "playhead";

export type SnapTarget = {
  kind: SnapTargetKind;
  timeS: number;
};

export type SnapCollectionOptions = {
  playheadS?: number;
  markerTimesS?: readonly number[];
  excludeClipUuids?: ReadonlySet<string>;
};

export function collectSnapTargets(
  snapshot: TimelineSnapshot,
  options: SnapCollectionOptions = {},
): SnapTarget[] {
  const targets: SnapTarget[] = [];
  const excluded = options.excludeClipUuids ?? new Set<string>();
  for (const track of snapshot.tracks) {
    for (const item of track.items) {
      if (item.kind !== "clip" || excluded.has(item.clip_uuid)) continue;
      targets.push({ kind: "clip_edge", timeS: item.track_start_s });
      targets.push({
        kind: "clip_edge",
        timeS: item.track_start_s + item.duration_s,
      });
    }
  }
  if (options.playheadS !== undefined && Number.isFinite(options.playheadS)) {
    targets.push({ kind: "playhead", timeS: Math.max(0, options.playheadS) });
  }
  for (const timeS of options.markerTimesS ?? []) {
    if (Number.isFinite(timeS) && timeS >= 0) {
      targets.push({ kind: "marker", timeS });
    }
  }
  return targets;
}

export function nearestSnap(
  timeS: number,
  targets: readonly SnapTarget[],
  toleranceS: number,
): SnapTarget | null {
  if (!Number.isFinite(timeS) || !Number.isFinite(toleranceS) || toleranceS < 0) {
    return null;
  }
  let best: SnapTarget | null = null;
  let bestDistance = Number.POSITIVE_INFINITY;
  for (const target of targets) {
    if (!Number.isFinite(target.timeS)) continue;
    const distance = Math.abs(target.timeS - timeS);
    if (distance > toleranceS) continue;
    if (
      best === null ||
      distance < bestDistance ||
      (distance === bestDistance && priority(target.kind) < priority(best.kind))
    ) {
      best = target;
      bestDistance = distance;
    }
  }
  return best;
}

export function snapTime(
  timeS: number,
  targets: readonly SnapTarget[],
  toleranceS: number,
): number {
  return nearestSnap(timeS, targets, toleranceS)?.timeS ?? timeS;
}

function priority(kind: SnapTargetKind): number {
  switch (kind) {
    case "marker":
      return 0;
    case "clip_edge":
      return 1;
    case "playhead":
      return 2;
  }
}
