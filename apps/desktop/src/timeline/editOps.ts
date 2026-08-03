import type { SelectedClipKey } from "../properties/store.ts";
import type { EdlOp } from "./edlBuilder.ts";
import {
  snapMoveDeltaS,
  type UserMoveDrag,
  type UserRollDrag,
  type UserTrimDrag,
} from "./editMath.ts";
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
  if (drag.ripple) {
    // Ripple trim emits a single backend op that trims the edge AND
    // shifts every downstream clip on the track by the delta.
    return [
      {
        kind: "ripple_trim",
        anchor: { kind: "clip_uuid", uuid: hit.clipUuid },
        edge: hit.side === "start" ? "start" : "end",
        valueS: hit.side === "start" ? newStart : newEnd,
      },
    ];
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

/** Build a ripple-delete envelope from a selection. Sends one
 *  `ripple_delete` per distinct clip; link-group siblings ride along
 *  inside the backend op. Unlike `buildDeleteSelectionOps`, this
 *  closes the gap left behind. */
export function buildRippleDeleteOps(
  snapshot: TimelineSnapshot,
  selectedClipKey: SelectedClipKey | null,
): EdlOp[] {
  if (!selectedClipKey) return [];
  const selectedTrack = snapshot.tracks[selectedClipKey.trackIndex];
  const selectedItem = selectedTrack?.items.find(
    (item) => item.index === selectedClipKey.clipIndex,
  );
  if (!selectedTrack || !selectedItem || selectedItem.kind !== "clip") return [];
  return [
    {
      kind: "ripple_delete",
      anchor: { kind: "clip_uuid", uuid: selectedItem.clip_uuid },
    },
  ];
}

/** Build a source-time split for the selected clip at a timeline playhead. */
export function buildSplitSelectionOps(
  snapshot: TimelineSnapshot,
  selectedClipKey: SelectedClipKey | null,
  playheadS: number,
): EdlOp[] {
  if (!selectedClipKey || !Number.isFinite(playheadS)) return [];
  const selectedTrack = snapshot.tracks[selectedClipKey.trackIndex];
  const selectedItem = selectedTrack?.items.find(
    (item) => item.index === selectedClipKey.clipIndex,
  );
  if (!selectedItem || selectedItem.kind !== "clip") return [];

  const timelineStart = selectedItem.track_start_s;
  const timelineEnd = timelineStart + selectedItem.duration_s;
  if (playheadS <= timelineStart + 0.01 || playheadS >= timelineEnd - 0.01) return [];

  const speed = selectedItem.speed ?? 1;
  if (!Number.isFinite(speed) || speed <= 0) return [];
  const sourceTime = (selectedItem.source_start_s ?? 0) + (playheadS - timelineStart) * speed;
  return [{
    kind: "split_clip",
    anchor: { kind: "clip_uuid", uuid: selectedItem.clip_uuid },
    atS: sourceTime,
  }];
}

/** Roll-edit op-builder. Returns a single
 *  ProfessionalTimelineEdit::RollEdit envelope sized by the drag's
 *  pixel delta converted to seconds. */
export function buildRollDragOps(drag: UserRollDrag, pps: number): EdlOp[] {
  const dxPx = drag.currentX - drag.startX;
  if (Math.abs(dxPx) < 2) return [];
  const dxS = pxDeltaToSourceDelta(dxPx, pps);
  if (Math.abs(dxS) < 0.01) return [];
  return [
    {
      kind: "professional_timeline_edit",
      edit: {
        edit: "roll_edit",
        between: {
          from: { kind: "clip_uuid", uuid: drag.hit.from.clipUuid },
          to: { kind: "clip_uuid", uuid: drag.hit.to.clipUuid },
        },
        delta_s: dxS,
      },
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

  // Slip and slide don't snap to neighbor edges — the dragged pixel
  // delta is the intent. Ripple keeps neighbor-edge snap so the drop
  // lands cleanly on adjacent cuts.
  const dxS =
    drag.mode === "ripple"
      ? snapMoveDeltaS(snapshot, currentTime, drag, pps)
      : pxDeltaToSourceDelta(dxPx, pps);
  if (Math.abs(dxS) < 0.01) return [];

  const primaryTrack = snapshot.tracks[drag.trackIndex];
  const primary = primaryTrack?.items.find(
    (item): item is Extract<TimelineItem, { kind: "clip" }> =>
      item.kind === "clip" && item.index === drag.clipIndex,
  );
  if (!primary) return [];

  if (drag.mode === "slip") {
    return [
      {
        kind: "professional_timeline_edit",
        edit: {
          edit: "slip_clip",
          anchor: { kind: "clip_uuid", uuid: primary.clip_uuid },
          delta_s: dxS,
        },
      },
    ];
  }
  if (drag.mode === "slide") {
    return [
      {
        kind: "professional_timeline_edit",
        edit: {
          edit: "slide_clip",
          anchor: { kind: "clip_uuid", uuid: primary.clip_uuid },
          delta_s: dxS,
        },
      },
    ];
  }

  // Ripple (default): backend shifts the moved clip + every clip
  // after it on its track, plus link-group siblings on other tracks
  // by the same delta. Matches Resolve/Premiere default body drag.
  return [
    {
      kind: "ripple_move",
      anchor: { kind: "clip_uuid", uuid: primary.clip_uuid },
      deltaS: dxS,
    },
  ];
}
