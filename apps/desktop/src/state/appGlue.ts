/**
 * useAppGlue — preserves the side effects the legacy App.tsx wired into
 * Tauri (event subscriptions, menu-command routing, project lifecycle,
 * native-menu enable/disable). The new AppShell is presentational; this
 * hook owns everything that has to keep running for the app to behave.
 *
 * Keeping these as a single hook makes the cutover reversible — the
 * legacy App.tsx and the new App.tsx both call useAppGlue() and the
 * UI underneath swaps.
 */

import { useEffect, useRef } from "react";
import { invoke, isTauri } from "@tauri-apps/api/core";
import { editorDispatch } from "../editor/tauriDispatch";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { useAgentStore } from "../agent/store";
import { useProjectStore } from "../app/state";
import { clearMediaStreamUrlCache } from "../media/mediaStreamUrl";
import { useMediaStore } from "../media/store";
import { useNotesStore } from "../notes/store";
import { type SelectedClipKey, useTimelineSelectionStore } from "../properties/store";
import { isProposedEditItem, useProposalStore } from "../timeline/proposal";
import { usePendingProposals } from "../timeline/pendingProposals";
import {
  isApprovalRequestItem,
  useBriefProposalsStore,
} from "./briefProposals";
import { serializeEdl, type EdlOp } from "../timeline/edlBuilder";
import { useTimelineStore } from "../timeline/store";
import {
  MENU_COMMANDS,
  emitMenuCommand,
  onMenuCommand,
} from "../app/menuCommands";
import { useStageStore } from "./index";
import {
  ITEM_EVENT,
  MENU_COMMAND_EVENT,
  TURN_END_EVENT,
  type ItemEvent,
  type NativeMenuCommandEvent,
  type TimelineItem,
  type TimelineSnapshot,
  type TurnEndEvent,
} from "../protocol";

export function useAppGlue() {
  const current = useProjectStore((s) => s.current);
  const setCurrent = useProjectStore((s) => s.setCurrent);
  const refresh = useProjectStore((s) => s.refresh);
  const setStage = useStageStore((s) => s.set);

  const clearAgent = useAgentStore((s) => s.clear);
  const upsertItem = useAgentStore((s) => s.upsert);
  const setRunning = useAgentStore((s) => s.setRunning);
  const setTurnError = useAgentStore((s) => s.setTurnError);
  const items = useAgentStore((s) => s.items);

  const clearProposal = useProposalStore((s) => s.clear);
  const ingestProposal = useProposalStore((s) => s.ingest);
  const activeProposal = useProposalStore((s) => s.active);
  const ingestPendingProposal = usePendingProposals((s) => s.ingest);
  const clearPendingProposals = usePendingProposals((s) => s.clear);
  const ingestBriefApproval = useBriefProposalsStore((s) => s.ingestApproval);
  const clearBriefProposals = useBriefProposalsStore((s) => s.clear);

  const clearMediaSelection = useMediaStore((s) => s.select);
  const refreshMedia = useMediaStore((s) => s.refresh);
  const refreshTimeline = useTimelineStore((s) => s.refresh);
  const timelineSnapshot = useTimelineStore((s) => s.snapshot);
  const timelineDuration = useTimelineStore((s) => s.snapshot.duration_s);
  const zoomIn = useTimelineStore((s) => s.zoomIn);
  const zoomOut = useTimelineStore((s) => s.zoomOut);
  const fitZoom = useTimelineStore((s) => s.fitZoom);
  const selectedClipKey = useTimelineSelectionStore((s) => s.selectedClipKey);
  const clearSelection = useTimelineSelectionStore((s) => s.clear);
  const clearNotes = useNotesStore((s) => s.clear);
  const ingestNote = useNotesStore((s) => s.ingest);
  const proxyBackfillKeyRef = useRef<string | null>(null);

  // Initial project refresh + timeline load.
  useEffect(() => {
    if (!isTauri()) return;
    let cancelled = false;
    refresh()
      .then(() => {
        if (!cancelled) {
          return Promise.all([refreshTimeline(), refreshMedia()]);
        }
        return undefined;
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [refresh, refreshTimeline, refreshMedia]);

  // Tauri event channels — items + turn-end + native menu commands.
  useEffect(() => {
    if (!isTauri()) return;
    const itemsUnlisten = listen<ItemEvent>(ITEM_EVENT, (event) => {
      const item = event.payload.item;
      if (isProposedEditItem(item)) {
        ingestProposal(item);
        ingestPendingProposal(item);
      }
      if (isApprovalRequestItem(item)) {
        ingestBriefApproval(item);
      }
      if (item.kind === "editorial_note") {
        void ingestNote(item);
      }
      upsertItem(item);
    });
    const endUnlisten = listen<TurnEndEvent>(TURN_END_EVENT, (event) => {
      if (event.payload.error) {
        setTurnError(event.payload.error);
      }
      setRunning(false);
    });
    const menuUnlisten = listen<NativeMenuCommandEvent>(
      MENU_COMMAND_EVENT,
      (event) => emitMenuCommand(event.payload.id),
    );
    return () => {
      itemsUnlisten.then((unlisten) => unlisten());
      endUnlisten.then((unlisten) => unlisten());
      menuUnlisten.then((unlisten) => unlisten());
    };
  }, [
    upsertItem,
    setRunning,
    setTurnError,
    ingestProposal,
    ingestPendingProposal,
    ingestBriefApproval,
    ingestNote,
  ]);

  // Native-menu command routing.
  useEffect(() => {
    if (!isTauri()) return;
    return onMenuCommand((id) => {
      if (id === MENU_COMMANDS.CLOSE_PROJECT) {
        invoke("close_project")
          .then(() => setCurrent(null))
          .catch((e) => console.warn("close_project failed", e));
      } else if (id === MENU_COMMANDS.REVEAL_PROJECT && current) {
        revealItemInDir(current).catch((e) =>
          console.warn("reveal project failed", e),
        );
      } else if (id === MENU_COMMANDS.RUN_INDEXERS && current) {
        setStage("edit");
        invoke("index_project").catch((e) =>
          console.warn("index_project failed", e),
        );
      } else if (id === MENU_COMMANDS.EXPORT_TIMELINE && current) {
        setStage("deliver");
        invoke("start_timeline_render").catch((e) =>
          console.warn("start_timeline_render failed", e),
        );
      } else if (id === MENU_COMMANDS.DELETE_CLIP && current && !activeProposal) {
        const edlText = buildDeleteSelectionEdl(timelineSnapshot, selectedClipKey);
        if (edlText) {
          invoke<string>("propose_user_edit", { edlText })
            .then(() => clearSelection())
            .catch((e) => console.warn("propose_user_edit (delete selection) failed", e));
        }
      } else if (id === MENU_COMMANDS.ACCEPT_PROPOSAL && activeProposal) {
        editorDispatch.acceptProposal(activeProposal.callId).catch((e) =>
          console.warn("accept_proposal failed", e),
        );
      } else if (id === MENU_COMMANDS.REJECT_PROPOSAL && activeProposal) {
        editorDispatch.rejectProposal(activeProposal.callId).catch((e) =>
          console.warn("reject_proposal failed", e),
        );
      } else if (id === MENU_COMMANDS.TIMELINE_ZOOM_IN) {
        zoomIn();
      } else if (id === MENU_COMMANDS.TIMELINE_ZOOM_OUT) {
        zoomOut();
      } else if (id === MENU_COMMANDS.TIMELINE_ZOOM_FIT) {
        fitZoom();
      } else if (id === MENU_COMMANDS.TOGGLE_FULLSCREEN) {
        const win = getCurrentWindow();
        win
          .isFullscreen()
          .then((fullscreen) => win.setFullscreen(!fullscreen))
          .catch((e) => console.warn("toggle fullscreen failed", e));
      } else if (id === MENU_COMMANDS.NAV_PROJECT) {
        setStage("edit");
      } else if (id === MENU_COMMANDS.NAV_WORKSPACE) {
        setStage("edit");
      } else if (id === MENU_COMMANDS.NAV_AGENT) {
        setStage("edit");
      } else if (id === MENU_COMMANDS.NAV_MEDIA || id === MENU_COMMANDS.VIEW_MEDIA) {
        setStage("edit");
      } else if (
        id === MENU_COMMANDS.NAV_REVIEW ||
        id === MENU_COMMANDS.VIEW_TIMELINE ||
        id === MENU_COMMANDS.VIEW_TRANSCRIPT ||
        id === MENU_COMMANDS.VIEW_EDITS
      ) {
        setStage("edit");
      } else if (id === MENU_COMMANDS.NAV_DELIVER) {
        setStage(current ? "deliver" : "edit");
      } else if (id === MENU_COMMANDS.NAV_SETTINGS) {
        setStage("edit");
      }
      // The legacy pane toggles that do not map to v2 lenses are kept
      // enabled for menu completeness but intentionally do not resize panes.
    });
  }, [
    activeProposal,
    clearSelection,
    current,
    fitZoom,
    selectedClipKey,
    setCurrent,
    setStage,
    timelineSnapshot,
    zoomIn,
    zoomOut,
  ]);

  // Native-menu enable/disable signaling.
  useEffect(() => {
    if (!isTauri()) return;
    const projectLoaded = current !== null;
    const proposalActive = activeProposal !== null;
    const runningKinds = items
      .filter(
        (it): it is Extract<typeof items[number], { kind: "job" }> =>
          it.kind === "job" && it.phase !== "completed",
      )
      .map((it) => it.job_kind);
    const importBusy =
      runningKinds.includes("local_import") ||
      runningKinds.includes("url_import");
    const transcodeBusy = runningKinds.includes("transcode");
    const indexBusy = runningKinds.includes("indexing");
    const exportBusy = runningKinds.includes("render");
    invoke("set_menu_item_enabled", {
      states: [
        { id: MENU_COMMANDS.CLOSE_PROJECT, enabled: projectLoaded },
        { id: MENU_COMMANDS.IMPORT_FILES, enabled: !importBusy },
        { id: MENU_COMMANDS.IMPORT_URL, enabled: !importBusy },
        {
          id: MENU_COMMANDS.RUN_INDEXERS,
          enabled: projectLoaded && !indexBusy && !transcodeBusy,
        },
        {
          id: MENU_COMMANDS.EXPORT_TIMELINE,
          enabled: projectLoaded && timelineDuration > 0 && !exportBusy,
        },
        { id: MENU_COMMANDS.REVEAL_PROJECT, enabled: projectLoaded },
        {
          id: MENU_COMMANDS.DELETE_CLIP,
          enabled: projectLoaded && selectedClipKey !== null && !proposalActive,
        },
        { id: MENU_COMMANDS.ACCEPT_PROPOSAL, enabled: proposalActive },
        { id: MENU_COMMANDS.REJECT_PROPOSAL, enabled: proposalActive },
        { id: MENU_COMMANDS.TIMELINE_ZOOM_IN, enabled: projectLoaded },
        { id: MENU_COMMANDS.TIMELINE_ZOOM_OUT, enabled: projectLoaded },
        { id: MENU_COMMANDS.TIMELINE_ZOOM_FIT, enabled: projectLoaded },
      ],
    }).catch((e) => console.warn("set_menu_item_enabled failed", e));
  }, [activeProposal, current, items, selectedClipKey, timelineDuration]);

  // Project lifecycle — reset everything when the project changes.
  useEffect(() => {
    clearAgent();
    clearProposal();
    clearPendingProposals();
    clearBriefProposals();
    clearMediaStreamUrlCache();
    clearMediaSelection(null);
    clearSelection();
    clearNotes();
    if (isTauri() && current !== null) {
      refreshTimeline().catch(() => {});
      const retry = window.setTimeout(() => {
        refreshTimeline().catch(() => {});
        refreshMedia().catch(() => {});
      }, 500);
      refreshMedia().catch(() => {});
      return () => window.clearTimeout(retry);
    }
  }, [
    current,
    clearAgent,
    clearProposal,
    clearPendingProposals,
    clearBriefProposals,
    clearMediaSelection,
    clearSelection,
    clearNotes,
    refreshTimeline,
    refreshMedia,
  ]);

  // The v2 shell no longer mounts MediaPane, so proxy discovery must
  // live with the rest of the app glue. Refresh when transcodes finish
  // and when imports complete, because both can make new proxy-backed
  // media available to the shell preview and timeline surfaces.
  useEffect(() => {
    if (!isTauri() || current === null) return;
    const completedMediaJobs = items.filter(
      (it) =>
        it.kind === "job" &&
        (it.job_kind === "transcode" ||
          it.job_kind === "local_import" ||
          it.job_kind === "url_import") &&
        it.phase === "completed",
    ).length;
    if (completedMediaJobs > 0) {
      refreshMedia().catch((e) => console.warn("refresh media failed", e));
      refreshTimeline().catch((e) => console.warn("refresh timeline failed", e));
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [
    current,
    items.filter(
      (it) =>
        it.kind === "job" &&
        (it.job_kind === "transcode" ||
          it.job_kind === "local_import" ||
          it.job_kind === "url_import") &&
        it.phase === "completed",
    ).length,
  ]);

  // If the timeline has clips with no proxy yet, make
  // proxy generation explicit from the UI side too. The backend command
  // is idempotent; this covers projects opened before the backfill ran,
  // dev hot-reloads, and stale UI states that would otherwise sit on
  // "Generating preview..." with no active transcode job.
  useEffect(() => {
    if (!isTauri() || current === null) return;
    const hasActiveTranscode = items.some(
      (it) =>
        it.kind === "job" &&
        it.job_kind === "transcode" &&
        it.phase !== "completed",
    );
    if (hasActiveTranscode) return;
    const pendingAssetIds = pendingProxyAssetIds(timelineSnapshot);
    if (pendingAssetIds.length === 0) return;
    const key = `${current}:${pendingAssetIds.join("|")}`;
    if (proxyBackfillKeyRef.current === key) return;
    proxyBackfillKeyRef.current = key;
    invoke<number>("transcode_project_proxies")
      .then((generated) => {
        if (generated > 0) {
          return Promise.all([refreshMedia(), refreshTimeline()]);
        }
        return undefined;
      })
      .catch((e) => {
        console.warn("proxy backfill failed", e);
        // Keep the key latched after a failed attempt. Otherwise a
        // persistent failure, such as a full disk, creates a tight
        // start/fail/start loop and the UI flickers between
        // processing and partial for the same asset set.
      });
  }, [current, items, refreshMedia, refreshTimeline, timelineSnapshot]);
}

function pendingProxyAssetIds(snapshot: TimelineSnapshot): string[] {
  const ids = new Set<string>();
  for (const track of snapshot.tracks ?? []) {
    if (track.kind !== "video") continue;
    for (const item of track.items ?? []) {
      if (item.kind !== "clip") continue;
      if (!item.asset_id || item.duration_s <= 0) continue;
      if (item.proxy_path !== null) continue;
      if (item.playable_kind === "proxy") continue;
      ids.add(item.asset_id);
    }
  }
  return Array.from(ids).sort();
}

function buildDeleteSelectionEdl(
  snapshot: TimelineSnapshot,
  selectedClipKey: SelectedClipKey | null,
): string | null {
  if (!selectedClipKey) return null;
  const selectedTrack = snapshot.tracks[selectedClipKey.trackIndex];
  const selectedItem = selectedTrack?.items.find(
    (item) => item.index === selectedClipKey.clipIndex,
  );
  if (!selectedTrack || !selectedItem) return null;

  if (selectedItem.kind === "transition") {
    const position = selectedTrack.items.findIndex(
      (item) => item.index === selectedItem.index,
    );
    const from = selectedTrack.items[position - 1];
    const to = selectedTrack.items[position + 1];
    if (from?.kind !== "clip" || to?.kind !== "clip") return null;
    return serializeEdl([
      {
        kind: "delete_transition",
        from: { kind: "clip_uuid", uuid: from.clip_uuid },
        to: { kind: "clip_uuid", uuid: to.clip_uuid },
      },
    ]);
  }

  if (selectedItem.kind !== "clip") return null;
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
  const ops: EdlOp[] = clips
    .filter((clip) => {
      if (seen.has(clip.clip_uuid)) return false;
      seen.add(clip.clip_uuid);
      return true;
    })
    .map((clip) => ({
      kind: "delete_clip",
      anchor: { kind: "clip_uuid", uuid: clip.clip_uuid },
    }));
  return ops.length > 0 ? serializeEdl(ops) : null;
}
