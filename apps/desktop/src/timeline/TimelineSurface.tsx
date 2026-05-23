import { useCallback, useEffect, useRef, useState } from "react";
import { useMediaStore } from "../media/store";
import { MENU_COMMANDS, onMenuCommand } from "../app/menuCommands";
import { editorDispatch } from "../editor/tauriDispatch";
import { useTimelineSelectionStore } from "../properties/store";
import { useProposalStore } from "./proposal";
import { ProposalHandles } from "./ProposalHandles";
import { hitTestEdge, hitTestSelectableBody, type EdgeHit } from "./hitDetect";
import { onThumbnailDecoded } from "./thumbnailCache";
import { onWaveformDecoded } from "./waveformCache";
import { type UserMoveDrag, type UserTrimDrag } from "./editMath";
import { buildDeleteSelectionOps, buildMoveDragOps, buildTrimDragOps } from "./editOps";
import { LANE_HEIGHT, PX_PER_SECOND_BASE, RULER_HEIGHT } from "./layout.ts";
import { collectDeletedKeys, collectHighlightKeys } from "./proposalDiffKeys.ts";
import { drawMoveGhost, drawPlayhead, drawRuler, drawTracks } from "./renderer.ts";
import { computeCanvasLayout } from "./canvasLayout.ts";
import { TimelineEditorialOverlay } from "./TimelineEditorialOverlay.tsx";
import { UserMoveTooltip, UserTrimTooltip } from "./TimelineDragTooltips.tsx";
import type { TimelineSnapshot } from "./store";

/** Wrapper that owns layout state (pps, width) so the canvas can
 *  publish it on each paint and the handles can subscribe. Avoids
 *  recomputing layout in two places. */
export function TimelineSurface({
  snapshot,
  currentTime,
  zoom,
}: {
  snapshot: TimelineSnapshot;
  currentTime: number;
  zoom: number;
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
        zoom={zoom}
        onLayout={handleLayout}
      />
      <TimelineEditorialOverlay
        snapshot={snapshot}
        containerWidth={layout.width}
        pps={layout.pps}
      />
      <ProposalHandles containerWidth={layout.width} pps={layout.pps} laneHeight={LANE_HEIGHT} />
    </>
  );
}

function TimelineCanvas({
  snapshot,
  currentTime,
  zoom,
  onLayout,
}: {
  snapshot: TimelineSnapshot;
  currentTime: number;
  zoom: number;
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
  const [userMove, setUserMove] = useState<UserMoveDrag | null>(null);
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
      const viewportWidth =
        container.parentElement?.clientWidth || container.clientWidth;
      const { cssHeight, cssWidth, pps, totalDuration } = computeCanvasLayout({
        snapshot,
        proposalSnapshot: proposal?.snapshot ?? null,
        viewportWidth,
        zoom,
      });

      canvas.width = Math.floor(cssWidth * dpr);
      canvas.height = Math.floor(cssHeight * dpr);
      canvas.style.width = `${cssWidth}px`;
      canvas.style.height = `${cssHeight}px`;

      const ctx = canvas.getContext("2d");
      if (!ctx) return;
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
      ctx.clearRect(0, 0, cssWidth, cssHeight);

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
          ctx.fillStyle = "rgba(120, 184, 255, 0.62)";
          ctx.fillRect(edgeX - 1, yTop, 2, LANE_HEIGHT - 8);
        }
      }

      // Draw the live drag-edge phantom on top of everything else.
      // 2px amber line at the dragged x.
      if (userTrim) {
        const x = userTrim.currentX;
        const yTop = RULER_HEIGHT;
        const yBot = RULER_HEIGHT + LANE_HEIGHT * snapshot.tracks.length;
        ctx.fillStyle = "#78b8ff";
        ctx.fillRect(x - 1, yTop, 2, yBot - yTop);
      }

      if (userMove) {
        drawMoveGhost(ctx, snapshot, currentTime, userMove, pps);
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
    zoom,
    onLayout,
    userTrim,
    userMove,
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
      void commitDeleteSelection();
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
  }, [commitDeleteSelection, proposal, selectedClipKey]);

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
    const body = hitTestSelectableBody(x, y, snapshot, ppsRef.current);
    if (body) {
      selectClip(body);
      const item = snapshot.tracks[body.trackIndex]?.items.find(
        (candidate) => candidate.index === body.clipIndex,
      );
      if (item?.kind === "clip") {
        e.currentTarget.setPointerCapture(e.pointerId);
        setUserMove({
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
    if (userMove) {
      setUserMove({ ...userMove, currentX: x, currentY: y });
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
    if (userMove) {
      void commitUserMove(userMove);
      setUserMove(null);
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
    const ops = buildMoveDragOps(snapshot, currentTime, drag, ppsRef.current);
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
      {userMove && (
        <UserMoveTooltip
          drag={userMove}
          snapshot={snapshot}
          currentTime={currentTime}
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
