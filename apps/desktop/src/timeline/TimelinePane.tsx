// Bottom-row timeline pane. Read-only in this step: renders OTIO
// clips as horizontal rectangles per track, draws a time ruler at
// the top and a playhead synced to the media pane's currentTime.
// Refreshes when the project changes or when an apply_edl tool
// call lands in chat (the agent just rewrote the OTIO).

import { useEffect, useRef, useState } from "react";
import { useTimelineStore, type TimelineItem, type TimelineSnapshot } from "./store";
import { useMediaStore } from "../media/store";
import { useAgentStore } from "../agent/store";
import { useProjectStore } from "../app/state";
import { useProposalStore } from "./proposal";
import { ProposalActions } from "./ProposalActions";
import { ProposalHandles } from "./ProposalHandles";
import type { AppliedDiff } from "../protocol";

/** Pixels-per-second at zoom=1. Tuned so a 60s project fits the
 *  default pane width without horizontal scroll. */
const PX_PER_SECOND_BASE = 12;

/** Height of one track lane in pixels. */
const LANE_HEIGHT = 38;

/** Height of the time ruler at the top of the canvas. */
const RULER_HEIGHT = 22;

/** Padding inside each clip block. */
const CLIP_PADDING_X = 6;

export function TimelinePane() {
  const projectReady = useProjectStore((s) => s.current !== null);
  const snapshot = useTimelineStore((s) => s.snapshot);
  const refresh = useTimelineStore((s) => s.refresh);
  const items = useAgentStore((s) => s.items);
  const currentTime = useMediaStore((s) => s.currentTime);

  // Refresh on mount + on project change.
  useEffect(() => {
    if (projectReady) {
      refresh();
    }
  }, [projectReady, refresh]);

  // Refresh after every completed apply_edl. The OTIO file just
  // changed; the canvas should reflect it. We watch the count of
  // completed apply_edl tool calls (a stable scalar) rather than
  // the items array (changes on every text delta).
  const completedEdits = items.filter(
    (it) =>
      it.kind === "tool_call" &&
      it.name === "apply_edl" &&
      it.phase === "completed",
  ).length;
  useEffect(() => {
    if (projectReady && completedEdits > 0) {
      refresh();
    }
  }, [completedEdits, projectReady, refresh]);

  if (!projectReady) {
    return null;
  }

  return (
    <section className="timeline-pane">
      <header className="timeline-header">
        <span className="timeline-label">Timeline</span>
        <span className="timeline-meta">
          {snapshot.tracks.length === 0
            ? "no tracks yet"
            : `${snapshot.duration_s.toFixed(1)}s · ${snapshot.tracks.length} track${snapshot.tracks.length === 1 ? "" : "s"}`}
        </span>
      </header>
      <div className="timeline-stage">
        <TimelineSurface snapshot={snapshot} currentTime={currentTime} />
        <ProposalActions />
      </div>
    </section>
  );
}

/** Wrapper that owns layout state (pps, width) so the canvas can
 *  publish it on each paint and the handles can subscribe. Avoids
 *  recomputing layout in two places. */
function TimelineSurface({
  snapshot,
  currentTime,
}: {
  snapshot: TimelineSnapshot;
  currentTime: number;
}) {
  const [layout, setLayout] = useState<{ pps: number; width: number }>({
    pps: PX_PER_SECOND_BASE,
    width: 0,
  });
  return (
    <>
      <TimelineCanvas
        snapshot={snapshot}
        currentTime={currentTime}
        onLayout={(pps, width) => {
          // Only update if it actually changed — paint() runs on
          // every frame React re-renders, but layout changes only
          // on resize / snapshot swap.
          setLayout((prev) =>
            prev.pps === pps && prev.width === width ? prev : { pps, width },
          );
        }}
      />
      <ProposalHandles containerWidth={layout.width} pps={layout.pps} />
    </>
  );
}

function TimelineCanvas({
  snapshot,
  currentTime,
  onLayout,
}: {
  snapshot: TimelineSnapshot;
  currentTime: number;
  onLayout: (pps: number, widthPx: number) => void;
}) {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const containerRef = useRef<HTMLDivElement | null>(null);
  // Latest pps used for paint, captured into a ref so click handlers
  // can convert x → seconds without recomputing layout.
  const ppsRef = useRef<number>(PX_PER_SECOND_BASE);
  const requestSeek = useMediaStore((s) => s.requestSeek);
  const proposal = useProposalStore((s) => s.active);

  // Compute pixel layout. When a proposal is active, the canvas
  // paints two passes: original snapshot at α=0.45 (the "before")
  // and the proposed snapshot at α=1.0 with diff-hint coloring
  // (the "after"). pps is sized by the larger of the two durations
  // so both fit; otherwise the post-state would clip if it grew.
  useEffect(() => {
    const canvas = canvasRef.current;
    const container = containerRef.current;
    if (!canvas || !container) return;

    function paint() {
      if (!canvas || !container) return;
      const dpr = window.devicePixelRatio || 1;
      const cssWidth = container.clientWidth;
      // Use the wider snapshot for layout — picking max of
      // current/proposed track counts keeps the canvas tall
      // enough that no track gets clipped.
      const proposedTrackCount = proposal?.snapshot.tracks.length ?? 0;
      const lanesCount = Math.max(
        snapshot.tracks.length,
        proposedTrackCount,
        1,
      );
      const cssHeight = RULER_HEIGHT + lanesCount * LANE_HEIGHT;

      canvas.width = Math.floor(cssWidth * dpr);
      canvas.height = Math.floor(cssHeight * dpr);
      canvas.style.width = `${cssWidth}px`;
      canvas.style.height = `${cssHeight}px`;

      const ctx = canvas.getContext("2d");
      if (!ctx) return;
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
      ctx.clearRect(0, 0, cssWidth, cssHeight);

      // Pps from the max of current vs proposed durations so the
      // whole post-state fits even when a proposal extends past
      // the original.
      const proposedDuration = proposal?.snapshot.duration_s ?? 0;
      const totalDuration = Math.max(snapshot.duration_s, proposedDuration);
      const pps = computePps(totalDuration, cssWidth);
      ppsRef.current = pps;
      onLayout(pps, cssWidth);

      drawRuler(ctx, cssWidth, totalDuration, pps);

      if (proposal) {
        // Pass A — current state under, dimmed, with delete strike.
        const deletedKeys = collectDeletedKeys(proposal.diffHints);
        ctx.globalAlpha = 0.45;
        drawTracks(ctx, cssWidth, snapshot.tracks, pps, {
          deletedKeys,
        });
        ctx.globalAlpha = 1.0;
        // Pass B — proposed state on top, full opacity, with
        // diff-hint highlights for trimmed/inserted/split items.
        const highlightKeys = collectHighlightKeys(proposal.diffHints);
        drawTracks(ctx, cssWidth, proposal.snapshot.tracks, pps, {
          highlightKeys,
        });
      } else {
        drawTracks(ctx, cssWidth, snapshot.tracks, pps, {});
      }

      drawPlayhead(ctx, cssWidth, cssHeight, currentTime, pps);
    }

    paint();

    // Repaint on resize. ResizeObserver is the right tool — covers
    // window resize AND parent flex/grid resizes.
    const ro = new ResizeObserver(() => paint());
    ro.observe(container);
    return () => ro.disconnect();
  }, [snapshot, currentTime, proposal, onLayout]);

  // Click + drag on the canvas → seek the player. We use pointer
  // events (covers mouse + trackpad + touch) and capture the
  // pointer on mousedown so the drag tracks even outside the
  // canvas bounds (Premiere/Resolve behavior).
  function timeFromClientX(clientX: number): number {
    const canvas = canvasRef.current;
    if (!canvas) return 0;
    const rect = canvas.getBoundingClientRect();
    const x = Math.max(0, Math.min(rect.width, clientX - rect.left));
    const t = x / Math.max(0.001, ppsRef.current);
    // Clamp to project duration so we don't ask the player to seek
    // past the end (HTMLMediaElement clamps anyway, but this keeps
    // the visual feedback clean).
    return Math.max(0, Math.min(t, snapshot.duration_s || t));
  }

  function onPointerDown(e: React.PointerEvent<HTMLCanvasElement>) {
    if (snapshot.duration_s <= 0) return; // nothing to seek into
    e.currentTarget.setPointerCapture(e.pointerId);
    requestSeek(timeFromClientX(e.clientX));
  }

  function onPointerMove(e: React.PointerEvent<HTMLCanvasElement>) {
    if (!e.currentTarget.hasPointerCapture(e.pointerId)) return;
    requestSeek(timeFromClientX(e.clientX));
  }

  function onPointerUp(e: React.PointerEvent<HTMLCanvasElement>) {
    if (e.currentTarget.hasPointerCapture(e.pointerId)) {
      e.currentTarget.releasePointerCapture(e.pointerId);
    }
  }

  return (
    <div className="timeline-canvas-wrap" ref={containerRef}>
      {snapshot.tracks.length === 0 && (
        <div className="timeline-empty">
          No clips on the timeline yet — ask the agent for an edit
          ("trim filler", "cut to the punchline") and they'll show up here.
        </div>
      )}
      <canvas
        ref={canvasRef}
        className="timeline-canvas"
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={onPointerUp}
        onPointerCancel={onPointerUp}
      />
    </div>
  );
}

/** pps: fit the whole project to the available width if there are
 *  clips; otherwise fall back to the base for the empty ruler. The
 *  upper bound (8× base) prevents a 2-second project from drawing
 *  ridiculous spacing. */
function computePps(durationS: number, cssWidth: number): number {
  const fitPps = durationS > 0 ? (cssWidth - 8) / durationS : PX_PER_SECOND_BASE;
  return Math.max(2, Math.min(fitPps, PX_PER_SECOND_BASE * 8));
}

/** Draw the time ruler with tick marks every 1, 5, or 10 seconds
 *  depending on zoom. Larger ticks are labeled. */
function drawRuler(
  ctx: CanvasRenderingContext2D,
  width: number,
  duration: number,
  pps: number,
) {
  ctx.fillStyle = "#161b22";
  ctx.fillRect(0, 0, width, RULER_HEIGHT);
  ctx.strokeStyle = "#30363d";
  ctx.beginPath();
  ctx.moveTo(0, RULER_HEIGHT - 0.5);
  ctx.lineTo(width, RULER_HEIGHT - 0.5);
  ctx.stroke();

  // Choose a tick interval that gives ~1 tick every 60-80px.
  const desiredPx = 64;
  const candidates = [0.5, 1, 2, 5, 10, 30, 60, 120, 300];
  let interval =
    candidates.find((c) => c * pps >= desiredPx) ?? candidates[candidates.length - 1];

  ctx.fillStyle = "#8b949e";
  ctx.font =
    "11px ui-monospace, SFMono-Regular, 'SF Mono', Menlo, monospace";
  ctx.textBaseline = "middle";

  for (let t = 0; t <= duration + interval; t += interval) {
    const x = Math.round(t * pps) + 0.5;
    if (x > width) break;
    ctx.strokeStyle = "#30363d";
    ctx.beginPath();
    ctx.moveTo(x, RULER_HEIGHT - 8);
    ctx.lineTo(x, RULER_HEIGHT);
    ctx.stroke();
    ctx.fillText(formatTime(t), x + 4, RULER_HEIGHT / 2 - 1);
  }
}

/** Draw all tracks. `opts` carries optional diff-hint key sets:
 *  `deletedKeys` items get strike-through styling; `highlightKeys`
 *  items get an accent ring. Keys are `"${trackIdx}:${itemIdx}"`. */
function drawTracks(
  ctx: CanvasRenderingContext2D,
  width: number,
  tracks: { kind: string; items: TimelineItem[] }[],
  pps: number,
  opts: { deletedKeys?: Set<string>; highlightKeys?: Set<string> },
) {
  for (let row = 0; row < tracks.length; row++) {
    const track = tracks[row];
    const y = RULER_HEIGHT + row * LANE_HEIGHT;

    // Lane background — subtle alternating tint for video vs audio.
    ctx.fillStyle = track.kind === "audio" ? "#0d1117" : "#0f141b";
    ctx.fillRect(0, y, width, LANE_HEIGHT);
    ctx.strokeStyle = "#30363d";
    ctx.beginPath();
    ctx.moveTo(0, y + LANE_HEIGHT - 0.5);
    ctx.lineTo(width, y + LANE_HEIGHT - 0.5);
    ctx.stroke();

    for (const item of track.items) {
      const x = Math.round(item.track_start_s * pps);
      const w = Math.max(2, Math.round(item.duration_s * pps));
      const key = `${row}:${item.index}`;
      const flag =
        opts.deletedKeys?.has(key)
          ? "deleted"
          : opts.highlightKeys?.has(key)
          ? "highlight"
          : "normal";
      drawItem(ctx, item, x, y + 4, w, LANE_HEIGHT - 8, track.kind, flag);
    }
  }
}

type ItemFlag = "normal" | "deleted" | "highlight";

function drawItem(
  ctx: CanvasRenderingContext2D,
  item: TimelineItem,
  x: number,
  y: number,
  w: number,
  h: number,
  trackKind: string,
  flag: ItemFlag,
) {
  const radius = 4;
  if (item.kind === "clip") {
    ctx.fillStyle = trackKind === "audio" ? "#1f4d3f" : "#1f3d5d";
    fillRoundedRect(ctx, x, y, w, h, radius);
    // Border color: red for deletes (this clip is going away),
    // amber for highlights (this clip is changing in the
    // proposal), normal accent otherwise.
    const stroke =
      flag === "deleted"
        ? "#f85149"
        : flag === "highlight"
        ? "#d29922"
        : trackKind === "audio"
        ? "#3fb950"
        : "#58a6ff";
    ctx.strokeStyle = stroke;
    ctx.lineWidth = flag === "normal" ? 1 : 2;
    strokeRoundedRect(ctx, x + 0.5, y + 0.5, w - 1, h - 1, radius);
    ctx.lineWidth = 1;
    // Clip label — centered, truncated if width too small.
    if (w > 24) {
      ctx.fillStyle = "#e6edf3";
      ctx.font = "11px ui-sans-serif, system-ui, sans-serif";
      ctx.textBaseline = "middle";
      const label = truncateToWidth(ctx, item.name, w - 2 * CLIP_PADDING_X);
      ctx.fillText(label, x + CLIP_PADDING_X, y + h / 2);
    }
    if (flag === "deleted") {
      // Strike-through line so the "before" is visually marked
      // for deletion even at low contrast.
      ctx.strokeStyle = "#f85149";
      ctx.lineWidth = 1.5;
      ctx.beginPath();
      ctx.moveTo(x + 2, y + h / 2);
      ctx.lineTo(x + w - 2, y + h / 2);
      ctx.stroke();
      ctx.lineWidth = 1;
    }
  } else if (item.kind === "gap") {
    ctx.fillStyle = "rgba(139, 148, 158, 0.12)";
    fillRoundedRect(ctx, x, y, w, h, radius);
    // Cross-hatch pattern feel via dashed border so gaps stand out.
    ctx.strokeStyle = "#30363d";
    ctx.setLineDash([3, 3]);
    strokeRoundedRect(ctx, x + 0.5, y + 0.5, w - 1, h - 1, radius);
    ctx.setLineDash([]);
  } else {
    // transition
    ctx.fillStyle = "rgba(210, 153, 34, 0.18)";
    fillRoundedRect(ctx, x, y, w, h, radius);
    ctx.strokeStyle = "#d29922";
    strokeRoundedRect(ctx, x + 0.5, y + 0.5, w - 1, h - 1, radius);
  }
}

/** Build the set of `${trackIdx}:${itemIdx}` keys whose items in the
 *  *original* snapshot are being deleted by the proposal. */
function collectDeletedKeys(diffs: AppliedDiff[]): Set<string> {
  const out = new Set<string>();
  for (const d of diffs) {
    if (d.kind === "delete") {
      out.add(`${d.track_index}:${d.item_index}`);
    }
  }
  return out;
}

/** Build the set of `${trackIdx}:${itemIdx}` keys whose items in the
 *  *proposed* snapshot are being changed (trimmed, split, inserted). */
function collectHighlightKeys(diffs: AppliedDiff[]): Set<string> {
  const out = new Set<string>();
  for (const d of diffs) {
    if (d.kind === "trim_edge") {
      out.add(`${d.track_index}:${d.item_index}`);
    } else if (d.kind === "split") {
      // Both halves of a split are "new" relative to the original.
      out.add(`${d.track_index}:${d.item_index}`);
      out.add(`${d.track_index}:${d.item_index + 1}`);
    } else if (d.kind === "insert") {
      out.add(`${d.track_index}:${d.item_index}`);
    }
  }
  return out;
}

function drawPlayhead(
  ctx: CanvasRenderingContext2D,
  width: number,
  height: number,
  currentTime: number,
  pps: number,
) {
  const x = Math.round(currentTime * pps) + 0.5;
  if (x < 0 || x > width) return;
  ctx.strokeStyle = "#f85149";
  ctx.lineWidth = 1.5;
  ctx.beginPath();
  ctx.moveTo(x, 0);
  ctx.lineTo(x, height);
  ctx.stroke();
  // Triangle handle at top.
  ctx.fillStyle = "#f85149";
  ctx.beginPath();
  ctx.moveTo(x - 5, 0);
  ctx.lineTo(x + 5, 0);
  ctx.lineTo(x, 6);
  ctx.closePath();
  ctx.fill();
  ctx.lineWidth = 1;
}

function fillRoundedRect(
  ctx: CanvasRenderingContext2D,
  x: number,
  y: number,
  w: number,
  h: number,
  r: number,
) {
  pathRoundedRect(ctx, x, y, w, h, r);
  ctx.fill();
}

function strokeRoundedRect(
  ctx: CanvasRenderingContext2D,
  x: number,
  y: number,
  w: number,
  h: number,
  r: number,
) {
  pathRoundedRect(ctx, x, y, w, h, r);
  ctx.stroke();
}

function pathRoundedRect(
  ctx: CanvasRenderingContext2D,
  x: number,
  y: number,
  w: number,
  h: number,
  r: number,
) {
  const rr = Math.min(r, w / 2, h / 2);
  ctx.beginPath();
  ctx.moveTo(x + rr, y);
  ctx.lineTo(x + w - rr, y);
  ctx.quadraticCurveTo(x + w, y, x + w, y + rr);
  ctx.lineTo(x + w, y + h - rr);
  ctx.quadraticCurveTo(x + w, y + h, x + w - rr, y + h);
  ctx.lineTo(x + rr, y + h);
  ctx.quadraticCurveTo(x, y + h, x, y + h - rr);
  ctx.lineTo(x, y + rr);
  ctx.quadraticCurveTo(x, y, x + rr, y);
}

function truncateToWidth(
  ctx: CanvasRenderingContext2D,
  text: string,
  maxWidth: number,
): string {
  if (ctx.measureText(text).width <= maxWidth) return text;
  // Binary-search down. For the widths we deal with (clip names) a
  // linear walk is fine, but binary is cheap.
  let lo = 0;
  let hi = text.length;
  while (lo < hi) {
    const mid = Math.ceil((lo + hi) / 2);
    if (ctx.measureText(text.slice(0, mid) + "…").width <= maxWidth) {
      lo = mid;
    } else {
      hi = mid - 1;
    }
  }
  return lo > 0 ? text.slice(0, lo) + "…" : "";
}

function formatTime(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds < 0) return "0:00";
  const m = Math.floor(seconds / 60);
  const s = Math.floor(seconds % 60);
  return `${m}:${s.toString().padStart(2, "0")}`;
}
