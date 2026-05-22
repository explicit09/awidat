import type { SelectedClipKey } from "../properties/store.ts";
import type { EdlOp } from "./edlBuilder.ts";
import {
  snapMoveDeltaS,
  targetPositionForMove,
  type UserMoveDrag,
  type UserTrimDrag,
} from "./editMath.ts";
import { shouldKeepMoveDraft } from "./moveDraft.ts";
import type { TimelineItem, TimelineSnapshot } from "./store.ts";
import { pxDeltaToSourceDelta } from "./hitDetect.ts";

export function buildDeleteSelectionOps(
  snapshot: TimelineSnapshot,
  selectedClipKey: SelectedClipKey | null,
): EdlOp[] {
  if (!selectedClipKey) return [];
  const selectedTrack = snapshot.tracks[selectedClipKey.trackIndex];
  const selectedItem = selectedTrack?.items.find(
    (item) => item.index === selectedClipKey.clipIndex,
  );
  if (!selectedTrack || !selectedItem) return [];

  if (selectedItem.kind === "transition") {
    const position = selectedTrack.items.findIndex(
      (item) => item.index === selectedItem.index,
    );
    const from = selectedTrack.items[position - 1];
    const to = selectedTrack.items[position + 1];
    if (from?.kind !== "clip" || to?.kind !== "clip") return [];
    return [
      {
        kind: "delete_transition",
        from: { kind: "clip_uuid", uuid: from.clip_uuid },
        to: { kind: "clip_uuid", uuid: to.clip_uuid },
      },
    ];
  }

  if (selectedItem.kind !== "clip") return [];
  const clips =
    selectedItem.link_group_id !== null
      ? snapshot.tracks.flatMap((track) =>
          track.items.filter(
            (item): item is Extract<TimelineItem, { kind: "clip" }> =>
              item.kind === "clip" &&
              item.link_group_id === selectedItem.link_group_id,
          ),
        )
      : [selectedItem];
  const seen = new Set<string>();
  return clips
    .filter((clip) => {
      if (seen.has(clip.clip_uuid)) return false;
      seen.add(clip.clip_uuid);
      return true;
    })
    .map((clip) => ({
      kind: "delete_clip",
      anchor: { kind: "clip_uuid", uuid: clip.clip_uuid },
    }));
}

export function buildTrimDragOps(drag: UserTrimDrag, pps: number): EdlOp[] {
  const dxPx = drag.currentX - drag.startX;
  const dxS = pxDeltaToSourceDelta(dxPx, pps);
  if (Math.abs(dxPx) < 2) return [];

  const { hit } = drag;
  let newStart = hit.sourceStart;
  let newEnd = hit.sourceEnd;
  if (hit.side === "start") {
    newStart = Math.max(0, hit.sourceStart + dxS);
    if (newStart >= newEnd) newStart = newEnd - 0.1;
  } else {
    newEnd = Math.max(hit.sourceStart + 0.1, hit.sourceEnd + dxS);
  }
  if (
    Math.abs(newStart - hit.sourceStart) < 0.01 &&
    Math.abs(newEnd - hit.sourceEnd) < 0.01
  ) {
    return [];
  }
  return [
    {
      kind: "trim_clip",
      anchor: { kind: "clip_uuid", uuid: hit.clipUuid },
      start: newStart !== hit.sourceStart ? newStart : undefined,
      end: newEnd !== hit.sourceEnd ? newEnd : undefined,
    },
  ];
}

export function buildMoveDragOps(
  snapshot: TimelineSnapshot,
  currentTime: number,
  drag: UserMoveDrag,
  pps: number,
): EdlOp[] {
  const dxPx = drag.currentX - drag.startX;
  const dyPx = drag.currentY - drag.startY;
  if (Math.hypot(dxPx, dyPx) < 5) return [];

  const dxS = snapMoveDeltaS(snapshot, currentTime, drag, pps);
  const primaryTrack = snapshot.tracks[drag.trackIndex];
  const primary = primaryTrack?.items.find(
    (item): item is Extract<TimelineItem, { kind: "clip" }> =>
      item.kind === "clip" && item.index === drag.clipIndex,
  );
  if (!primary) return [];

  const movingClips =
    drag.linkGroupId !== null
      ? snapshot.tracks.flatMap((track, trackIndex) =>
          track.items
            .filter(
              (item): item is Extract<TimelineItem, { kind: "clip" }> =>
                item.kind === "clip" && item.link_group_id === drag.linkGroupId,
            )
            .map((item) => ({ trackIndex, item })),
        )
      : [{ trackIndex: drag.trackIndex, item: primary }];
  const seen = new Set<string>();
  return movingClips
    .filter(({ item }) => {
      if (seen.has(item.clip_uuid)) return false;
      seen.add(item.clip_uuid);
      return true;
    })
    .map(({ trackIndex, item }) => {
      const track = snapshot.tracks[trackIndex];
      const targetStartS = item.track_start_s + dxS;
      const toPosition = targetPositionForMove(track.items, item.index, targetStartS);
      return {
        kind: "move_clip" as const,
        anchor: { kind: "clip_uuid" as const, uuid: item.clip_uuid },
        toPosition,
        atS: Math.max(0, targetStartS),
        fromPosition: item.index,
        fromAtS: item.track_start_s,
      };
    })
    .filter(shouldKeepMoveDraft)
    .map(({ fromPosition: _fromPosition, fromAtS: _fromAtS, ...op }) => op);
}
