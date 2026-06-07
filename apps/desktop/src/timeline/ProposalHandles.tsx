// Drag handles for a proposal's adjustable knobs. Sibling overlay
// of the canvas — same coordinate space, but as absolute-positioned
// divs so we get cursor: ew-resize, hover, focus, and accessibility
// for free.
//
// Each handle corresponds to one AppliedDiff entry. On pointer-down
// we capture the pointer, on pointer-move we draw a local visual
// offset, on pointer-up we fire `adjust_proposal` with the new
// `value_s`. The backend re-runs apply, emits a Delta with bumped
// revision, the proposal store rebuilds — and on the next frame
// the canvas + these handles repaint at the new x.

import { useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useProposalStore } from "./proposal";
import type { AppliedDiff, AdjustField, TimelineSnapshot } from "../protocol";

/** Layout constants — must match TimelinePane.tsx exactly. The
 *  handles overlay the canvas; same ruler offset is baked into both.
 *  Lane height is supplied as a prop so vertical zoom flows through. */
const RULER_HEIGHT = 22;
const HANDLE_WIDTH = 8;
const PX_PER_SECOND_BASE = 12;

type HandleSpec = {
  key: string;
  /** Pixel center-x of the handle (CSS px, not device px). */
  x: number;
  /** Pixel top of the handle. */
  y: number;
  /** Pixel height of the handle. */
  h: number;
  /** Tooltip label shown on hover / drag. */
  label: string;
  /** Adjustment to fire on pointer-up, given the dragged-to seconds. */
  buildAdjustment: (newValueS: number) => {
    op_index: number;
    field: AdjustField;
    value_s: number;
  };
};

export function ProposalHandles({
  containerWidth,
  pps,
  laneHeight,
}: {
  /** Width of the canvas in CSS pixels — handles position relative to it. */
  containerWidth: number;
  /** Same pps the canvas uses; passed in so we don't recompute layout. */
  pps: number;
  /** Same lane height the canvas uses, in CSS pixels. */
  laneHeight: number;
}) {
  const proposal = useProposalStore((s) => s.active);

  if (!proposal) return null;

  const handles = buildHandles(proposal.snapshot, proposal.diffHints, pps, laneHeight);

  return (
    <div className="proposal-handles" style={{ width: containerWidth }}>
      {handles.map((h) => (
        <Handle key={h.key} spec={h} pps={pps} callId={proposal.callId} />
      ))}
    </div>
  );
}

function Handle({
  spec,
  pps,
  callId,
}: {
  spec: HandleSpec;
  pps: number;
  callId: string;
}) {
  const [dragOffset, setDragOffset] = useState(0);
  const [tooltip, setTooltip] = useState<string | null>(null);
  const startXRef = useRef<number>(0);

  function onPointerDown(e: React.PointerEvent<HTMLDivElement>) {
    e.currentTarget.setPointerCapture(e.pointerId);
    e.preventDefault();
    e.stopPropagation();
    startXRef.current = e.clientX;
    setDragOffset(0);
    setTooltip(spec.label);
  }

  function onPointerMove(e: React.PointerEvent<HTMLDivElement>) {
    if (!e.currentTarget.hasPointerCapture(e.pointerId)) return;
    const dx = e.clientX - startXRef.current;
    setDragOffset(dx);
    // Live tooltip — convert visual offset to seconds for feedback.
    const dt = dx / Math.max(0.001, pps);
    setTooltip(`${spec.label} ${dt >= 0 ? "+" : ""}${dt.toFixed(2)}s`);
  }

  function onPointerUp(e: React.PointerEvent<HTMLDivElement>) {
    if (!e.currentTarget.hasPointerCapture(e.pointerId)) return;
    e.currentTarget.releasePointerCapture(e.pointerId);
    const finalDx = e.clientX - startXRef.current;
    const baseSeconds = spec.x / Math.max(0.001, pps);
    const newValueS = Math.max(0, baseSeconds + finalDx / Math.max(0.001, pps));
    setDragOffset(0);
    setTooltip(null);
    const adj = spec.buildAdjustment(newValueS);
    invoke("adjust_proposal", {
      callId,
      adjustments: [adj],
    }).catch((err) => {
      // eslint-disable-next-line no-console
      console.warn("adjust_proposal failed", err);
    });
  }

  return (
    <div
      className="proposal-handle"
      style={{
        left: spec.x + dragOffset - HANDLE_WIDTH / 2,
        top: spec.y,
        width: HANDLE_WIDTH,
        height: spec.h,
      }}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onPointerCancel={onPointerUp}
      title={tooltip ?? spec.label}
    >
      {tooltip && <div className="proposal-handle-tooltip">{tooltip}</div>}
    </div>
  );
}

/** For each draggable diff hint, compute its handle layout. */
function buildHandles(
  snapshot: TimelineSnapshot,
  diffs: AppliedDiff[],
  pps: number,
  laneHeight: number,
): HandleSpec[] {
  const out: HandleSpec[] = [];
  for (const diff of diffs) {
    const handle = handleForDiff(snapshot, diff, pps, laneHeight);
    if (handle) out.push(handle);
  }
  return out;
}

function handleForDiff(
  snapshot: TimelineSnapshot,
  diff: AppliedDiff,
  pps: number,
  laneHeight: number,
): HandleSpec | null {
  switch (diff.kind) {
    case "trim_edge": {
      const item = locateClipItem(snapshot, diff.track_index, diff.item_index);
      if (!item) return null;
      const xStart = item.track_start_s * pps;
      const xEnd = (item.track_start_s + item.duration_s) * pps;
      const x = diff.side === "left" ? xStart : xEnd;
      const y = RULER_HEIGHT + diff.track_index * laneHeight + 4;
      const h = laneHeight - 8;
      const field: AdjustField =
        diff.side === "left" ? "trim_start" : "trim_end";
      // The handle's "value" lives in source-media seconds, not
      // track seconds. We approximate via the clip's
      // source_start_s + the bound's distance from track_start_s.
      const sourceStart = item.source_start_s ?? 0;
      const sourceBound =
        diff.side === "left"
          ? sourceStart
          : sourceStart + item.duration_s;
      return {
        key: `trim-${diff.op_index}-${diff.side}`,
        x,
        y,
        h,
        label: diff.side === "left" ? "trim start" : "trim end",
        buildAdjustment: (_newTrackSeconds) => {
          // Pointer-up gives us a *track* time. Convert to source.
          // We assume linear mapping (no time stretch) — true for
          // every montage clip today.
          const trackBound =
            diff.side === "left"
              ? item.track_start_s
              : item.track_start_s + item.duration_s;
          const trackDelta = _newTrackSeconds - trackBound;
          const sourceValue = sourceBound + trackDelta;
          return {
            op_index: diff.op_index,
            field,
            value_s: Math.max(0, sourceValue),
          };
        },
      };
    }
    case "split": {
      const item = locateClipItem(snapshot, diff.track_index, diff.item_index);
      if (!item) return null;
      // Split anchor sits at the *end* of the left half on the
      // proposed snapshot. We render the handle at that x.
      const x = (item.track_start_s + item.duration_s) * pps;
      const y = RULER_HEIGHT + diff.track_index * laneHeight + 4;
      const h = laneHeight - 8;
      const sourceStart = item.source_start_s ?? 0;
      const sourceCut = sourceStart + item.duration_s;
      return {
        key: `split-${diff.op_index}`,
        x,
        y,
        h,
        label: "split at",
        buildAdjustment: (newTrackSeconds) => {
          const trackBound = item.track_start_s + item.duration_s;
          const trackDelta = newTrackSeconds - trackBound;
          const sourceValue = sourceCut + trackDelta;
          return {
            op_index: diff.op_index,
            field: "split_at",
            value_s: Math.max(0, sourceValue),
          };
        },
      };
    }
    case "insert":
      // Insert needs two handles (both edges); for v1 we render
      // just the right edge to keep the handle math simple. Left
      // edge could come later.
      return null;
    case "insert_b_roll":
    case "insert_pi_p":
    case "move":
      return null;
    case "delete":
      return null;
  }
}

function locateClipItem(
  snapshot: TimelineSnapshot,
  trackIndex: number,
  itemIndex: number,
): { track_start_s: number; duration_s: number; source_start_s: number | null } | null {
  const track = snapshot.tracks[trackIndex];
  if (!track) return null;
  const item = track.items[itemIndex];
  if (!item || item.kind !== "clip") return null;
  return {
    track_start_s: item.track_start_s,
    duration_s: item.duration_s,
    source_start_s: item.source_start_s,
  };
}

// Reserved for parent that needs to pass pps. Kept as an export
// stub so test imports stay stable.
export const _PX_PER_SECOND_BASE = PX_PER_SECOND_BASE;
