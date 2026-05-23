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
import { MENU_COMMANDS, onMenuCommand } from "../app/menuCommands";
import {
  hitTestEdge,
  hitTestSelectableBody,
  pxDeltaToSourceDelta,
  type EdgeHit,
} from "./hitDetect";
import { getStrip, onThumbnailDecoded } from "./thumbnailCache";
import { getBuckets, onWaveformDecoded } from "./waveformCache";
import { useTimelineSelectionStore } from "../properties/store";
import { collectSnapTargets, snapTime } from "./snap";
import { cacheStripSegments, type CacheStripInput } from "./cacheStrip";

/** Pixels-per-second at zoom=1. Tuned so a 60s project fits the
 *  default pane width without horizontal scroll. */
const PX_PER_SECOND_BASE = 12;

/** Height of one track lane at trackZoom = 1, in pixels. Actual
 *  lane height during paint is `LANE_HEIGHT_BASE * trackZoom`. */
const LANE_HEIGHT_BASE = 38;

/** Height of the time ruler at the top of the canvas. */
const RULER_HEIGHT = 22;

/** Padding inside each clip block. */
const CLIP_PADDING_X = 6;

/** Timeline-time snap tolerance for direct manipulation in the canvas. */
const SNAP_TOLERANCE_S = 0.15;

export function TimelinePane() {
  const projectReady = useProjectStore((s) => s.current !== null);
  const projectRoot = useProjectStore((s) => s.current);
  const snapshot = useTimelineStore((s) => s.snapshot);
  const zoom = useTimelineStore((s) => s.zoom);
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

  const stageRef = useRef<HTMLDivElement | null>(null);
  // Mouse-wheel handlers — must be non-passive so cmd/ctrl+wheel can
  // preventDefault before the browser's page-zoom kicks in. React's
  // synthetic wheel handler is passive in modern React, so we attach
  // manually to the underlying DOM node.
  useEffect(() => {
    const el = stageRef.current;
    if (!el) return;
    function onWheel(event: WheelEvent) {
      // Wheel modifier semantics on macOS:
      //   - Real Cmd+wheel (mouse) sets `metaKey`.
      //   - Trackpad pinch fires `wheel` with `ctrlKey: true` but
      //     `metaKey: false` — the browser fakes ctrl for pinch.
      // Map them to different zoom axes so users get both:
      //   - Cmd+wheel  → horizontal time-zoom (anchored at cursor)
      //   - pinch      → vertical track-zoom
      // Linux/Windows users can hold real Ctrl for vertical zoom (the
      // same gesture the OS uses for browser page-zoom by default).
      const horizontalZoom = event.metaKey;
      const verticalZoom = event.ctrlKey && !event.metaKey;
      if (horizontalZoom) {
        event.preventDefault();
        const { zoom: currentZoom, setZoom } = useTimelineStore.getState();
        const factor = Math.exp(-event.deltaY / 240);
        const nextZoom = currentZoom * factor;
        const rect = el!.getBoundingClientRect();
        const cursorContentX = (event.clientX - rect.left) + el!.scrollLeft;
        setZoom(nextZoom);
        // Preserve the content-x under the cursor across the zoom so
        // the user's anchor point stays visually stable.
        const ratio = useTimelineStore.getState().zoom / currentZoom;
        const newContentX = cursorContentX * ratio;
        const cursorViewportX = event.clientX - rect.left;
        el!.scrollLeft = Math.max(0, newContentX - cursorViewportX);
        return;
      }
      if (verticalZoom) {
        event.preventDefault();
        const { trackZoom: currentTrackZoom, setTrackZoom } =
          useTimelineStore.getState();
        const factor = Math.exp(-event.deltaY / 240);
        setTrackZoom(currentTrackZoom * factor);
        return;
      }
      // Shift + wheel = horizontal scroll (deltaY drives X), matching
      // editor conventions. Without shift, deltaY is vertical scroll
      // which the browser handles natively now that .timeline-stage
      // has overflow-y: auto.
      if (event.shiftKey && event.deltaY !== 0 && event.deltaX === 0) {
        event.preventDefault();
        el!.scrollLeft += event.deltaY;
      }
    }
    el.addEventListener("wheel", onWheel, { passive: false });
    return () => {
      el.removeEventListener("wheel", onWheel);
    };
  }, []);

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
        <ZoomControls />
      </header>
      <div className="timeline-stage" ref={stageRef}>
        <TimelineSurface snapshot={snapshot} currentTime={currentTime} zoom={zoom} />
        <ProposalActions />
      </div>
    </section>
  );
}

/** Compact +/−/fit controls for horizontal time-zoom and vertical
 *  track-zoom. Sits in the timeline header. Keyboard shortcuts
 *  (`timeline:zoom_in/out/fit`) still drive the same store actions
 *  for users who prefer the menu. */
function ZoomControls() {
  const zoom = useTimelineStore((s) => s.zoom);
  const trackZoom = useTimelineStore((s) => s.trackZoom);
  const zoomIn = useTimelineStore((s) => s.zoomIn);
  const zoomOut = useTimelineStore((s) => s.zoomOut);
  const fitZoom = useTimelineStore((s) => s.fitZoom);
  const trackZoomIn = useTimelineStore((s) => s.trackZoomIn);
  const trackZoomOut = useTimelineStore((s) => s.trackZoomOut);
  const fitTrackZoom = useTimelineStore((s) => s.fitTrackZoom);
  return (
    <div className="timeline-zoom-controls">
      <div className="timeline-zoom-group" title="Horizontal zoom (Cmd/Ctrl + wheel)">
        <button type="button" onClick={zoomOut} aria-label="Zoom out">−</button>
        <button type="button" onClick={fitZoom} aria-label="Reset zoom">
          {zoom.toFixed(2)}×
        </button>
        <button type="button" onClick={zoomIn} aria-label="Zoom in">+</button>
      </div>
      <div className="timeline-zoom-group" title="Track height">
        <button type="button" onClick={trackZoomOut} aria-label="Shrink tracks">▾</button>
        <button type="button" onClick={fitTrackZoom} aria-label="Reset track height">
          {trackZoom.toFixed(2)}×
        </button>
        <button type="button" onClick={trackZoomIn} aria-label="Grow tracks">▴</button>
      </div>
    </div>
  );
}

/** Wrapper that owns layout state (pps, width) so the canvas can
 *  publish it on each paint and the handles can subscribe. Avoids
 *  recomputing layout in two places. */
function TimelineSurface({
  snapshot,
  currentTime,
  zoom,
}: {
  snapshot: TimelineSnapshot;
  currentTime: number;
  zoom: number;
}) {
  const [layout, setLayout] = useState<{ pps: number; width: number; laneHeight: number }>({
    pps: PX_PER_SECOND_BASE,
    width: 0,
    laneHeight: LANE_HEIGHT_BASE,
  });
  const handleLayout = useCallback((pps: number, width: number, laneHeight: number) => {
    // Only update if it actually changed — paint() runs on
    // every frame React re-renders, but layout changes only
    // on resize / snapshot swap / track-zoom change.
    setLayout((prev) =>
      prev.pps === pps && prev.width === width && prev.laneHeight === laneHeight
        ? prev
        : { pps, width, laneHeight },
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
        laneHeight={layout.laneHeight}
      />
      <ProposalHandles
        containerWidth={layout.width}
        pps={layout.pps}
        laneHeight={layout.laneHeight}
      />
    </>
  );
}

function TimelineEditorialOverlay({
  snapshot,
  containerWidth,
  pps,
  laneHeight,
}: {
  snapshot: TimelineSnapshot;
  containerWidth: number;
  pps: number;
  laneHeight: number;
}) {
  if (containerWidth <= 0 || snapshot.tracks.length === 0) return null;
  const cutBadges = buildCutBadges(snapshot, pps, laneHeight);
  const splitOffsets = buildSplitOffsets(snapshot, pps, laneHeight);
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

/** Active user-trim drag — lives in canvas state so cursor + paint
 *  update on edge-hover and during the drag. */
type UserTrimDrag = {
  hit: EdgeHit;
  /** Pointer x at drag start, in canvas-local pixels. */
  startX: number;
  /** Live pointer x in canvas-local pixels. */
  currentX: number;
  /** Cmd / Ctrl held at pointer-down → ripple-trim semantics on
   *  commit (downstream clips shift by the trim delta). */
  ripple: boolean;
};

type UserMoveDrag = {
  trackIndex: number;
  clipIndex: number;
  clipUuid: string;
  linkGroupId: string | null;
  startX: number;
  currentX: number;
  startY: number;
  currentY: number;
};

function TimelineCanvas({
  snapshot,
  currentTime,
  zoom,
  onLayout,
}: {
  snapshot: TimelineSnapshot;
  currentTime: number;
  zoom: number;
  onLayout: (pps: number, widthPx: number, laneHeight: number) => void;
}) {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const containerRef = useRef<HTMLDivElement | null>(null);
  // Latest pps used for paint, captured into a ref so click handlers
  // can convert x → seconds without recomputing layout.
  const ppsRef = useRef<number>(PX_PER_SECOND_BASE);
  // Latest lane height used during paint, captured into a ref so hit
  // tests dispatched outside the paint cycle can stay in sync when
  // the user changes vertical zoom.
  const laneHeightRef = useRef<number>(LANE_HEIGHT_BASE);
  const trackZoom = useTimelineStore((s) => s.trackZoom);
  // Canvas seek + playhead use timeline-time. Single-asset / empty-
  // timeline mode keeps using source-time inside the MediaPane, but
  // the canvas itself is a timeline-time surface — clicking at x=80px
  // means "seek to ~5s of the timeline", not "5s of the source." With
  // a multi-clip timeline these two axes diverge; with a single-clip
  // timeline they coincide for now (until trim shifts source_start_s).
  const requestTimelineSeek = useMediaStore((s) => s.requestTimelineSeek);
  const refreshTimeline = useTimelineStore((s) => s.refresh);
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
      // Use the wider snapshot for layout — picking max of
      // current/proposed track counts keeps the canvas tall
      // enough that no track gets clipped.
      const proposedTrackCount = proposal?.snapshot.tracks.length ?? 0;
      const lanesCount = Math.max(
        snapshot.tracks.length,
        proposedTrackCount,
        1,
      );
      const laneHeight = LANE_HEIGHT_BASE * trackZoom;
      const cssHeight = RULER_HEIGHT + lanesCount * laneHeight;
      // Pps from the max of current vs proposed durations so the
      // whole post-state fits even when a proposal extends past
      // the original.
      const proposedDuration = proposal?.snapshot.duration_s ?? 0;
      const totalDuration = Math.max(snapshot.duration_s, proposedDuration);
      const pps = computePps(totalDuration, viewportWidth, zoom);
      const cssWidth =
        totalDuration > 0
          ? Math.max(viewportWidth, totalDuration * pps + 8)
          : viewportWidth;

      canvas.width = Math.floor(cssWidth * dpr);
      canvas.height = Math.floor(cssHeight * dpr);
      canvas.style.width = `${cssWidth}px`;
      canvas.style.height = `${cssHeight}px`;

      const ctx = canvas.getContext("2d");
      if (!ctx) return;
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
      ctx.clearRect(0, 0, cssWidth, cssHeight);

      ppsRef.current = pps;
      laneHeightRef.current = laneHeight;
      onLayout(pps, cssWidth, laneHeight);

      const cacheSegments = cacheStripSegments(
        collectCacheStripInputs(snapshot.tracks),
      );
      drawRuler(ctx, cssWidth, totalDuration, pps, cacheSegments);

      const selectedKey = selectedClipKey
        ? `${selectedClipKey.trackIndex}:${selectedClipKey.clipIndex}`
        : undefined;
      if (proposal) {
        // Pass A — current state under, dimmed, with delete strike.
        const deletedKeys = collectDeletedKeys(proposal.diffHints);
        ctx.globalAlpha = 0.45;
        drawTracks(ctx, cssWidth, snapshot.tracks, pps, laneHeight, {
          deletedKeys,
        });
        ctx.globalAlpha = 1.0;
        // Pass B — proposed state on top, full opacity, with
        // diff-hint highlights for trimmed/inserted/split items.
        // Selection ring rides on the post-state pass so the user
        // sees which clip in the *new* timeline they're inspecting.
        const highlightKeys = collectHighlightKeys(proposal.diffHints);
        drawTracks(ctx, cssWidth, proposal.snapshot.tracks, pps, laneHeight, {
          highlightKeys,
          selectedKey,
        });
      } else {
        drawTracks(ctx, cssWidth, snapshot.tracks, pps, laneHeight, { selectedKey });
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
          const yTop = RULER_HEIGHT + edgeHover.trackIndex * laneHeight + 4;
          ctx.fillStyle = "rgba(120, 184, 255, 0.62)";
          ctx.fillRect(edgeX - 1, yTop, 2, laneHeight - 8);
        }
      }

      // Draw the live drag-edge phantom on top of everything else.
      // 2px amber line at the dragged x.
      if (userTrim) {
        const x = userTrim.currentX;
        const yTop = RULER_HEIGHT;
        const yBot = RULER_HEIGHT + laneHeight * snapshot.tracks.length;
        ctx.fillStyle = "#78b8ff";
        ctx.fillRect(x - 1, yTop, 2, yBot - yTop);
      }

      if (userMove) {
        drawMoveGhost(ctx, snapshot, currentTime, userMove, pps, laneHeight);
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
    trackZoom,
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

  // Premiere/Resolve convention: Shift+Delete = ripple delete (close
  // the gap by shifting downstream clips left). Plain Delete keeps the
  // gap. The ripple-delete path sends one `ripple_delete` per clip
  // (link-group siblings ride along inside the backend op).
  const commitRippleDeleteSelection = useCallback(async (): Promise<void> => {
    if (proposal || !selectedClipKey) return;
    const selectedTrack = snapshot.tracks[selectedClipKey.trackIndex];
    const selectedItem = selectedTrack?.items.find(
      (it) => it.index === selectedClipKey.clipIndex,
    );
    if (!selectedItem || selectedItem.kind !== "clip") return;
    try {
      await invoke<string>("propose_user_edit", {
        edlText: serializeEdl([
          {
            kind: "ripple_delete",
            anchor: { kind: "clip_uuid", uuid: selectedItem.clip_uuid },
          },
        ]),
      });
      clearSelection();
    } catch (err) {
      // eslint-disable-next-line no-console
      console.warn("propose_user_edit (ripple delete) failed", err);
    }
  }, [clearSelection, proposal, selectedClipKey, snapshot]);

  const commitDeleteSelection = useCallback(async (): Promise<void> => {
    if (proposal || !selectedClipKey) return;
    const selectedTrack = snapshot.tracks[selectedClipKey.trackIndex];
    const selectedItem = selectedTrack?.items.find(
      (it) => it.index === selectedClipKey.clipIndex,
    );
    if (!selectedItem) return;

    if (selectedItem.kind === "transition") {
      const position = selectedTrack.items.findIndex(
        (item) => item.index === selectedItem.index,
      );
      const from = selectedTrack.items[position - 1];
      const to = selectedTrack.items[position + 1];
      if (from?.kind !== "clip" || to?.kind !== "clip") return;
      try {
        await invoke<string>("propose_user_edit", {
          edlText: serializeEdl([
            {
              kind: "delete_transition",
              from: { kind: "clip_uuid", uuid: from.clip_uuid },
              to: { kind: "clip_uuid", uuid: to.clip_uuid },
            },
          ]),
        });
        clearSelection();
      } catch (err) {
        // eslint-disable-next-line no-console
        console.warn("propose_user_edit (delete transition) failed", err);
      }
      return;
    }

    if (selectedItem.kind !== "clip") return;

    const clips =
      selectedItem.link_group_id !== null
        ? snapshot.tracks.flatMap((track) =>
            track.items.filter(
              (it): it is Extract<TimelineItem, { kind: "clip" }> =>
                it.kind === "clip" &&
                it.link_group_id === selectedItem.link_group_id,
            ),
          )
        : [selectedItem];
    const seen = new Set<string>();
    const ops = clips
      .filter((clip) => {
        if (seen.has(clip.clip_uuid)) return false;
        seen.add(clip.clip_uuid);
        return true;
      })
      .map((clip) => ({
        kind: "delete_clip" as const,
        anchor: { kind: "clip_uuid" as const, uuid: clip.clip_uuid },
      }));
    if (ops.length === 0) return;

    try {
      await invoke<string>("propose_user_edit", {
        edlText: serializeEdl(ops),
      });
      clearSelection();
    } catch (err) {
      // eslint-disable-next-line no-console
      console.warn("propose_user_edit (delete clip) failed", err);
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
  }, [commitDeleteSelection, commitRippleDeleteSelection, proposal, selectedClipKey]);

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
    const hit = hitTestEdge(x, y, snapshot, ppsRef.current, laneHeightRef.current);
    if (hit) {
      e.currentTarget.setPointerCapture(e.pointerId);
      setUserTrim({ hit, startX: x, currentX: x, ripple: e.metaKey || e.ctrlKey });
      // Don't seek on edge-down; the user is starting a trim, not
      // scrubbing.
      return;
    }
    // No edge hit — update the properties-pane selection. Clip body
    // under the pointer = select; empty space = clear. Either way
    // the click also scrubs the playhead (preserving the existing
    // seek-on-click behaviour).
    const body = hitTestSelectableBody(x, y, snapshot, ppsRef.current, laneHeightRef.current);
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
    const hover = hitTestEdge(x, y, snapshot, ppsRef.current, laneHeightRef.current);
    setEdgeHover(hover);
    // Hover tooltip — set the canvas's `title` attribute to the full
    // clip name so the user can see it even when the label is middle-
    // truncated on a narrow clip. Native browser tooltip is enough;
    // no need for a custom overlay component.
    const body = hitTestSelectableBody(
      x,
      y,
      snapshot,
      ppsRef.current,
      laneHeightRef.current,
    );
    if (body) {
      const item = snapshot.tracks[body.trackIndex]?.items.find(
        (candidate) => candidate.index === body.clipIndex,
      );
      if (item?.kind === "clip") {
        e.currentTarget.title = item.name;
      } else {
        e.currentTarget.title = "";
      }
    } else {
      e.currentTarget.title = "";
    }
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
    const dxPx = drag.currentX - drag.startX;
    const rawDxS = pxDeltaToSourceDelta(dxPx, ppsRef.current);
    // Tiny drags are scrub mistakes, not trim intent. Don't send.
    if (Math.abs(dxPx) < 2) return;
    const { hit } = drag;
    // Snap the dragged edge's *track-time* position to nearby clip
    // edges + the playhead, then convert back to a source-time delta.
    // Within a single clip source-time and track-time are 1:1, so the
    // dx applies symmetrically to either axis.
    const trackEdgeBefore =
      hit.side === "start"
        ? trackTimeForSourceEdge(snapshot, hit.clipUuid, "start")
        : trackTimeForSourceEdge(snapshot, hit.clipUuid, "end");
    const targets = collectSnapTargets(snapshot, {
      playheadS: currentTime,
      excludeClipUuids: new Set([hit.clipUuid]),
    });
    const snappedTrackEdge =
      trackEdgeBefore !== null
        ? snapTime(trackEdgeBefore + rawDxS, targets, SNAP_TOLERANCE_S)
        : null;
    const dxS =
      snappedTrackEdge !== null && trackEdgeBefore !== null
        ? snappedTrackEdge - trackEdgeBefore
        : rawDxS;
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
      drag.ripple
        ? {
            kind: "ripple_trim",
            anchor: { kind: "clip_uuid", uuid: hit.clipUuid },
            edge: hit.side === "start" ? "start" : "end",
            valueS: hit.side === "start" ? newStart : newEnd,
          }
        : {
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

  async function commitUserMove(drag: UserMoveDrag): Promise<void> {
    const dxPx = drag.currentX - drag.startX;
    const dyPx = drag.currentY - drag.startY;
    if (Math.hypot(dxPx, dyPx) < 5) return;
    const dxS = snapMoveDeltaS(snapshot, currentTime, drag, ppsRef.current);
    if (Math.abs(dxS) < 0.01) return;
    const primaryTrack = snapshot.tracks[drag.trackIndex];
    const primary = primaryTrack?.items.find(
      (item) => item.kind === "clip" && item.index === drag.clipIndex,
    );
    if (!primary || primary.kind !== "clip") return;

    // One ripple_move op — the backend shifts the moved clip + every
    // clip after it on its track, plus the moved clip's link-group
    // siblings on other tracks by the same delta. Matches
    // Resolve/Premiere default body-drag behavior.
    const op = {
      kind: "ripple_move" as const,
      anchor: { kind: "clip_uuid" as const, uuid: primary.clip_uuid },
      deltaS: dxS,
    };

    try {
      await invoke<string>("propose_user_edit", {
        edlText: serializeEdl([op]),
      });
    } catch (err) {
      // eslint-disable-next-line no-console
      console.warn("propose_user_edit (ripple move) failed", err);
    }
  }

  function onPointerLeave() {
    setEdgeHover(null);
  }

  function onDragOver(e: React.DragEvent<HTMLCanvasElement>) {
    if (proposal) return;
    if (e.dataTransfer.types.includes("application/x-awidat-media")) {
      e.preventDefault();
      e.dataTransfer.dropEffect = "copy";
    }
  }

  async function onDrop(e: React.DragEvent<HTMLCanvasElement>) {
    if (proposal) return;
    const assetId = e.dataTransfer.getData("application/x-awidat-media");
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
          No clips on the timeline yet. Drag source media here, use
          Add to timeline, or ask the agent for a first cut.
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

function UserMoveTooltip({
  drag,
  snapshot,
  currentTime,
  pps,
}: {
  drag: UserMoveDrag;
  snapshot: TimelineSnapshot;
  currentTime: number;
  pps: number;
}) {
  const dxS = snapMoveDeltaS(snapshot, currentTime, drag, pps);
  return (
    <div className="user-trim-tooltip" style={{ left: drag.currentX }}>
      move {dxS >= 0 ? "+" : ""}
      {dxS.toFixed(2)}s
    </div>
  );
}

function computePps(durationS: number, cssWidth: number, zoom: number): number {
  const fitPps =
    durationS > 0 ? Math.max(0.05, (cssWidth - 8) / durationS) : PX_PER_SECOND_BASE;
  return Math.min(fitPps * zoom, PX_PER_SECOND_BASE * 8);
}

function isEditableTarget(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  if (target.isContentEditable) return true;
  const tag = target.tagName.toLowerCase();
  return tag === "input" || tag === "textarea" || tag === "select";
}

/** Resolve a clip's current track-time start or end position from a
 *  snapshot, looked up by anchor uuid. Returns null if the clip isn't
 *  present (e.g. proposal pruned it). Used by trim-snap to convert
 *  the source-time edge being dragged into a timeline-time target so
 *  it can snap against neighbor clip edges + the playhead. */
function trackTimeForSourceEdge(
  snapshot: TimelineSnapshot,
  clipUuid: string,
  side: "start" | "end",
): number | null {
  for (const track of snapshot.tracks) {
    for (const item of track.items) {
      if (item.kind !== "clip") continue;
      if (item.clip_uuid !== clipUuid) continue;
      return side === "start"
        ? item.track_start_s
        : item.track_start_s + item.duration_s;
    }
  }
  return null;
}

function snapMoveDeltaS(
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

function drawMoveGhost(
  ctx: CanvasRenderingContext2D,
  snapshot: TimelineSnapshot,
  currentTime: number,
  drag: UserMoveDrag,
  pps: number,
  laneHeight: number,
) {
  const dx = snapMoveDeltaS(snapshot, currentTime, drag, pps) * pps;
  const drawClip = (trackIndex: number, item: Extract<TimelineItem, { kind: "clip" }>) => {
    const x = Math.round(item.track_start_s * pps + dx);
    const y = RULER_HEIGHT + trackIndex * laneHeight + 4;
    const w = Math.max(2, Math.round(item.duration_s * pps));
    const h = laneHeight - 8;
    ctx.save();
    ctx.setLineDash([5, 4]);
    ctx.strokeStyle = "#78b8ff";
    ctx.lineWidth = 2;
    strokeRoundedRect(ctx, x + 0.5, y + 0.5, w - 1, h - 1, 4);
    ctx.restore();
  };

  if (drag.linkGroupId !== null) {
    for (let trackIndex = 0; trackIndex < snapshot.tracks.length; trackIndex += 1) {
      for (const item of snapshot.tracks[trackIndex].items) {
        if (item.kind === "clip" && item.link_group_id === drag.linkGroupId) {
          drawClip(trackIndex, item);
        }
      }
    }
    return;
  }

  const item = snapshot.tracks[drag.trackIndex]?.items.find(
    (candidate): candidate is Extract<TimelineItem, { kind: "clip" }> =>
      candidate.kind === "clip" && candidate.index === drag.clipIndex,
  );
  if (item) {
    drawClip(drag.trackIndex, item);
  }
}

/** Walk every video-bearing clip in the timeline and emit (start, end,
 *  kind) ranges for the cache strip. Audio-only clips are skipped —
 *  the strip is about visual playback, not audio buffering. */
function collectCacheStripInputs(
  tracks: { kind: string; items: TimelineItem[] }[],
): CacheStripInput[] {
  const ranges: CacheStripInput[] = [];
  for (const track of tracks) {
    if (track.kind === "audio") continue;
    for (const item of track.items) {
      if (item.kind !== "clip") continue;
      if (item.duration_s <= 0) continue;
      const start = item.track_start_s;
      const end = start + item.duration_s;
      const kind = item.playable_kind ?? "missing";
      ranges.push({ startS: start, endS: end, kind });
    }
  }
  // Order by start time so cacheStripSegments can merge consecutive
  // same-kind ranges in one pass.
  ranges.sort((a, b) => a.startS - b.startS);
  return ranges;
}

function cacheStripColor(kind: CacheStripInput["kind"]): string {
  switch (kind) {
    case "proxy":   return "#5ab676"; // var(--accent-success)
    case "source":  return "#d8a043"; // var(--accent-warning)
    case "missing": return "#e25b5b"; // var(--accent-danger)
  }
}

/** Draw the time ruler with tick marks every 1, 5, or 10 seconds
 *  depending on zoom. Larger ticks are labeled. */
function drawRuler(
  ctx: CanvasRenderingContext2D,
  width: number,
  duration: number,
  pps: number,
  cacheSegments: CacheStripInput[],
) {
  ctx.fillStyle = "#151711";
  ctx.fillRect(0, 0, width, RULER_HEIGHT);
  ctx.strokeStyle = "#30352d";
  ctx.beginPath();
  ctx.moveTo(0, RULER_HEIGHT - 0.5);
  ctx.lineTo(width, RULER_HEIGHT - 0.5);
  ctx.stroke();

  // Choose a tick interval that gives ~1 tick every 60-80px.
  const desiredPx = 64;
  const candidates = [0.5, 1, 2, 5, 10, 30, 60, 120, 300];
  let interval =
    candidates.find((c) => c * pps >= desiredPx) ?? candidates[candidates.length - 1];

  ctx.fillStyle = "#a49f91";
  ctx.font =
    "11px ui-monospace, SFMono-Regular, 'SF Mono', Menlo, monospace";
  ctx.textBaseline = "middle";

  for (let t = 0; t <= duration + interval; t += interval) {
    const x = Math.round(t * pps) + 0.5;
    if (x > width) break;
    ctx.strokeStyle = "#30352d";
    ctx.beginPath();
    ctx.moveTo(x, RULER_HEIGHT - 8);
    ctx.lineTo(x, RULER_HEIGHT);
    ctx.stroke();
    ctx.fillText(formatTime(t), x + 4, RULER_HEIGHT / 2 - 1);
  }

  // Paint the cache strip — a thin colored bar along the bottom edge
  // of the ruler that tints each region by cache state. Helps the user
  // see at a glance which parts of the timeline will play smoothly
  // (proxy = green), which fall back to source (amber), and which are
  // missing media (red).
  const stripHeight = 3;
  const stripY = RULER_HEIGHT - stripHeight - 0.5;
  for (const seg of cacheSegments) {
    const x0 = Math.max(0, Math.round(seg.startS * pps));
    const x1 = Math.min(width, Math.round(seg.endS * pps));
    if (x1 <= x0) continue;
    ctx.fillStyle = cacheStripColor(seg.kind);
    ctx.fillRect(x0, stripY, x1 - x0, stripHeight);
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
  laneHeight: number,
  opts: {
    deletedKeys?: Set<string>;
    highlightKeys?: Set<string>;
    selectedKey?: string;
  },
) {
  for (let row = 0; row < tracks.length; row++) {
    const track = tracks[row];
    const y = RULER_HEIGHT + row * laneHeight;
    const isTitlesRow = track.role === "titles";

    // Lane background. Titles row gets a darker amber-tinted band
    // so it reads as a different kind of layer; audio is the
    // existing dark-green ish; video lanes keep their default tint.
    if (isTitlesRow) {
      ctx.fillStyle = "#070b10";
    } else if (track.kind === "audio") {
      ctx.fillStyle = "#0b100d";
    } else {
      ctx.fillStyle = "#0d0f0d";
    }
    ctx.fillRect(0, y, width, laneHeight);
    ctx.strokeStyle = "#30352d";
    ctx.beginPath();
    ctx.moveTo(0, y + laneHeight - 0.5);
    ctx.lineTo(width, y + laneHeight - 0.5);
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
        laneHeight - 8,
        track.kind,
        flag,
        selected,
        isTitlesRow,
      );
    }
  }
}

type ItemFlag = "normal" | "deleted" | "highlight";

/** Per-role fill + stroke colors for a clip rect. Tuned for a calm,
 *  DaVinci-style timeline: fills are desaturated blue/green-grey so
 *  the clips read as material rather than glowing buttons; strokes
 *  are barely-there white hairlines — only selected/changed clips
 *  light up (handled by the `flag === "highlight"` and `selected`
 *  branches in `drawItem`). Title clips keep an amber-on-black band
 *  because that's a chyron, not a media clip. */
function clipRoleColors(
  item: Extract<TimelineItem, { kind: "clip" }>,
  trackKind: string,
  isTitleClip: boolean,
): { fill: string; stroke: string } {
  if (isTitleClip) {
    return { fill: "#1f1206", stroke: "rgba(228, 174, 82, 0.55)" };
  }
  if (trackKind === "audio") {
    return { fill: "#1f4d3d", stroke: "rgba(255, 255, 255, 0.08)" };
  }
  if (item.video_overlay) {
    const mode = item.video_overlay.mode;
    // PiP corner overlays vs full-frame b-roll cutaways still get
    // distinct fills so the editor can spot insert/overlay layers
    // at a glance — but strokes stay quiet across the board.
    if (mode === "corner") {
      return { fill: "#1c3a45", stroke: "rgba(255, 255, 255, 0.08)" };
    }
    return { fill: "#2f2843", stroke: "rgba(255, 255, 255, 0.08)" };
  }
  return { fill: "#263241", stroke: "rgba(255, 255, 255, 0.08)" };
}

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
    const roleColors = clipRoleColors(item, trackKind, isTitleClip);
    ctx.fillStyle = roleColors.fill;
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
    // proposal), otherwise the role's accent.
    const stroke =
      flag === "deleted"
        ? "#ef7168"
        : flag === "highlight"
        ? "#78b8ff"
        : roleColors.stroke;
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
      const label = truncateToWidthMiddle(ctx, item.name, w - 2 * CLIP_PADDING_X);
      const labelY = y + h / 2;
      if (drewOverlay) {
        const metrics = ctx.measureText(label);
        ctx.fillStyle = "rgba(5, 6, 5, 0.74)";
        ctx.fillRect(
          x + CLIP_PADDING_X - 2,
          labelY - 7,
          Math.min(w - 2 * CLIP_PADDING_X + 4, metrics.width + 4),
          14,
        );
      }
      // Muted label — pro NLEs keep clip labels dim so the filmstrip
      // / waveform reads as the clip's identity, with the label as
      // supporting metadata. Bright white labels make every clip
      // shout.
      ctx.fillStyle = "rgba(238, 232, 215, 0.75)";
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
      ctx.strokeStyle = "#ef7168";
      ctx.lineWidth = 1.5;
      ctx.beginPath();
      ctx.moveTo(x + 2, y + h / 2);
      ctx.lineTo(x + w - 2, y + h / 2);
      ctx.stroke();
      ctx.lineWidth = 1;
    }
    // Edit handles — small bracket marks at each clip edge, modeled on
    // DaVinci's trim affordance. Painted last (before the selection
    // ring) so they sit on top of the filmstrip/waveform and the
    // strike-through. Skip when the clip is too narrow to fit them
    // without overlapping each other, and skip on title clips since
    // titles don't trim like regular media.
    if (w > 32 && !isTitleClip) {
      drawClipEditHandles(ctx, x, y, w, h, stroke);
    }
    if (selected) {
      // Outer amber ring to show "this clip is the inspector's
      // current target". Drawn after the regular border so it
      // wins z-order; stays visible when the clip is also a
      // proposal highlight (amber over amber is still distinct
      // because the proposal stroke is inside the rect, this
      // one straddles it).
      ctx.strokeStyle = "#91d7ff";
      ctx.lineWidth = 2;
      strokeRoundedRect(ctx, x - 0.5, y - 0.5, w + 1, h + 1, radius + 1);
      ctx.lineWidth = 1;
    }
  } else if (item.kind === "gap") {
    ctx.fillStyle = "rgba(164, 159, 145, 0.12)";
    fillRoundedRect(ctx, x, y, w, h, radius);
    // Cross-hatch pattern feel via dashed border so gaps stand out.
    ctx.strokeStyle = "#30352d";
    ctx.setLineDash([3, 3]);
    strokeRoundedRect(ctx, x + 0.5, y + 0.5, w - 1, h - 1, radius);
    ctx.setLineDash([]);
  } else {
    // transition
    ctx.fillStyle = "rgba(120, 184, 255, 0.18)";
    fillRoundedRect(ctx, x, y, w, h, radius);
    ctx.strokeStyle = "#78b8ff";
    strokeRoundedRect(ctx, x + 0.5, y + 0.5, w - 1, h - 1, radius);
    if (w > 30) {
      ctx.font = "10px ui-sans-serif, system-ui, sans-serif";
      ctx.textBaseline = "middle";
      ctx.fillStyle = "#c9e7ff";
      const label = truncateToWidth(
        ctx,
        `${transitionLabel(item.effect_name)} ${item.duration_s.toFixed(2)}s`,
        w - 2 * CLIP_PADDING_X,
      );
      ctx.fillText(label, x + CLIP_PADDING_X, y + h / 2);
    }
    if (selected) {
      ctx.strokeStyle = "#91d7ff";
      ctx.lineWidth = 2;
      strokeRoundedRect(ctx, x - 0.5, y - 0.5, w + 1, h + 1, radius + 1);
      ctx.lineWidth = 1;
    }
  }
}

function transitionLabel(effectName: string): string {
  switch (effectName) {
    case "SMPTE_Dissolve":
    case "awidat.cross_dissolve":
    case "fade":
      return "Dissolve";
    case "awidat.fade_black":
    case "fadeblack":
      return "Fade Black";
    case "awidat.flash_white":
    case "fadewhite":
      return "Flash";
    case "awidat.slide_left":
    case "slideleft":
      return "Slide L";
    case "awidat.slide_right":
    case "slideright":
      return "Slide R";
    case "awidat.smooth_push_left":
    case "smoothleft":
      return "Push L";
    case "awidat.wipe_left":
    case "wipeleft":
      return "Wipe L";
    case "awidat.wipe_right":
    case "wiperight":
      return "Wipe R";
    case "awidat.zoom_in":
    case "zoomin":
      return "Zoom In";
    case "awidat.pixelize":
    case "pixelize":
      return "Pixelize";
    case "awidat.radial":
    case "radial":
      return "Radial";
    default:
      return effectName.replace(/^awidat\./, "").replace(/_/g, " ");
  }
}

type EditorialMarker = {
  key: string;
  x: number;
  y: number;
  label: string;
  title: string;
};

function buildCutBadges(
  snapshot: TimelineSnapshot,
  pps: number,
  laneHeight: number,
): EditorialMarker[] {
  const out: EditorialMarker[] = [];
  for (const boundary of snapshot.cut_boundaries) {
    const located = locateClipByUuid(snapshot, boundary.to_clip_id);
    if (!located) continue;
    out.push({
      key: `cut-${boundary.key}`,
      x: Math.max(2, located.item.track_start_s * pps - 10),
      y: RULER_HEIGHT + located.trackIndex * laneHeight + 2,
      label: shortCutLabel(boundary.cut_type),
      title: [
        formatEditorialLabel(boundary.cut_type),
        boundary.intent ? `intent: ${boundary.intent}` : null,
        boundary.audio_relation ? `audio: ${boundary.audio_relation}` : null,
        boundary.reason,
      ]
        .filter(Boolean)
        .join(" - "),
    });
  }
  return out;
}

function buildSplitOffsets(
  snapshot: TimelineSnapshot,
  pps: number,
  laneHeight: number,
): EditorialMarker[] {
  const out: EditorialMarker[] = [];
  for (let trackIndex = 0; trackIndex < snapshot.tracks.length; trackIndex += 1) {
    const track = snapshot.tracks[trackIndex];
    for (const item of track.items) {
      if (item.kind !== "clip") continue;
      const y = RULER_HEIGHT + trackIndex * laneHeight + laneHeight - 18;
      if (item.audio_lead_s !== null && item.audio_lead_s > 0) {
        out.push({
          key: `lead-${item.clip_uuid}`,
          x: Math.max(2, item.track_start_s * pps + 4),
          y,
          label: `J +${formatMarkerSeconds(item.audio_lead_s)}`,
          title: splitOffsetTitle("Audio lead", item.audio_lead_s, item),
        });
      }
      if (item.audio_trail_s !== null && item.audio_trail_s > 0) {
        out.push({
          key: `trail-${item.clip_uuid}`,
          x: Math.max(2, (item.track_start_s + item.duration_s) * pps - 50),
          y,
          label: `L +${formatMarkerSeconds(item.audio_trail_s)}`,
          title: splitOffsetTitle("Audio trail", item.audio_trail_s, item),
        });
      }
    }
  }
  return out;
}

function locateClipByUuid(snapshot: TimelineSnapshot, clipUuid: string) {
  for (let trackIndex = 0; trackIndex < snapshot.tracks.length; trackIndex += 1) {
    const item = snapshot.tracks[trackIndex].items.find(
      (candidate) => candidate.kind === "clip" && candidate.clip_uuid === clipUuid,
    );
    if (item?.kind === "clip") return { trackIndex, item };
  }
  return null;
}

function shortCutLabel(cutType: string): string {
  switch (cutType) {
    case "cut_on_action":
      return "Action";
    case "shot_reverse_shot":
      return "S/RS";
    case "eyeline_match_cut":
      return "Eye";
    case "match_cut":
      return "Match";
    case "smash_cut":
      return "Smash";
    case "cross_cut":
      return "Cross";
    default:
      return formatEditorialLabel(cutType);
  }
}

function formatEditorialLabel(value: string): string {
  return value
    .split("_")
    .filter(Boolean)
    .map((part) => part.charAt(0).toUpperCase() + part.slice(1))
    .join(" ");
}

function formatMarkerSeconds(value: number): string {
  return `${value.toFixed(2).replace(/\.?0+$/, "")}s`;
}

function splitOffsetTitle(
  label: string,
  seconds: number,
  item: Extract<TimelineItem, { kind: "clip" }>,
): string {
  return [
    `${label}: ${formatMarkerSeconds(seconds)}`,
    item.split_edit_reason,
    item.split_edit_confidence !== null
      ? `confidence ${Math.round(item.split_edit_confidence * 100)}%`
      : null,
  ]
    .filter(Boolean)
    .join(" - ");
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
  // Desaturated white-green at ~50% alpha — reads as "waveform"
  // (mental model: subtle green = audio) without competing with
  // selection / playhead colors. Matches the calm clip palette.
  ctx.strokeStyle = "rgba(170, 205, 185, 0.5)";
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

/** Paint DaVinci-style trim brackets at each end of a clip rect.
 *
 *  Each handle is a short vertical bar with two horizontal nubs at the
 *  top and bottom (a `[` on the head, a `]` on the tail). Drawn in the
 *  clip's stroke color so they read as part of the clip frame. These
 *  are purely visual today — actual trim hit-detection happens in
 *  [`hitDetect.ts`] and operates on the clip's edge regions regardless
 *  of whether handles are drawn. */
function drawClipEditHandles(
  ctx: CanvasRenderingContext2D,
  x: number,
  y: number,
  w: number,
  h: number,
  color: string,
) {
  const inset = 3;
  const nubLength = 4;
  const verticalMargin = Math.min(4, h * 0.15);
  const topY = y + verticalMargin;
  const bottomY = y + h - verticalMargin;
  const leftX = x + inset + 0.5;
  const rightX = x + w - inset - 0.5;
  ctx.strokeStyle = color;
  ctx.lineWidth = 1.5;
  ctx.beginPath();
  // Head bracket — `[`
  ctx.moveTo(leftX + nubLength, topY);
  ctx.lineTo(leftX, topY);
  ctx.lineTo(leftX, bottomY);
  ctx.lineTo(leftX + nubLength, bottomY);
  // Tail bracket — `]`
  ctx.moveTo(rightX - nubLength, topY);
  ctx.lineTo(rightX, topY);
  ctx.lineTo(rightX, bottomY);
  ctx.lineTo(rightX - nubLength, bottomY);
  ctx.stroke();
  ctx.lineWidth = 1;
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
  ctx.fillStyle = "#91d7ff";
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
    ctx.fillStyle = "rgba(5, 6, 5, 0.78)";
    ctx.fillRect(boxX, boxY, boxW, boxH);
    ctx.fillStyle = "#91d7ff";
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
    } else if (
      d.kind === "insert" ||
      d.kind === "insert_b_roll" ||
      d.kind === "insert_pi_p"
    ) {
      out.add(`${d.track_index}:${d.item_index}`);
    } else if (d.kind === "move") {
      out.add(`${d.to_track_index}:${d.to_item_index}`);
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
  ctx.strokeStyle = "#ef7168";
  ctx.lineWidth = 1.5;
  ctx.beginPath();
  ctx.moveTo(x, 0);
  ctx.lineTo(x, height);
  ctx.stroke();
  // Triangle handle at top.
  ctx.fillStyle = "#ef7168";
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

/** Middle-truncate `text` so head + ellipsis + tail fits within
 *  `maxWidth`. Useful for filenames where the unique part is at the
 *  end (e.g. `copy_F65206FA-9AEC-4CA8-BD2F-ACD63775F7FA` → end
 *  truncation hides the disambiguating suffix; middle-truncation
 *  preserves it). Falls back to end-truncation when the string is
 *  short enough that splitting it doesn't help. */
function truncateToWidthMiddle(
  ctx: CanvasRenderingContext2D,
  text: string,
  maxWidth: number,
): string {
  if (ctx.measureText(text).width <= maxWidth) return text;
  if (text.length < 8) return truncateToWidth(ctx, text, maxWidth);
  // Binary search on the split point — keep `n` chars at each end,
  // shrinking until head+ellipsis+tail fits. Bias slightly toward
  // the tail (where the unique part of UUID-style names lives).
  let lo = 1;
  let hi = Math.floor(text.length / 2);
  let best = "";
  while (lo <= hi) {
    const n = Math.floor((lo + hi) / 2);
    const head = text.slice(0, n);
    const tail = text.slice(text.length - n);
    const candidate = `${head}…${tail}`;
    if (ctx.measureText(candidate).width <= maxWidth) {
      best = candidate;
      lo = n + 1;
    } else {
      hi = n - 1;
    }
  }
  return best || truncateToWidth(ctx, text, maxWidth);
}

function formatTime(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds < 0) return "0:00";
  const m = Math.floor(seconds / 60);
  const s = Math.floor(seconds % 60);
  return `${m}:${s.toString().padStart(2, "0")}`;
}
