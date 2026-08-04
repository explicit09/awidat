import { memo, useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useMediaStore } from "../media/store";
import { MENU_COMMANDS, onMenuCommand } from "../app/menuCommands";
import { editorDispatch } from "../editor/tauriDispatch";
import { useTimelineSelectionStore } from "../properties/store";
import { useProposalStore } from "./proposal";
import { ProposalHandles } from "./ProposalHandles";
import {
  hitTestBoundary,
  hitTestEdge,
  hitTestSelectableBody,
  type EdgeHit,
} from "./hitDetect";
import { onThumbnailDecoded } from "./thumbnailCache";
import { onWaveformDecoded } from "./waveformCache";
import {
  type BodyDragMode,
  type UserMoveDrag,
  type UserRollDrag,
  type UserTrimDrag,
} from "./editMath";
import {
  buildDeleteSelectionOps,
  buildMoveDragOps,
  buildRippleDeleteOps,
  buildRollDragOps,
  buildTrimDragOps,
} from "./editOps";
import { LANE_HEIGHT, PX_PER_SECOND_BASE, RULER_HEIGHT, timeToX, xToTime } from "./layout.ts";
import { collectDeletedKeys, collectHighlightKeys } from "./proposalDiffKeys.ts";
import {
  drawGhostClips,
  drawMoveGhost,
  drawPlayhead,
  drawRuler,
  drawTracks,
} from "./renderer.ts";
import { computeCanvasLayout } from "./canvasLayout.ts";
import { buildGhostRanges } from "./ghostClipRanges.ts";
import { usePendingProposals } from "./pendingProposals.ts";
import { useTimelineProposalFocus } from "./timelineProposalFocus.ts";
import { TimelineEditorialOverlay } from "./TimelineEditorialOverlay.tsx";
import { TimelineGhostOverlay } from "./TimelineGhostOverlay.tsx";
import { UserMoveTooltip, UserTrimTooltip } from "./TimelineDragTooltips.tsx";
import { useTimelineStore, type TimelineSnapshot } from "./store";
import { useFlashRanges } from "../state/focusController";
import { drawFlashRanges } from "./flashOverlay.ts";

const TIMELINE_PAINT_METRICS_KEY =
  import.meta.env.MODE === "perf" ? "__montageTimelinePaintMetrics" : undefined;
const TIMELINE_PAINT_INSTRUMENTATION_VERSION_KEY =
  import.meta.env.MODE === "perf"
    ? "__montageTimelinePaintInstrumentationVersion"
    : undefined;
const TIMELINE_PAINT_INSTRUMENTATION_VERSION = 1;

if (TIMELINE_PAINT_INSTRUMENTATION_VERSION_KEY) {
  const perfWindow = window as typeof window & Record<string, number | undefined>;
  perfWindow[TIMELINE_PAINT_INSTRUMENTATION_VERSION_KEY] =
    TIMELINE_PAINT_INSTRUMENTATION_VERSION;
}

type TimelinePaintMetrics = {
  count: number;
  totalMs: number;
  maxMs: number;
  durationsMs: number[];
  reasonCounts: Record<TimelinePaintReason, number>;
};

type TimelinePaintReason = "effect" | "resize" | "thumbnail" | "waveform";

function recordTimelinePaint(durationMs: number, reason: TimelinePaintReason) {
  if (!TIMELINE_PAINT_METRICS_KEY || !Number.isFinite(durationMs)) return;
  const perfWindow = window as typeof window & Record<string, TimelinePaintMetrics | undefined>;
  const metrics = perfWindow[TIMELINE_PAINT_METRICS_KEY] ?? {
    count: 0,
    totalMs: 0,
    maxMs: 0,
    durationsMs: [],
    reasonCounts: { effect: 0, resize: 0, thumbnail: 0, waveform: 0 },
  };
  metrics.count += 1;
  metrics.totalMs += durationMs;
  metrics.maxMs = Math.max(metrics.maxMs, durationMs);
  metrics.durationsMs.push(durationMs);
  metrics.reasonCounts[reason] += 1;
  perfWindow[TIMELINE_PAINT_METRICS_KEY] = metrics;
}

/** Wrapper that owns layout state (pps, width) so the canvas can
 *  publish it on each paint and the handles can subscribe. Avoids
 *  recomputing layout in two places. */
export const TimelineSurface = memo(function TimelineSurface({
  snapshot,
  zoom,
}: {
  snapshot: TimelineSnapshot;
  zoom: number;
}) {
  // Vertical zoom multiplier on the per-track lane height. trackZoom = 1
  // gives the base LANE_HEIGHT; users grow/shrink via ZoomControls (P4.4)
  // or pinch (P4.4). Each downstream consumer reads the resolved
  // laneHeight off of layout so renderer / hit-detect / handles agree.
  const trackZoom = useTimelineStore((s) => s.trackZoom);
  const laneHeight = LANE_HEIGHT * trackZoom;
  const [layout, setLayout] = useState<{ pps: number; width: number; laneHeight: number }>({
    pps: PX_PER_SECOND_BASE,
    width: 0,
    laneHeight,
  });
  const handleLayout = useCallback(
    (pps: number, width: number, lane: number) => {
      // Only update if it actually changed — paint() runs on
      // every frame React re-renders, but layout changes only
      // on resize / snapshot swap.
      setLayout((prev) =>
        prev.pps === pps && prev.width === width && prev.laneHeight === lane
          ? prev
          : { pps, width, laneHeight: lane },
      );
    },
    [],
  );
  return (
    <>
      <TimelineCanvas
        snapshot={snapshot}
        zoom={zoom}
        laneHeight={laneHeight}
        onLayout={handleLayout}
      />
      <TimelineEditorialOverlay
        snapshot={snapshot}
        containerWidth={layout.width}
        pps={layout.pps}
      />
      <ProposalHandles
        containerWidth={layout.width}
        pps={layout.pps}
        laneHeight={layout.laneHeight}
      />
      <TimelineGhostOverlay
        snapshot={snapshot}
        containerWidth={layout.width}
        pps={layout.pps}
        laneHeight={layout.laneHeight}
      />
    </>
  );
});

function TimelineCanvas({
  snapshot,
  zoom,
  laneHeight,
  onLayout,
}: {
  snapshot: TimelineSnapshot;
  zoom: number;
  laneHeight: number;
  onLayout: (pps: number, widthPx: number, laneHeight: number) => void;
}) {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const playheadCanvasRef = useRef<HTMLCanvasElement | null>(null);
  const containerRef = useRef<HTMLDivElement | null>(null);
  const staticPaintRef = useRef<((reason: TimelinePaintReason) => void) | null>(null);
  const playheadPaintRef = useRef<(() => void) | null>(null);
  const canvasLayoutRef = useRef({
    cssHeight: 0,
    cssWidth: 0,
    dpr: 1,
    pps: PX_PER_SECOND_BASE,
  });
  // Latest pps used for paint, captured into a ref so click handlers
  // can convert x → seconds without recomputing layout.
  const ppsRef = useRef<number>(PX_PER_SECOND_BASE);
  const currentTimeRef = useRef(useMediaStore.getState().timelineTime);
  // Canvas seek + playhead use timeline-time. Single-asset / empty-
  // timeline mode keeps using source-time inside the MediaPane, but
  // the canvas itself is a timeline-time surface — clicking at x=80px
  // means "seek to ~5s of the timeline", not "5s of the source." With
  // a multi-clip timeline these two axes diverge; with a single-clip
  // timeline they coincide for now (until trim shifts source_start_s).
  const requestTimelineSeek = useMediaStore((s) => s.requestTimelineSeek);
  const refreshTimeline = useTimelineStore((s) => s.refresh);
  const proposal = useProposalStore((s) => s.active);
  // Pending cut-medium proposals power the ghost-overlay pass. The
  // DOM <TimelineGhostOverlay> sibling owns the hover affordance and
  // keyboard handling; the canvas just draws the dashed-cyan bands.
  const pendingProposals = usePendingProposals((s) => s.pending);
  const focusedProposalId = useTimelineProposalFocus((s) => s.focusedId);
  // Wave 4 W4.6 — Review → flashes. Subscribe to the focus controller's
  // ephemeral range set so the canvas re-paints when a range arrives
  // and again when it expires (the range list is empty after ~600ms).
  const flashRanges = useFlashRanges((s) => s.ranges);
  // Cursor hint when hovering near a clip edge (without dragging).
  const [edgeHover, setEdgeHover] = useState<EdgeHit | null>(null);
  // Active drag, set on pointerdown-near-edge, cleared on pointerup.
  const [userTrim, setUserTrim] = useState<UserTrimDrag | null>(null);
  const [userMove, setUserMove] = useState<UserMoveDrag | null>(null);
  const [userRoll, setUserRoll] = useState<UserRollDrag | null>(null);
  // Properties-pane selection: which clip is currently inspected.
  // The canvas paints a subtle amber outline on the selected clip
  // so the link between timeline selection and right-rail content
  // is visible.
  const selectedClipKey = useTimelineSelectionStore(
    (s) => s.selectedClipKey,
  );
  const selectClip = useTimelineSelectionStore((s) => s.select);
  const clearSelection = useTimelineSelectionStore((s) => s.clear);

  playheadPaintRef.current = () => {
    const canvas = playheadCanvasRef.current;
    const { cssHeight, cssWidth, dpr, pps } = canvasLayoutRef.current;
    if (!canvas || cssWidth <= 0 || cssHeight <= 0) return;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, cssWidth, cssHeight);
    drawPlayhead(ctx, cssWidth, cssHeight, currentTimeRef.current, pps);
    if (edgeHover && !userTrim) {
      const item = snapshot.tracks[edgeHover.trackIndex]?.items.find(
        (it) => it.index === edgeHover.clipIndex,
      );
      if (item && item.kind === "clip") {
        const edgeX =
          edgeHover.side === "start"
            ? timeToX(item.track_start_s, pps)
            : timeToX(item.track_start_s + item.duration_s, pps);
        const yTop = RULER_HEIGHT + edgeHover.trackIndex * laneHeight + 4;
        ctx.fillStyle = "rgba(239, 68, 68, 0.62)";
        ctx.fillRect(edgeX - 1, yTop, 2, laneHeight - 8);
      }
    }
    if (userTrim) {
      const x = userTrim.currentX;
      const yTop = RULER_HEIGHT;
      const yBot = RULER_HEIGHT + laneHeight * snapshot.tracks.length;
      ctx.fillStyle = "#EF4444";
      ctx.fillRect(x - 1, yTop, 2, yBot - yTop);
    }
    if (userMove) {
      drawMoveGhost(ctx, snapshot, currentTimeRef.current, userMove, pps, laneHeight);
    }
  };

  // Compute pixel layout. When a proposal is active, the base canvas
  // paints two passes: original snapshot at α=0.45 (the "before") and
  // the proposed snapshot at α=1.0 with diff-hint coloring (the "after").
  // The playhead is drawn separately so playback time does not repaint the
  // static timeline surface.
  staticPaintRef.current = (reason) => {
    const canvas = canvasRef.current;
    const playheadCanvas = playheadCanvasRef.current;
    const container = containerRef.current;
    if (!canvas || !playheadCanvas || !container) return;

    const paintStartedAt = TIMELINE_PAINT_METRICS_KEY ? performance.now() : 0;
    const dpr = window.devicePixelRatio || 1;
    const viewportWidth =
      container.parentElement?.clientWidth || container.clientWidth;
    const { cssHeight, cssWidth, pps, totalDuration } = computeCanvasLayout({
      snapshot,
      proposalSnapshot: proposal?.snapshot ?? null,
      viewportWidth,
      zoom,
      laneHeight,
    });

    for (const target of [canvas, playheadCanvas]) {
      target.width = Math.floor(cssWidth * dpr);
      target.height = Math.floor(cssHeight * dpr);
      target.style.width = `${cssWidth}px`;
      target.style.height = `${cssHeight}px`;
    }
    canvasLayoutRef.current = { cssHeight, cssWidth, dpr, pps };

    const ctx = canvas.getContext("2d");
    if (!ctx) return;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, cssWidth, cssHeight);

    ppsRef.current = pps;
    onLayout(pps, cssWidth, laneHeight);

    drawRuler(ctx, cssWidth, totalDuration, pps);

    const selectedKey = selectedClipKey
      ? `${selectedClipKey.trackIndex}:${selectedClipKey.clipIndex}`
      : undefined;
    if (proposal) {
      // Pass A — current state under, dimmed, with delete strike.
      const deletedKeys = collectDeletedKeys(proposal.diffHints);
      ctx.globalAlpha = 0.45;
      drawTracks(
        ctx,
        cssWidth,
        snapshot.tracks,
        pps,
        { deletedKeys },
        laneHeight,
      );
      ctx.globalAlpha = 1.0;
      // Pass B — proposed state on top, full opacity, with
      // diff-hint highlights for trimmed/inserted/split items.
      // Selection ring rides on the post-state pass so the user
      // sees which clip in the *new* timeline they're inspecting.
      const highlightKeys = collectHighlightKeys(proposal.diffHints);
      drawTracks(
        ctx,
        cssWidth,
        proposal.snapshot.tracks,
        pps,
        {
          highlightKeys,
          selectedKey,
          // Surface the agent's one-sentence rationale below the
          // clip name on highlighted clips. The Inspector / Brief
          // own the full treatment; this is the lightweight
          // canvas hint (Wave 3 B5).
          proposalRationale: proposal.rationale,
        },
        laneHeight,
      );
    } else {
      drawTracks(
        ctx,
        cssWidth,
        snapshot.tracks,
        pps,
        { selectedKey },
        laneHeight,
      );
    }

    // Wave 3 C2 — ghost-clip review pass. Only fires when there's
    // no active proposal ghost (the two surfaces conflict; the
    // single-proposal canvas overlay above already paints the cyan
    // diff). Cut-medium pendings always render.
    if (!proposal) {
      const cutPending = pendingProposals.filter((p) => p.medium === "cut");
      if (cutPending.length > 0) {
        const ranges = buildGhostRanges(cutPending, snapshot);
        if (ranges.length > 0) {
          const proposalsById = new Map(
            cutPending.map((p) => [p.callId, p] as const),
          );
          drawGhostClips(
            ctx,
            ranges,
            proposalsById,
            focusedProposalId,
            pps,
            laneHeight,
          );
        }
      }
    }

    // Wave 4 W4.6 — Review-focus flash pass. Paints a brief glow on
    // ranges the focus controller registered (cleared after ~600ms).
    // Drawn before the playhead so the playhead stays visible over a
    // flashed range.
    if (flashRanges.length > 0) {
      drawFlashRanges(ctx, flashRanges, pps, laneHeight);
    }

    if (TIMELINE_PAINT_METRICS_KEY) {
      recordTimelinePaint(performance.now() - paintStartedAt, reason);
    }
    playheadPaintRef.current?.();
  };

  useEffect(() => {
    staticPaintRef.current?.("effect");
  }, [
    snapshot,
    proposal,
    zoom,
    laneHeight,
    onLayout,
    selectedClipKey,
    pendingProposals,
    focusedProposalId,
    flashRanges,
  ]);

  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;
    // Covers window resize and parent flex/grid resizes without being
    // recreated for each playback-clock update.
    const ro = new ResizeObserver(() => staticPaintRef.current?.("resize"));
    ro.observe(container);
    const unsubThumb = onThumbnailDecoded(() =>
      staticPaintRef.current?.("thumbnail"),
    );
    const unsubWave = onWaveformDecoded(() =>
      staticPaintRef.current?.("waveform"),
    );
    return () => {
      ro.disconnect();
      unsubThumb();
      unsubWave();
    };
  }, []);

  useEffect(() => {
    const update = (timelineTime: number) => {
      currentTimeRef.current = timelineTime;
      playheadPaintRef.current?.();
    };
    update(useMediaStore.getState().timelineTime);
    return useMediaStore.subscribe((state, previous) => {
      if (state.timelineTime === previous.timelineTime) return;
      update(state.timelineTime);
    });
  }, []);

  useEffect(() => {
    playheadPaintRef.current?.();
  }, [edgeHover, userTrim, userMove]);

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
    const t = xToTime(x, ppsRef.current);
    // Clamp to project duration so we don't ask the player to seek
    // past the end (HTMLMediaElement clamps anyway, but this keeps
    // the visual feedback clean).
    return Math.max(0, Math.min(t, snapshot.duration_s || t));
  }

  const commitDeleteSelection = useCallback(async (): Promise<void> => {
    if (proposal || !selectedClipKey) return;
    const ops = buildDeleteSelectionOps(snapshot, selectedClipKey);
    if (ops.length === 0) return;

    try {
      await editorDispatch.proposeUserEdit(ops);
      clearSelection();
    } catch (err) {
      // eslint-disable-next-line no-console
      console.warn("propose_user_edit (delete selection) failed", err);
    }
  }, [clearSelection, proposal, selectedClipKey, snapshot]);

  // Ripple-delete: like delete, but closes the gap. Shift+Del/Backspace.
  const commitRippleDeleteSelection = useCallback(async (): Promise<void> => {
    if (proposal || !selectedClipKey) return;
    const ops = buildRippleDeleteOps(snapshot, selectedClipKey);
    if (ops.length === 0) return;
    try {
      await editorDispatch.proposeUserEdit(ops);
      clearSelection();
    } catch (err) {
      // eslint-disable-next-line no-console
      console.warn("propose_user_edit (ripple delete) failed", err);
    }
  }, [clearSelection, proposal, selectedClipKey, snapshot]);

  useEffect(() => {
    function onKeyDown(e: KeyboardEvent) {
      const isDeleteKey =
        e.code === "Delete" ||
        e.code === "Backspace" ||
        e.key === "Delete" ||
        e.key === "Backspace" ||
        e.key === "Del";
      if (!isDeleteKey) return;
      if (isEditableTarget(e.target)) return;
      if (!selectedClipKey || proposal) return;
      e.preventDefault();
      if (e.shiftKey) {
        void commitRippleDeleteSelection();
      } else {
        void commitDeleteSelection();
      }
    }
    window.addEventListener("keydown", onKeyDown);
    const unlistenMenu = onMenuCommand((id) => {
      if (id === MENU_COMMANDS.DELETE_CLIP) {
        void commitDeleteSelection();
      }
    });
    return () => {
      window.removeEventListener("keydown", onKeyDown);
      unlistenMenu();
    };
  }, [
    commitDeleteSelection,
    commitRippleDeleteSelection,
    proposal,
    selectedClipKey,
  ]);

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
    // Roll edit: shared boundary between two adjacent clips. Tested
    // first because a single pointer x can be near both clip edges
    // simultaneously, and roll is the more specific gesture. No
    // modifier required — the position itself disambiguates.
    const boundary = hitTestBoundary(x, y, snapshot, ppsRef.current, laneHeight);
    if (boundary) {
      e.currentTarget.setPointerCapture(e.pointerId);
      setUserRoll({
        hit: {
          from: { clipUuid: boundary.from.clipUuid, sourceEnd: boundary.from.sourceEnd },
          to: { clipUuid: boundary.to.clipUuid, sourceStart: boundary.to.sourceStart },
        },
        startX: x,
        currentX: x,
      });
      return;
    }
    const hit = hitTestEdge(x, y, snapshot, ppsRef.current, laneHeight);
    if (hit) {
      e.currentTarget.setPointerCapture(e.pointerId);
      // Cmd / Ctrl at pointer-down → ripple-trim semantics on commit
      // (downstream clips shift by the trim delta).
      setUserTrim({
        hit,
        startX: x,
        currentX: x,
        ripple: e.metaKey || e.ctrlKey,
      });
      // Don't seek on edge-down; the user is starting a trim, not
      // scrubbing.
      return;
    }
    // No edge hit — update the properties-pane selection. Clip body
    // under the pointer = select; empty space = clear. Either way
    // the click also scrubs the playhead (preserving the existing
    // seek-on-click behaviour).
    const body = hitTestSelectableBody(x, y, snapshot, ppsRef.current, laneHeight);
    if (body) {
      selectClip(body);
      const item = snapshot.tracks[body.trackIndex]?.items.find(
        (candidate) => candidate.index === body.clipIndex,
      );
      if (item?.kind === "clip") {
        e.currentTarget.setPointerCapture(e.pointerId);
        requestTimelineSeek(timeFromClientX(clientX));
        // Modifier semantics (Premiere/Resolve):
        //   - Alt only        → slip (shift source range, hold position)
        //   - Cmd/Ctrl + Alt  → slide (shift position, hold neighbors)
        //   - no modifier     → ripple move (default body drag)
        const altOnly = e.altKey && !(e.metaKey || e.ctrlKey);
        const cmdAlt = e.altKey && (e.metaKey || e.ctrlKey);
        const mode: BodyDragMode = altOnly ? "slip" : cmdAlt ? "slide" : "ripple";
        setUserMove({
          mode,
          trackIndex: body.trackIndex,
          clipIndex: body.clipIndex,
          clipUuid: item.clip_uuid,
          linkGroupId: item.link_group_id,
          startX: x,
          currentX: x,
          startY: y,
          currentY: y,
        });
        return;
      }
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
    if (userRoll) {
      setUserRoll({ ...userRoll, currentX: x });
      return;
    }
    if (userMove) {
      setUserMove({ ...userMove, currentX: x, currentY: y });
      return;
    }
    // Hover state — update edge cursor hint.
    const hover = hitTestEdge(x, y, snapshot, ppsRef.current, laneHeight);
    setEdgeHover(hover);
    // Hover tooltip — set the canvas's `title` attribute to the full
    // clip name so the user can see it even when the label is middle-
    // truncated on a narrow clip. Native browser tooltip is enough;
    // no need for a custom overlay component.
    const body = hitTestSelectableBody(x, y, snapshot, ppsRef.current, laneHeight);
    if (body) {
      const item = snapshot.tracks[body.trackIndex]?.items.find(
        (candidate) => candidate.index === body.clipIndex,
      );
      e.currentTarget.title = item?.kind === "clip" ? item.name : "";
    } else {
      e.currentTarget.title = "";
    }
    if (e.currentTarget.hasPointerCapture(e.pointerId)) {
      requestTimelineSeek(timeFromClientX(clientX));
    }
  }

  function onDragOver(e: React.DragEvent<HTMLCanvasElement>) {
    if (proposal) return;
    if (e.dataTransfer.types.includes("application/x-montage-media")) {
      e.preventDefault();
      e.dataTransfer.dropEffect = "copy";
    }
  }

  async function onDrop(e: React.DragEvent<HTMLCanvasElement>) {
    if (proposal) return;
    const assetId = e.dataTransfer.getData("application/x-montage-media");
    if (!assetId) return;
    e.preventDefault();
    try {
      await invoke<boolean>("insert_media_on_timeline", {
        assetId,
        atS: timeFromClientX(e.clientX),
      });
      await refreshTimeline();
    } catch (err) {
      // eslint-disable-next-line no-console
      console.warn("insert_media_on_timeline failed", err);
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
    if (userRoll) {
      void commitUserRoll(userRoll);
      setUserRoll(null);
    }
    if (userMove) {
      void commitUserMove(userMove);
      setUserMove(null);
    }
  }

  async function commitUserRoll(drag: UserRollDrag): Promise<void> {
    const ops = buildRollDragOps(drag, ppsRef.current);
    if (ops.length === 0) return;
    try {
      await editorDispatch.proposeUserEdit(ops);
    } catch (err) {
      // eslint-disable-next-line no-console
      console.warn("propose_user_edit (roll) failed", err);
    }
  }

  // Build a one-op TrimClip envelope from the drag state and submit
  // it via propose_user_edit. The Step-5 proposal pipeline takes
  // over: the ghost overlay shows the proposed cut, Accept commits
  // it. Cancellation while dragging (pointercancel / pointerup with
  // no movement) just clears userTrim — no envelope is sent.
  async function commitUserTrim(drag: UserTrimDrag): Promise<void> {
    const ops = buildTrimDragOps(drag, ppsRef.current);
    if (ops.length === 0) return;
    try {
      await editorDispatch.proposeUserEdit(ops);
    } catch (err) {
      // Surface failures to the console; the existing
      // build_proposal-failed Item::Error path will also fire if the
      // backend rejects the envelope, so the user sees the reason
      // in chat.
      // eslint-disable-next-line no-console
      console.warn("propose_user_edit failed", err);
    }
  }

  async function commitUserMove(drag: UserMoveDrag): Promise<void> {
    const ops = buildMoveDragOps(
      snapshot,
      currentTimeRef.current,
      drag,
      ppsRef.current,
    );
    if (ops.length === 0) return;

    try {
      await editorDispatch.proposeUserEdit(ops);
    } catch (err) {
      // eslint-disable-next-line no-console
      console.warn("propose_user_edit (move clip) failed", err);
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
    : userRoll
    ? "ew-resize"
    : userMove
    ? "grabbing"
    : edgeHover
    ? "ew-resize"
    : snapshot.duration_s > 0
    ? "grab"
    : "default";

  return (
    <div className="timeline-canvas-wrap" ref={containerRef}>
      {snapshot.tracks.length === 0 && (
        <div className="timeline-empty">
          Drop a clip on a track to start editing.
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
        onDragOver={onDragOver}
        onDrop={onDrop}
      />
      <canvas
        ref={playheadCanvasRef}
        aria-hidden="true"
        style={{
          inset: 0,
          pointerEvents: "none",
          position: "absolute",
          zIndex: 1,
        }}
      />
      {userTrim && (
        <UserTrimTooltip drag={userTrim} pps={ppsRef.current} />
      )}
      {userMove && (
        <UserMoveTooltip
          drag={userMove}
          snapshot={snapshot}
          pps={ppsRef.current}
        />
      )}
    </div>
  );
}

function isEditableTarget(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  if (target.isContentEditable) return true;
  const tag = target.tagName.toLowerCase();
  return tag === "input" || tag === "textarea" || tag === "select";
}
