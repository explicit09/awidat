// Bottom-row timeline pane. Read-only in this step: renders OTIO
// clips as horizontal rectangles per track, draws a time ruler at
// the top and a playhead synced to the media pane's currentTime.
// Refreshes when the project changes or when an apply_edl tool
// call lands in chat (the agent just rewrote the OTIO).

import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useTimelineStore, type TimelineItem, type TimelineSnapshot } from "./store";
import { useMediaStore } from "../media/store";
import { useAgentStore } from "../agent/store";
import { useProjectStore } from "../app/state";
import { useProposalStore } from "./proposal";
import { ProposalActions } from "./ProposalActions";
import { ProposalHandles } from "./ProposalHandles";
import { TIMELINE_CHANGED_EVENT, type AppliedDiff } from "../protocol";
import { serializeEdl } from "./edlBuilder";
import {
  hitTestClipBody,
  hitTestEdge,
  pxDeltaToSourceDelta,
  type EdgeHit,
} from "./hitDetect";
import { getStrip, onThumbnailDecoded } from "./thumbnailCache";
import { getBuckets, onWaveformDecoded } from "./waveformCache";
import { useTimelineSelectionStore } from "../properties/store";

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
  const projectRoot = useProjectStore((s) => s.current);
  const snapshot = useTimelineStore((s) => s.snapshot);
  const refresh = useTimelineStore((s) => s.refresh);
  const items = useAgentStore((s) => s.items);
  // The canvas is a timeline-time surface; the playhead should track
  // the timeline-time clock the SegmentedVideoView drives, not the
  // source-time of whatever proxy happens to be loaded.
  const currentTime = useMediaStore((s) => s.timelineTime);

  // Refresh on mount + on project change.
  useEffect(() => {
    if (projectReady) {
      refresh();
    }
  }, [projectReady, projectRoot, refresh]);

  useEffect(() => {
    const unlisten = listen<string>(TIMELINE_CHANGED_EVENT, (event) => {
      if (useProjectStore.getState().current === event.payload) {
        refresh();
      }
    });
    return () => {
      unlisten.then((u) => u());
    };
  }, [refresh]);

  // Refresh after every completed apply_edl OR every completed
  // proposed_edit. Both paths can mutate the OTIO on disk:
  //   - apply_edl Completed lands when the agent's tool handler
  //     finishes (Allow path: agent wrote the file).
  //   - proposed_edit Completed lands when a proposal accept /
  //     reject finishes (Deny-with-adjustment path: desktop wrote
  //     the file, no agent tool ran; user-initiated edits via
  //     propose_user_edit also take this path).
  // We watch a stable scalar (count of completions) rather than
  // the full items array so React doesn't re-fire the effect on
  // every text delta.
  const completedEdits =
    items.filter(
      (it) =>
        it.kind === "tool_call" &&
        it.name === "apply_edl" &&
        it.phase === "completed",
    ).length +
    items.filter(
      (it) =>
        it.kind === "proposed_edit" &&
        it.phase === "completed" &&
        it.source.source === "user" &&
        it.snapshot.tracks.length > 0,
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
  const handleLayout = useCallback((pps: number, width: number) => {
    // Only update if it actually changed — paint() runs on
    // every frame React re-renders, but layout changes only
    // on resize / snapshot swap.
    setLayout((prev) =>
      prev.pps === pps && prev.width === width ? prev : { pps, width },
    );
  }, []);
  return (
    <>
      <TimelineCanvas
        snapshot={snapshot}
        currentTime={currentTime}
        onLayout={handleLayout}
      />
      <ProposalHandles containerWidth={layout.width} pps={layout.pps} />
    </>
  );
}

/** Active user-trim drag — lives in canvas state so cursor + paint
 *  update on edge-hover and during the drag. */
type UserTrimDrag = {
  hit: EdgeHit;
  /** Pointer x at drag start, in canvas-local pixels. */
  startX: number;
  /** Live pointer x in canvas-local pixels. */
  currentX: number;
};

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
  // Canvas seek + playhead use timeline-time. Single-asset / empty-
  // timeline mode keeps using source-time inside the MediaPane, but
  // the canvas itself is a timeline-time surface — clicking at x=80px
  // means "seek to ~5s of the timeline", not "5s of the source." With
  // a multi-clip timeline these two axes diverge; with a single-clip
  // timeline they coincide for now (until trim shifts source_start_s).
  const requestTimelineSeek = useMediaStore((s) => s.requestTimelineSeek);
  const proposal = useProposalStore((s) => s.active);
  // Cursor hint when hovering near a clip edge (without dragging).
  const [edgeHover, setEdgeHover] = useState<EdgeHit | null>(null);
  // Active drag, set on pointerdown-near-edge, cleared on pointerup.
  const [userTrim, setUserTrim] = useState<UserTrimDrag | null>(null);
  // Properties-pane selection: which clip is currently inspected.
  // The canvas paints a subtle amber outline on the selected clip
  // so the link between timeline selection and right-rail content
  // is visible.
  const selectedClipKey = useTimelineSelectionStore(
    (s) => s.selectedClipKey,
  );
  const selectClip = useTimelineSelectionStore((s) => s.select);
  const clearSelection = useTimelineSelectionStore((s) => s.clear);

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

      const selectedKey = selectedClipKey
        ? `${selectedClipKey.trackIndex}:${selectedClipKey.clipIndex}`
        : undefined;
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
        // Selection ring rides on the post-state pass so the user
        // sees which clip in the *new* timeline they're inspecting.
        const highlightKeys = collectHighlightKeys(proposal.diffHints);
        drawTracks(ctx, cssWidth, proposal.snapshot.tracks, pps, {
          highlightKeys,
          selectedKey,
        });
      } else {
        drawTracks(ctx, cssWidth, snapshot.tracks, pps, { selectedKey });
      }

      drawPlayhead(ctx, cssWidth, cssHeight, currentTime, pps);

      // Hover affordance — a faint amber outline on the edge under
      // the pointer when the user isn't yet dragging. Tells them
      // "yes, you can grab this" before they commit.
      if (edgeHover && !userTrim) {
        const item = snapshot.tracks[edgeHover.trackIndex]?.items.find(
          (it) => it.index === edgeHover.clipIndex,
        );
        if (item && item.kind === "clip") {
          const edgeX =
            edgeHover.side === "start"
              ? item.track_start_s * pps
              : (item.track_start_s + item.duration_s) * pps;
          const yTop = RULER_HEIGHT + edgeHover.trackIndex * LANE_HEIGHT + 4;
          ctx.fillStyle = "rgba(245, 158, 11, 0.55)";
          ctx.fillRect(edgeX - 1, yTop, 2, LANE_HEIGHT - 8);
        }
      }

      // Draw the live drag-edge phantom on top of everything else.
      // 2px amber line at the dragged x.
      if (userTrim) {
        const x = userTrim.currentX;
        const yTop = RULER_HEIGHT;
        const yBot = RULER_HEIGHT + LANE_HEIGHT * snapshot.tracks.length;
        ctx.fillStyle = "#f59e0b";
        ctx.fillRect(x - 1, yTop, 2, yBot - yTop);
      }
    }

    paint();

    // Repaint on resize. ResizeObserver is the right tool — covers
    // window resize AND parent flex/grid resizes.
    const ro = new ResizeObserver(() => paint());
    ro.observe(container);
    // Repaint when a thumbnail or waveform finishes decoding so the
    // strip / amplitude line populate as their data loads.
    const unsubThumb = onThumbnailDecoded(() => paint());
    const unsubWave = onWaveformDecoded(() => paint());
    return () => {
      ro.disconnect();
      unsubThumb();
      unsubWave();
    };
  }, [
    snapshot,
    currentTime,
    proposal,
    onLayout,
    userTrim,
    edgeHover,
    selectedClipKey,
  ]);

  // Pointer dispatch:
  //   - On a clip edge → start user-trim drag
  //   - Elsewhere → seek-on-drag (existing behavior)
  // We use pointer events (covers mouse + trackpad + touch) and
  // capture the pointer on mousedown so the drag tracks even outside
  // the canvas bounds (Premiere/Resolve behavior).
  function canvasPos(e: React.PointerEvent<HTMLCanvasElement>): {
    x: number;
    y: number;
    clientX: number;
  } {
    const rect = e.currentTarget.getBoundingClientRect();
    return {
      x: Math.max(0, Math.min(rect.width, e.clientX - rect.left)),
      y: Math.max(0, Math.min(rect.height, e.clientY - rect.top)),
      clientX: e.clientX,
    };
  }

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
    if (snapshot.duration_s <= 0) return; // nothing to interact with
    // Don't start a new user-trim while a proposal is already on
    // screen — it would be confusing to have two "ghost" overlays.
    // The user can accept/reject the active proposal first.
    if (proposal) {
      e.currentTarget.setPointerCapture(e.pointerId);
      requestTimelineSeek(timeFromClientX(e.clientX));
      return;
    }
    const { x, y, clientX } = canvasPos(e);
    const hit = hitTestEdge(x, y, snapshot, ppsRef.current);
    if (hit) {
      e.currentTarget.setPointerCapture(e.pointerId);
      setUserTrim({ hit, startX: x, currentX: x });
      // Don't seek on edge-down; the user is starting a trim, not
      // scrubbing.
      return;
    }
    // No edge hit — update the properties-pane selection. Clip body
    // under the pointer = select; empty space = clear. Either way
    // the click also scrubs the playhead (preserving the existing
    // seek-on-click behaviour).
    const body = hitTestClipBody(x, y, snapshot, ppsRef.current);
    if (body) {
      selectClip(body);
    } else {
      clearSelection();
    }
    e.currentTarget.setPointerCapture(e.pointerId);
    requestTimelineSeek(timeFromClientX(clientX));
  }

  function onPointerMove(e: React.PointerEvent<HTMLCanvasElement>) {
    const { x, y, clientX } = canvasPos(e);
    if (userTrim) {
      // Active drag — update the phantom edge x.
      setUserTrim({ ...userTrim, currentX: x });
      return;
    }
    // Hover state — update edge cursor hint.
    const hover = hitTestEdge(x, y, snapshot, ppsRef.current);
    setEdgeHover(hover);
    if (e.currentTarget.hasPointerCapture(e.pointerId)) {
      requestTimelineSeek(timeFromClientX(clientX));
    }
  }

  function onPointerUp(e: React.PointerEvent<HTMLCanvasElement>) {
    if (e.currentTarget.hasPointerCapture(e.pointerId)) {
      e.currentTarget.releasePointerCapture(e.pointerId);
    }
    if (userTrim) {
      void commitUserTrim(userTrim);
      setUserTrim(null);
    }
  }

  // Build a one-op TrimClip envelope from the drag state and submit
  // it via propose_user_edit. The Step-5 proposal pipeline takes
  // over: the ghost overlay shows the proposed cut, Accept commits
  // it. Cancellation while dragging (pointercancel / pointerup with
  // no movement) just clears userTrim — no envelope is sent.
  async function commitUserTrim(drag: UserTrimDrag): Promise<void> {
    const dxPx = drag.currentX - drag.startX;
    const dxS = pxDeltaToSourceDelta(dxPx, ppsRef.current);
    // Tiny drags are scrub mistakes, not trim intent. Don't send.
    if (Math.abs(dxPx) < 2) return;
    const { hit } = drag;
    let newStart = hit.sourceStart;
    let newEnd = hit.sourceEnd;
    if (hit.side === "start") {
      newStart = Math.max(0, hit.sourceStart + dxS);
      // Don't allow start to cross end — clamp to a minimal slice.
      if (newStart >= newEnd) newStart = newEnd - 0.1;
    } else {
      newEnd = Math.max(hit.sourceStart + 0.1, hit.sourceEnd + dxS);
    }
    // No-op if the edge didn't actually move (rounding hit zero).
    if (
      Math.abs(newStart - hit.sourceStart) < 0.01 &&
      Math.abs(newEnd - hit.sourceEnd) < 0.01
    ) {
      return;
    }
    const edl = serializeEdl([
      {
        kind: "trim_clip",
        anchor: { kind: "clip_uuid", uuid: hit.clipUuid },
        start: newStart !== hit.sourceStart ? newStart : undefined,
        end: newEnd !== hit.sourceEnd ? newEnd : undefined,
      },
    ]);
    try {
      await invoke<string>("propose_user_edit", { edlText: edl });
    } catch (err) {
      // Surface failures to the console; the existing
      // build_proposal-failed Item::Error path will also fire if the
      // backend rejects the envelope, so the user sees the reason
      // in chat.
      // eslint-disable-next-line no-console
      console.warn("propose_user_edit failed", err);
    }
  }

  function onPointerLeave() {
    setEdgeHover(null);
  }

  // CSS cursor depending on state. ew-resize over an edge or while
  // dragging; col-resize (timeline scrub) elsewhere; default outside
  // the canvas. The cursor shows up on the canvas style; setting it
  // via React style on the element is enough.
  const cursor = userTrim
    ? "ew-resize"
    : edgeHover
    ? "ew-resize"
    : snapshot.duration_s > 0
    ? "col-resize"
    : "default";

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
        style={{ cursor }}
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={onPointerUp}
        onPointerCancel={onPointerUp}
        onPointerLeave={onPointerLeave}
      />
      {userTrim && (
        <UserTrimTooltip drag={userTrim} pps={ppsRef.current} />
      )}
    </div>
  );
}

/** pps: fit the whole project to the available width if there are
 *  clips; otherwise fall back to the base for the empty ruler. The
 *  upper bound (8× base) prevents a 2-second project from drawing
 *  ridiculous spacing. */
/** Floating tooltip that follows the dragged edge during a user-trim
 *  drag, showing the proposed source-time. Positioned absolutely
 *  inside `.timeline-canvas-wrap`, anchored via `currentX` (canvas-
 *  local pixels). */
function UserTrimTooltip({
  drag,
  pps,
}: {
  drag: UserTrimDrag;
  pps: number;
}) {
  const dxPx = drag.currentX - drag.startX;
  const dxS = dxPx / Math.max(0.001, pps);
  const proposed =
    drag.hit.side === "start"
      ? Math.max(0, drag.hit.sourceStart + dxS)
      : Math.max(drag.hit.sourceStart + 0.1, drag.hit.sourceEnd + dxS);
  const label = `${drag.hit.side}: ${proposed.toFixed(2)}s`;
  // Live-drag tooltip — anchored to free coordinates (drag.currentX)
  // so it can't ride on Radix's hover-trigger positioning, but visual
  // language matches the awidat-tooltip surface so the affordance
  // reads consistently with the rest of the app's hover tooltips.
  return (
    <div
      className="user-trim-tooltip"
      style={{ left: drag.currentX }}
    >
      {label}
    </div>
  );
}

function computePps(durationS: number, cssWidth: number): number {
  const fitPps =
    durationS > 0 ? Math.max(0.05, (cssWidth - 8) / durationS) : PX_PER_SECOND_BASE;
  return Math.min(fitPps, PX_PER_SECOND_BASE * 8);
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
 *  items get an accent ring; `selectedKey` (a single key string) gets
 *  an amber selection outline drawn over everything else.
 *  Keys are `"${trackIdx}:${itemIdx}"`. */
function drawTracks(
  ctx: CanvasRenderingContext2D,
  width: number,
  tracks: { kind: string; role: string | null; items: TimelineItem[] }[],
  pps: number,
  opts: {
    deletedKeys?: Set<string>;
    highlightKeys?: Set<string>;
    selectedKey?: string;
  },
) {
  for (let row = 0; row < tracks.length; row++) {
    const track = tracks[row];
    const y = RULER_HEIGHT + row * LANE_HEIGHT;
    const isTitlesRow = track.role === "titles";

    // Lane background. Titles row gets a darker amber-tinted band
    // so it reads as a different kind of layer; audio is the
    // existing dark-green ish; video lanes keep their default tint.
    if (isTitlesRow) {
      ctx.fillStyle = "#0a0a0a";
    } else if (track.kind === "audio") {
      ctx.fillStyle = "#0d1117";
    } else {
      ctx.fillStyle = "#0f141b";
    }
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
      const selected = opts.selectedKey === key;
      drawItem(
        ctx,
        item,
        x,
        y + 4,
        w,
        LANE_HEIGHT - 8,
        track.kind,
        flag,
        selected,
        isTitlesRow,
      );
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
  selected: boolean,
  isTitlesRow: boolean,
) {
  const radius = 4;
  if (item.kind === "clip") {
    // Title clips on the Titles track get an amber-on-black band
    // with inline text rather than the regular media-clip styling.
    const isTitleClip = isTitlesRow && item.title !== null && item.title !== undefined;
    if (isTitleClip) {
      ctx.fillStyle = "#1a1207"; // dark amber-tinted background
    } else {
      ctx.fillStyle = trackKind === "audio" ? "#1f4d3f" : "#1f3d5d";
    }
    fillRoundedRect(ctx, x, y, w, h, radius);
    // Filmstrip / waveform: drawn on top of the coloured fill, under
    // the border. Video tracks get filmstrips, audio tracks get
    // waveforms — same "drew" boolean for the label dark band.
    // Titles skip both — the title text overlay takes the role of
    // the inline content.
    let drewOverlay = false;
    if (isTitleClip) {
      drawClipTitleText(ctx, item.title!, x, y, w, h);
      drewOverlay = true;
    } else if (trackKind !== "audio" && item.thumbnail_dir && w > 24) {
      drewOverlay = drawClipFilmstrip(ctx, item, x, y, w, h, radius);
    } else if (trackKind === "audio" && item.waveform_path && w > 24) {
      drewOverlay = drawClipWaveform(ctx, item, x, y, w, h, radius);
    }
    // Border color: red for deletes (this clip is going away),
    // amber for highlights (this clip is changing in the
    // proposal), normal accent otherwise. Title clips get an amber
    // border to match their warm fill.
    const stroke =
      flag === "deleted"
        ? "#f85149"
        : flag === "highlight"
        ? "#d29922"
        : isTitleClip
        ? "#f59e0b"
        : trackKind === "audio"
        ? "#3fb950"
        : "#58a6ff";
    ctx.strokeStyle = stroke;
    ctx.lineWidth = flag === "normal" ? 1 : 2;
    strokeRoundedRect(ctx, x + 0.5, y + 0.5, w - 1, h - 1, radius);
    ctx.lineWidth = 1;
    // Clip label — centered, truncated if width too small. When the
    // filmstrip / waveform drew, paint the label on a translucent
    // dark band so it stays legible over the overlay; otherwise plain.
    // Title clips skip this — drawClipTitleText already painted the
    // title text inline.
    if (w > 24 && !isTitleClip) {
      ctx.font = "11px ui-sans-serif, system-ui, sans-serif";
      ctx.textBaseline = "middle";
      const label = truncateToWidth(ctx, item.name, w - 2 * CLIP_PADDING_X);
      const labelY = y + h / 2;
      if (drewOverlay) {
        const metrics = ctx.measureText(label);
        ctx.fillStyle = "rgba(13, 17, 23, 0.7)";
        ctx.fillRect(
          x + CLIP_PADDING_X - 2,
          labelY - 7,
          Math.min(w - 2 * CLIP_PADDING_X + 4, metrics.width + 4),
          14,
        );
      }
      ctx.fillStyle = "#e6edf3";
      ctx.fillText(label, x + CLIP_PADDING_X, labelY);
    }
    // Volume / speed badges — painted in the top-right corner so they
    // don't fight the label for space. Only render when non-default.
    // Title clips skip badges (no volume/speed on titles).
    if (w > 36 && !isTitleClip) {
      drawClipBadges(ctx, item, x, y, w);
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
    if (selected) {
      // Outer amber ring to show "this clip is the inspector's
      // current target". Drawn after the regular border so it
      // wins z-order; stays visible when the clip is also a
      // proposal highlight (amber over amber is still distinct
      // because the proposal stroke is inside the rect, this
      // one straddles it).
      ctx.strokeStyle = "#f59e0b";
      ctx.lineWidth = 2;
      strokeRoundedRect(ctx, x - 0.5, y - 0.5, w + 1, h + 1, radius + 1);
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

/** Tile filmstrip JPEGs across a video clip's pixel area.
 *
 *  Returns true if at least one frame painted (so the caller knows
 *  to overlay the label on a dark band for legibility). Returns
 *  false when the cache is still listing or no frames are decoded
 *  yet — caller falls back to the plain coloured rect, which the
 *  fill above already painted.
 *
 *  Frame placement: the source range is `[source_start_s,
 *  source_start_s + duration_s]`. Frames are 1/sec, so the indices
 *  needed are roughly floor(source_start) … floor(source_start +
 *  duration). We pick `frames_to_show = round(w / 50)` evenly-spaced
 *  frames from that range and stretch them across `w` so each tile
 *  is `w / frames_to_show` wide. */
function drawClipFilmstrip(
  ctx: CanvasRenderingContext2D,
  item: Extract<TimelineItem, { kind: "clip" }>,
  x: number,
  y: number,
  w: number,
  h: number,
  radius: number,
): boolean {
  if (!item.thumbnail_dir) return false;
  const strip = getStrip(item.thumbnail_dir);
  if (!strip || strip.paths.length === 0) return false;

  const sourceStart = item.source_start_s ?? 0;
  const sourceEnd = sourceStart + item.duration_s;
  // Target ~one tile per 50 px of clip width. Min 1, cap at one tile
  // per second of source-time so a 5-second clip never gets more
  // than 5 tiles (and matches the 1-fps generation density).
  const tilesByWidth = Math.max(1, Math.round(w / 50));
  const tilesByDuration = Math.max(1, Math.floor(item.duration_s));
  const tileCount = Math.min(tilesByWidth, tilesByDuration);

  // Clip the drawing region to the rounded clip rect so JPEG tiles
  // don't bleed past the borders.
  ctx.save();
  pathRoundedRect(ctx, x, y, w, h, radius);
  ctx.clip();

  let drewAny = false;
  const tileWidth = w / tileCount;
  for (let i = 0; i < tileCount; i++) {
    // Pick the source-time at tile center, then floor to the
    // nearest generated frame index.
    const sourceTime =
      sourceStart + (sourceEnd - sourceStart) * ((i + 0.5) / tileCount);
    const frameIndex = Math.min(
      strip.paths.length - 1,
      Math.max(0, Math.floor(sourceTime)),
    );
    const img = strip.images[frameIndex];
    if (!img) continue;
    const tx = x + i * tileWidth;
    // drawImage with destination size scales the 120-px wide source
    // jpeg to fill the tile; aspect ratio differences just letterbox
    // a tiny bit, which is fine at this size.
    ctx.drawImage(img, tx, y, tileWidth, h);
    drewAny = true;
  }
  ctx.restore();
  return drewAny;
}

/** Draw a waveform amplitude line across an audio clip's pixel area.
 *
 *  Returns true if any line drew (so the caller knows to overlay
 *  the label on a dark band for legibility). Returns false when
 *  the cache is still loading the sidecar or the asset has no
 *  buckets — caller falls back to the plain coloured rect.
 *
 *  Bucket selection: the cached array spans the WHOLE asset's
 *  duration, but the clip only plays `[source_start_s,
 *  source_start_s + duration_s]` of source-time. We assume the
 *  asset's full duration is approximated by `duration_s + source_start_s`
 *  *for clips that haven't been split* — fine when a single clip
 *  references the whole asset; not always exactly right after a
 *  split. The sidecar doesn't carry an asset-duration header
 *  (yet), so we approximate by walking only the bucket span
 *  proportional to `duration_s / (duration_s + source_start_s)`.
 *  v2 will pass an explicit asset duration in the protocol so we
 *  can map source-time → bucket-index exactly. */
function drawClipWaveform(
  ctx: CanvasRenderingContext2D,
  item: Extract<TimelineItem, { kind: "clip" }>,
  x: number,
  y: number,
  w: number,
  h: number,
  radius: number,
): boolean {
  if (!item.waveform_path) return false;
  const buckets = getBuckets(item.waveform_path);
  if (!buckets || buckets.length === 0) return false;

  const sourceStart = item.source_start_s ?? 0;
  // Approximate the asset duration as the source-window we know we
  // can play from this clip. See the docblock — exact mapping needs
  // an asset-duration field in the protocol.
  const approxAssetEnd = sourceStart + item.duration_s;
  const approxAssetDuration = Math.max(1e-3, approxAssetEnd);
  const startFrac = sourceStart / approxAssetDuration;
  const endFrac = approxAssetEnd / approxAssetDuration;

  const startBucket = Math.max(
    0,
    Math.floor(buckets.length * startFrac),
  );
  const endBucket = Math.min(
    buckets.length,
    Math.ceil(buckets.length * endFrac),
  );
  if (endBucket <= startBucket) return false;

  // Clip to the rounded clip rect so the waveform doesn't bleed past
  // the borders.
  ctx.save();
  pathRoundedRect(ctx, x, y, w, h, radius);
  ctx.clip();

  // Draw a centered amplitude band: top half mirrored from bottom.
  // We resample the bucket slice to display width on the fly.
  const centerY = y + h / 2;
  // Leave a small inner margin so the line doesn't kiss the rounded
  // border on tall clips.
  const ampMax = Math.max(1, h / 2 - 3);

  // Stroke the upper envelope as a single path.
  ctx.beginPath();
  ctx.strokeStyle = "rgba(63, 185, 80, 0.85)";
  ctx.lineWidth = 1;
  for (let i = 0; i < w; i++) {
    // Pick the bucket(s) that map to this pixel column.
    const colStart = startBucket + ((endBucket - startBucket) * i) / w;
    const colEnd = startBucket + ((endBucket - startBucket) * (i + 1)) / w;
    const lo = Math.max(0, Math.floor(colStart));
    const hi = Math.min(buckets.length, Math.max(lo + 1, Math.ceil(colEnd)));
    let peak = 0;
    for (let j = lo; j < hi; j++) {
      if (buckets[j] > peak) peak = buckets[j];
    }
    const ampPx = peak * ampMax;
    const colX = x + i + 0.5;
    if (i === 0) {
      ctx.moveTo(colX, centerY - ampPx);
    } else {
      ctx.lineTo(colX, centerY - ampPx);
    }
  }
  ctx.stroke();

  // Mirror the lower envelope.
  ctx.beginPath();
  for (let i = 0; i < w; i++) {
    const colStart = startBucket + ((endBucket - startBucket) * i) / w;
    const colEnd = startBucket + ((endBucket - startBucket) * (i + 1)) / w;
    const lo = Math.max(0, Math.floor(colStart));
    const hi = Math.min(buckets.length, Math.max(lo + 1, Math.ceil(colEnd)));
    let peak = 0;
    for (let j = lo; j < hi; j++) {
      if (buckets[j] > peak) peak = buckets[j];
    }
    const ampPx = peak * ampMax;
    const colX = x + i + 0.5;
    if (i === 0) {
      ctx.moveTo(colX, centerY + ampPx);
    } else {
      ctx.lineTo(colX, centerY + ampPx);
    }
  }
  ctx.stroke();

  ctx.restore();
  return true;
}

/** Paint a title clip's text inline on its rect — the timeline-side
 *  preview of what `drawtext` will render at export time. Amber on
 *  the dark amber-tinted band; truncated if too wide. */
function drawClipTitleText(
  ctx: CanvasRenderingContext2D,
  styling: import("../protocol").TitleStyling,
  x: number,
  y: number,
  w: number,
  h: number,
) {
  if (w < 8) return;
  ctx.font = "11px ui-sans-serif, system-ui, sans-serif";
  ctx.textBaseline = "middle";
  const label = truncateToWidth(ctx, styling.text, w - 2 * CLIP_PADDING_X);
  ctx.fillStyle = "#f59e0b";
  ctx.fillText(label, x + CLIP_PADDING_X, y + h / 2);
}

/** Paint small `🔉 0.5×` / `⚡ 2×` badges in the top-right corner of
 *  a clip rect when its volume / speed differ from unity. Skipped for
 *  thin clips (the caller gates on `w > 36`). */
function drawClipBadges(
  ctx: CanvasRenderingContext2D,
  item: Extract<TimelineItem, { kind: "clip" }>,
  x: number,
  y: number,
  w: number,
) {
  const badges: string[] = [];
  if (
    item.volume !== null &&
    item.volume !== undefined &&
    Math.abs(item.volume - 1.0) > 1e-6
  ) {
    badges.push(`🔉 ${formatBadgeNumber(item.volume)}×`);
  }
  if (
    item.speed !== null &&
    item.speed !== undefined &&
    Math.abs(item.speed - 1.0) > 1e-6
  ) {
    badges.push(`⚡ ${formatBadgeNumber(item.speed)}×`);
  }
  if (badges.length === 0) return;
  ctx.font = "10px ui-sans-serif, system-ui, sans-serif";
  ctx.textBaseline = "top";
  // Right-align: walk badges right-to-left from the clip's right edge.
  let cursorX = x + w - 4;
  for (let i = badges.length - 1; i >= 0; i--) {
    const text = badges[i];
    const metrics = ctx.measureText(text);
    const padX = 4;
    const padY = 1;
    const boxW = metrics.width + padX * 2;
    const boxH = 14;
    const boxX = cursorX - boxW;
    const boxY = y + 3;
    if (boxX < x + 4) break; // Out of room — drop later badges.
    ctx.fillStyle = "rgba(13, 17, 23, 0.78)";
    ctx.fillRect(boxX, boxY, boxW, boxH);
    ctx.fillStyle = "#f59e0b";
    ctx.fillText(text, boxX + padX, boxY + padY);
    cursorX = boxX - 4;
  }
}

/** Format a multiplier for badges: trailing-zero-trim, max 2 decimals.
 *  `0.5 → "0.5"`, `2 → "2"`, `1.25 → "1.25"`. */
function formatBadgeNumber(n: number): string {
  const fixed = n.toFixed(2);
  return fixed.replace(/\.?0+$/, "");
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
