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

import { useEffect } from "react";
import { invoke, isTauri } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { useAgentStore } from "../agent/store";
import { useProjectStore } from "../app/state";
import { clearMediaStreamUrlCache } from "../media/mediaStreamUrl";
import { useMediaStore } from "../media/store";
import { useNotesStore } from "../notes/store";
import { useTimelineSelectionStore } from "../properties/store";
import { isProposedEditItem, useProposalStore } from "../timeline/proposal";
import { useTimelineStore } from "../timeline/store";
import {
  MENU_COMMANDS,
  emitMenuCommand,
  onMenuCommand,
} from "../app/menuCommands";
import {
  ITEM_EVENT,
  MENU_COMMAND_EVENT,
  TURN_END_EVENT,
  type ItemEvent,
  type NativeMenuCommandEvent,
  type TurnEndEvent,
} from "../protocol";

export function useAppGlue() {
  const current = useProjectStore((s) => s.current);
  const setCurrent = useProjectStore((s) => s.setCurrent);
  const refresh = useProjectStore((s) => s.refresh);

  const clearAgent = useAgentStore((s) => s.clear);
  const upsertItem = useAgentStore((s) => s.upsert);
  const setRunning = useAgentStore((s) => s.setRunning);
  const setTurnError = useAgentStore((s) => s.setTurnError);
  const items = useAgentStore((s) => s.items);

  const clearProposal = useProposalStore((s) => s.clear);
  const ingestProposal = useProposalStore((s) => s.ingest);
  const activeProposal = useProposalStore((s) => s.active);

  const clearMediaSelection = useMediaStore((s) => s.select);
  const refreshTimeline = useTimelineStore((s) => s.refresh);
  const timelineDuration = useTimelineStore((s) => s.snapshot.duration_s);
  const zoomIn = useTimelineStore((s) => s.zoomIn);
  const zoomOut = useTimelineStore((s) => s.zoomOut);
  const fitZoom = useTimelineStore((s) => s.fitZoom);
  const selectedClipKey = useTimelineSelectionStore((s) => s.selectedClipKey);
  const clearSelection = useTimelineSelectionStore((s) => s.clear);
  const clearNotes = useNotesStore((s) => s.clear);
  const ingestNote = useNotesStore((s) => s.ingest);

  // Initial project refresh + timeline load.
  useEffect(() => {
    if (!isTauri()) return;
    let cancelled = false;
    refresh()
      .then(() => {
        if (!cancelled) {
          return refreshTimeline();
        }
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [refresh, refreshTimeline]);

  // Tauri event channels — items + turn-end + native menu commands.
  useEffect(() => {
    if (!isTauri()) return;
    const itemsUnlisten = listen<ItemEvent>(ITEM_EVENT, (event) => {
      const item = event.payload.item;
      if (isProposedEditItem(item)) {
        ingestProposal(item);
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
      }
      // The legacy `VIEW_*` toggles are no-ops in v2 — the new shell
      // doesn't have toggleable panes. Keeping the menu items enabled
      // (see set_menu_item_enabled below) but ignoring the commands.
    });
  }, [current, fitZoom, setCurrent, zoomIn, zoomOut]);

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
        { id: MENU_COMMANDS.IMPORT_FILES, enabled: projectLoaded && !importBusy },
        { id: MENU_COMMANDS.IMPORT_URL, enabled: projectLoaded && !importBusy },
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
    clearMediaStreamUrlCache();
    clearMediaSelection(null);
    clearSelection();
    clearNotes();
    if (isTauri() && current !== null) {
      refreshTimeline().catch(() => {});
      const retry = window.setTimeout(() => {
        refreshTimeline().catch(() => {});
      }, 500);
      return () => window.clearTimeout(retry);
    }
  }, [
    current,
    clearAgent,
    clearProposal,
    clearMediaSelection,
    clearSelection,
    clearNotes,
    refreshTimeline,
  ]);
}
