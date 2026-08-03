// Bottom-row timeline shell. Owns project refresh, the wheel-zoom
// listener, the timeline header (zoom controls + add track), and
// delegates the editing surface to <TimelineSurface>.

import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { Pause, Play, Redo2, Scissors, SkipBack, SkipForward, Undo2 } from "lucide-react";
import { useTimelineStore } from "./store";
import { useMediaStore } from "../media/store";
import { useAgentStore } from "../agent/store";
import { useProjectStore } from "../app/state";
import { ProposalActions } from "./ProposalActions";
import { TIMELINE_CHANGED_EVENT } from "../protocol";
import { TimelineSurface } from "./TimelineSurface.tsx";
import { computePps } from "./layout.ts";
import { countCompletedTimelineEdits } from "./refreshActivity.ts";
import { useTimelineSelectionStore } from "../properties/store.ts";
import { useProposalStore } from "./proposal.ts";
import { buildSplitSelectionOps } from "./editOps.ts";
import { editorDispatch } from "../editor/tauriDispatch.ts";
import { MENU_COMMANDS, onMenuCommand } from "../app/menuCommands.ts";
import { logicalHistory, type TimelineCommit } from "./history.ts";

type TimelineHistory = {
  headRef: string | null;
  currentRef: string | null;
  undoRefs: string[];
  redoRefs: string[];
  busy: boolean;
};

type VeditRestoreResponse = {
  auditCommitHash: string | null;
  restoredParentHash: string | null;
};

type TimelinePaneProps = {
  previewRate?: number;
  onPreviewRate?: (rate: number) => void;
};

const PLAYBACK_RATES = [1, 1.5, 2] as const;

export function TimelinePane({
  previewRate = 1,
  onPreviewRate,
}: TimelinePaneProps = {}) {
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
  const selectedClipKey = useTimelineSelectionStore((s) => s.selectedClipKey);
  const activeProposal = useProposalStore((s) => s.active);
  const [editableFocus, setEditableFocus] = useState(() =>
    isEditableTarget(document.activeElement),
  );
  const [history, setHistory] = useState<TimelineHistory>({
    headRef: null,
    currentRef: null,
    undoRefs: [],
    redoRefs: [],
    busy: false,
  });
  const restoringProjectRef = useRef<string | null>(null);
  const historyRequestRef = useRef(0);

  const loadHistory = useCallback(async () => {
    const request = ++historyRequestRef.current;
    if (!projectReady) {
      setHistory({ headRef: null, currentRef: null, undoRefs: [], redoRefs: [], busy: false });
      return;
    }
    setHistory((current) => ({ ...current, busy: true }));
    try {
      const commits = await invoke<TimelineCommit[]>("list_vedit_commits", { limit: 200 });
      if (request !== historyRequestRef.current) return;
      const logical = logicalHistory(commits);
      setHistory({
        headRef: commits[0]?.commitHash ?? null,
        currentRef: logical.currentRef,
        undoRefs: logical.undoRefs,
        redoRefs: [],
        busy: false,
      });
    } catch (error) {
      if (request !== historyRequestRef.current) return;
      console.warn("list_vedit_commits failed", error);
      setHistory({ headRef: null, currentRef: null, undoRefs: [], redoRefs: [], busy: false });
    }
  }, [projectReady]);

  // Refresh on mount + on project change.
  useEffect(() => {
    if (projectReady) refresh();
    void loadHistory();
  }, [loadHistory, projectReady, projectRoot, refresh]);

  useEffect(() => {
    const onFocusIn = (event: FocusEvent) => {
      setEditableFocus(isEditableTarget(event.target));
    };
    const onFocusOut = (event: FocusEvent) => {
      if (!isEditableTarget(event.relatedTarget)) setEditableFocus(false);
    };
    document.addEventListener("focusin", onFocusIn);
    document.addEventListener("focusout", onFocusOut);
    return () => {
      document.removeEventListener("focusin", onFocusIn);
      document.removeEventListener("focusout", onFocusOut);
    };
  }, []);

  useEffect(() => {
    const unlisten = listen<string>(TIMELINE_CHANGED_EVENT, (event) => {
      if (useProjectStore.getState().current === event.payload) {
        refresh();
        if (restoringProjectRef.current === event.payload) {
          restoringProjectRef.current = null;
        } else {
          void loadHistory();
        }
      }
    });
    return () => {
      unlisten.then((u) => u());
    };
  }, [loadHistory, refresh]);

  // Refresh after every completed apply_edl OR every completed
  // proposed_edit. Both paths can mutate the OTIO on disk.
  const completedEdits = countCompletedTimelineEdits(items);
  useEffect(() => {
    if (projectReady && completedEdits > 0) {
      refresh();
      void loadHistory();
    }
  }, [completedEdits, loadHistory, projectReady, refresh]);

  const restoreRef = useCallback(async (direction: "undo" | "redo") => {
    if (
      !projectRoot ||
      history.busy ||
      !history.headRef ||
      !history.currentRef ||
      activeProposal
    ) return;
    const refs = direction === "undo" ? history.undoRefs : history.redoRefs;
    const target = refs[0];
    if (!target) return;
    const previous = history.currentRef;
    const request = ++historyRequestRef.current;
    setHistory((current) => ({ ...current, busy: true }));
    restoringProjectRef.current = projectRoot;
    try {
      const restored = await invoke<VeditRestoreResponse | null>("restore_vedit_ref", {
        refstr: target,
        expectedCurrent: history.headRef,
      });
      if (
        request !== historyRequestRef.current ||
        useProjectStore.getState().current !== projectRoot
      ) {
        if (restoringProjectRef.current === projectRoot) {
          restoringProjectRef.current = null;
        }
        return;
      }
      setHistory((current) => direction === "undo"
        ? {
            headRef: restored?.auditCommitHash ?? target,
            currentRef: target,
            undoRefs: appendMissingParent(
              current.undoRefs.slice(1),
              restored?.restoredParentHash,
            ),
            redoRefs: [previous, ...current.redoRefs],
            busy: false,
          }
        : {
            headRef: restored?.auditCommitHash ?? target,
            currentRef: target,
            undoRefs: [previous, ...current.undoRefs],
            redoRefs: current.redoRefs.slice(1),
            busy: false,
          });
      await refresh();
    } catch (error) {
      if (
        request !== historyRequestRef.current ||
        useProjectStore.getState().current !== projectRoot
      ) {
        return;
      }
      restoringProjectRef.current = null;
      console.warn(`timeline ${direction} failed`, error);
      await loadHistory();
    }
  }, [activeProposal, history, loadHistory, projectRoot, refresh]);

  const splitAtPlayhead = useCallback(async () => {
    if (!projectReady || activeProposal) return;
    const ops = buildSplitSelectionOps(snapshot, selectedClipKey, currentTime);
    if (ops.length === 0) return;
    try {
      await editorDispatch.proposeUserEdit(ops);
    } catch (error) {
      console.warn("propose_user_edit (split clip) failed", error);
    }
  }, [activeProposal, currentTime, projectReady, selectedClipKey, snapshot]);

  const canUndo = projectReady && history.undoRefs.length > 0 && !history.busy && !activeProposal;
  const canRedo = projectReady && history.redoRefs.length > 0 && !history.busy && !activeProposal;
  const canSplit = projectReady && buildSplitSelectionOps(snapshot, selectedClipKey, currentTime).length > 0 && !activeProposal;

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      const modifier = event.metaKey || event.ctrlKey;
      if (!modifier || event.altKey || isEditableTarget(event.target)) return;
      const key = event.key.toLowerCase();
      if (key === "z" && event.shiftKey && canRedo) {
        event.preventDefault();
        void restoreRef("redo");
      } else if (key === "z" && !event.shiftKey && canUndo) {
        event.preventDefault();
        void restoreRef("undo");
      } else if (key === "b" && canSplit) {
        event.preventDefault();
        void splitAtPlayhead();
      }
    };
    window.addEventListener("keydown", onKeyDown);
    const unlistenMenu = onMenuCommand((id) => {
      if (handleEditableMenuCommand(id)) return;
      if (id === MENU_COMMANDS.UNDO) void restoreRef("undo");
      if (id === MENU_COMMANDS.REDO) void restoreRef("redo");
      if (id === MENU_COMMANDS.SPLIT_CLIP) void splitAtPlayhead();
    });
    return () => {
      window.removeEventListener("keydown", onKeyDown);
      unlistenMenu();
    };
  }, [canRedo, canSplit, canUndo, restoreRef, splitAtPlayhead]);

  useEffect(() => {
    void invoke("set_menu_item_enabled", {
      states: [
        { id: MENU_COMMANDS.UNDO, enabled: editableFocus || canUndo },
        { id: MENU_COMMANDS.REDO, enabled: editableFocus || canRedo },
        { id: MENU_COMMANDS.SPLIT_CLIP, enabled: !editableFocus && canSplit },
      ],
    }).catch((error) => console.warn("set edit menu state failed", error));
  }, [canRedo, canSplit, canUndo, editableFocus]);

  const stageRef = useRef<HTMLDivElement | null>(null);
  const [stageWidth, setStageWidth] = useState(0);
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
      // Linux/Windows users can hold real Ctrl for vertical zoom.
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
      // which the browser handles natively (.timeline-stage is
      // overflow-y: auto).
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

  useEffect(() => {
    const el = stageRef.current;
    if (!el) return;
    const updateWidth = () => setStageWidth(el.clientWidth);
    updateWidth();
    const observer = new ResizeObserver(updateWidth);
    observer.observe(el);
    return () => observer.disconnect();
  }, []);

  if (!projectReady) {
    return null;
  }

  return (
    <section className="timeline-pane">
      <header className="timeline-header">
        <div className="timeline-header-left">
          <span className="timeline-label">Timeline</span>
          <span className="timeline-meta">
            {snapshot.tracks.length === 0
              ? "no tracks yet"
              : `${snapshot.duration_s.toFixed(1)}s · ${snapshot.tracks.length} track${snapshot.tracks.length === 1 ? "" : "s"}`}
          </span>
          <AddTrackButton />
        </div>
        <div className="timeline-header-center">
          <TimelineTransportControls
            previewRate={previewRate}
            setPreviewRate={onPreviewRate}
          />
        </div>
        <div className="timeline-header-right">
          <div className="flex items-center gap-1" aria-label="Timeline edit controls">
            <button type="button" aria-label="Undo" title="Undo (⌘Z)" disabled={!canUndo} onClick={() => void restoreRef("undo")}>
              <Undo2 className="h-3.5 w-3.5" />
            </button>
            <button type="button" aria-label="Redo" title="Redo (⌘⇧Z)" disabled={!canRedo} onClick={() => void restoreRef("redo")}>
              <Redo2 className="h-3.5 w-3.5" />
            </button>
            <button type="button" aria-label="Split Clip at Playhead" title="Split Clip at Playhead (⌘B)" disabled={!canSplit} onClick={() => void splitAtPlayhead()}>
              <Scissors className="h-3.5 w-3.5" />
            </button>
          </div>
          <ZoomControls pps={computePps(snapshot.duration_s, stageWidth, zoom)} />
        </div>
      </header>
      <div className="timeline-stage" ref={stageRef}>
        <TimelineSurface snapshot={snapshot} currentTime={currentTime} zoom={zoom} />
        <ProposalActions />
      </div>
    </section>
  );
}

function appendMissingParent(refs: string[], parent: string | null | undefined): string[] {
  return parent && !refs.includes(parent) ? [...refs, parent] : refs;
}

function isEditableTarget(target: EventTarget | null): boolean {
  const element = target instanceof HTMLElement ? target : null;
  return !!element?.closest("input, textarea, select, [contenteditable='true']");
}

function handleEditableMenuCommand(id: string): boolean {
  if (!isEditableTarget(document.activeElement)) return false;
  if (id === MENU_COMMANDS.UNDO) {
    document.execCommand("undo");
    return true;
  }
  if (id === MENU_COMMANDS.REDO) {
    document.execCommand("redo");
    return true;
  }
  return id === MENU_COMMANDS.SPLIT_CLIP;
}

function TimelineTransportControls({
  previewRate,
  setPreviewRate,
}: {
  previewRate: number;
  setPreviewRate?: (rate: number) => void;
}) {
  const isPlaying = useMediaStore((s) => s.isPlaying);
  const timelineTime = useMediaStore((s) => s.timelineTime);
  const timelineDurationS = useMediaStore((s) => s.timelineDurationS);
  const sourceDurationS = useMediaStore((s) => s.durationS);
  const setPlaying = useMediaStore((s) => s.setPlaying);
  const requestSeek = useMediaStore((s) => s.requestSeek);
  const requestTimelineSeek = useMediaStore((s) => s.requestTimelineSeek);
  const previewDurationS = timelineDurationS > 0 ? timelineDurationS : sourceDurationS;
  const requestPreviewSeek = (timeS: number) => {
    if (timelineDurationS > 0) {
      requestTimelineSeek(timeS);
    } else {
      requestSeek(timeS);
    }
  };

  const toggle = () => {
    if (isPlaying) {
      setPlaying(false);
      return;
    }
    if (timelineDurationS > 0 && timelineTime >= timelineDurationS) {
      requestPreviewSeek(0);
    }
    setPlaying(true);
  };

  return (
    <div className="timeline-transport-controls" aria-label="Timeline playback controls">
      <button
        type="button"
        onClick={() => requestPreviewSeek(0)}
        aria-label="Jump to start"
        title="Jump to start"
      >
        <SkipBack className="h-3.5 w-3.5" />
      </button>
      <button
        type="button"
        onClick={toggle}
        aria-label={isPlaying ? "Pause timeline" : "Play timeline"}
        title="Space"
      >
        {isPlaying ? <Pause className="h-3.5 w-3.5" /> : <Play className="h-3.5 w-3.5" />}
      </button>
      <button
        type="button"
        onClick={() => requestPreviewSeek(previewDurationS)}
        aria-label="Jump to end"
        title="Jump to end"
        disabled={previewDurationS <= 0}
      >
        <SkipForward className="h-3.5 w-3.5" />
      </button>
      <select
        aria-label="Playback speed"
        value={String(previewRate)}
        onChange={(event) => setPreviewRate?.(Number(event.currentTarget.value))}
        disabled={!setPreviewRate}
      >
        {PLAYBACK_RATES.map((rate) => (
          <option key={rate} value={rate}>
            {rate}x
          </option>
        ))}
      </select>
    </div>
  );
}

/** Compact +/−/fit controls for horizontal time-zoom and vertical
 *  track-zoom. Sits in the timeline header. Keyboard shortcuts still
 *  drive the same store actions for users who prefer the menu. */
function ZoomControls({ pps }: { pps: number }) {
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
          Fit
        </button>
        <button type="button" onClick={zoomIn} aria-label="Zoom in">+</button>
      </div>
      <span className="timeline-zoom-readout" title={`${zoom.toFixed(2)}x zoom`}>
        {pps.toFixed(1)} px/s
      </span>
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

/** "+ Track" button. Single click opens a tiny menu with Video / Audio.
 *  Names auto-pick the next free V or A slot to match the renderer's
 *  V/A naming convention. */
function AddTrackButton() {
  const [open, setOpen] = useState(false);
  const snapshot = useTimelineStore((s) => s.snapshot);
  const refresh = useTimelineStore((s) => s.refresh);

  const nextName = (kind: "video" | "audio") => {
    const prefix = kind === "video" ? "V" : "A";
    const used = new Set(snapshot.tracks.map((t) => t.name));
    for (let n = 1; n < 100; n++) {
      const candidate = `${prefix}${n}`;
      if (!used.has(candidate)) return candidate;
    }
    return `${prefix}${snapshot.tracks.length + 1}`;
  };

  async function add(kind: "video" | "audio") {
    setOpen(false);
    try {
      await invoke("insert_timeline_track", {
        name: nextName(kind),
        kind,
      });
      // `timeline_changed` event triggers refresh elsewhere; nudge here
      // too so the new lane appears immediately if the listener races.
      await refresh();
    } catch (e) {
      // eslint-disable-next-line no-console
      console.warn("insert_timeline_track failed", e);
    }
  }

  return (
    <div className="timeline-add-track">
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        title="Add a track"
        className="timeline-add-track-button"
      >
        + Track
      </button>
      {open ? (
        <div className="timeline-add-track-menu" role="menu">
          <button
            type="button"
            role="menuitem"
            onClick={() => void add("video")}
          >
            Video
          </button>
          <button
            type="button"
            role="menuitem"
            onClick={() => void add("audio")}
          >
            Audio
          </button>
        </div>
      ) : null}
    </div>
  );
}
