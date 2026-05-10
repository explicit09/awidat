// App shell.
//
// Layout when a project is loaded: header → action bar → split workspace
// (chat on the left, media pane on the right) → composer pinned to the
// bottom. Layout when no project is loaded: header → ChatStream's
// "open or create a project" placeholder → disabled composer.

import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { ProjectBanner } from "./app/ProjectBanner";
import { ActionBar } from "./app/ActionBar";
import { useProjectStore } from "./app/state";
import { useAgentStore } from "./agent/store";
import { ChatStream } from "./agent/ChatStream";
import { Composer } from "./agent/Composer";
import { SessionBar } from "./agent/SessionBar";
import { MediaPane } from "./media/MediaPane";
import { useMediaStore } from "./media/store";
import { TimelinePane } from "./timeline/TimelinePane";
import { isProposedEditItem, useProposalStore } from "./timeline/proposal";
import { useTimelineStore } from "./timeline/store";
import {
  MENU_COMMANDS,
  emitMenuCommand,
  onMenuCommand,
} from "./app/menuCommands";
import { PropertiesPane } from "./properties/PropertiesPane";
import { useTimelineSelectionStore } from "./properties/store";
import { NotesPanel } from "./notes/NotesPanel";
import { useNotesStore } from "./notes/store";
import { TranscriptSidebar } from "./transcript/TranscriptSidebar";
import { VeditPanel } from "./vedit/VeditPanel";
import {
  ITEM_EVENT,
  MENU_COMMAND_EVENT,
  TURN_END_EVENT,
  type ItemEvent,
  type NativeMenuCommandEvent,
  type TurnEndEvent,
} from "./protocol";
import "./App.css";

function App() {
  const [sidebarTab, setSidebarTab] =
    useState<"chat" | "transcript" | "edits">("chat");
  const [showSidebar, setShowSidebar] = useState(true);
  const [showMedia, setShowMedia] = useState(true);
  const [showProperties, setShowProperties] = useState(true);
  const [showTimeline, setShowTimeline] = useState(true);
  const [showNotes, setShowNotes] = useState(true);
  const current = useProjectStore((s) => s.current);
  const setCurrent = useProjectStore((s) => s.setCurrent);
  const refresh = useProjectStore((s) => s.refresh);
  const clearAgent = useAgentStore((s) => s.clear);
  const upsertItem = useAgentStore((s) => s.upsert);
  const setRunning = useAgentStore((s) => s.setRunning);
  const setTurnError = useAgentStore((s) => s.setTurnError);
  const clearProposal = useProposalStore((s) => s.clear);
  const ingestProposal = useProposalStore((s) => s.ingest);
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
  const activeProposal = useProposalStore((s) => s.active);
  const items = useAgentStore((s) => s.items);

  // Keep the banner's local-state callback wired to the store so any
  // path change (open / new / future close) propagates to Composer's
  // disabled flag and any future panes that key off `current`.
  useEffect(() => {
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

  // Subscribe at app-shell scope so toolbar-triggered jobs (import,
  // index, export) are captured independently of ChatStream mount
  // timing during project open/close transitions.
  useEffect(() => {
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

  useEffect(() => {
    return onMenuCommand((id) => {
      if (id === MENU_COMMANDS.CLOSE_PROJECT) {
        invoke("close_project")
          .then(() => setCurrent(null))
          .catch((e) => console.warn("close_project failed", e));
      } else if (id === MENU_COMMANDS.REVEAL_PROJECT && current) {
        revealItemInDir(current).catch((e) =>
          console.warn("reveal project failed", e),
        );
      } else if (id === MENU_COMMANDS.VIEW_MEDIA) {
        setShowMedia((v) => !v);
      } else if (id === MENU_COMMANDS.VIEW_TIMELINE) {
        setShowTimeline((v) => !v);
      } else if (id === MENU_COMMANDS.VIEW_PROPERTIES) {
        setShowProperties((v) => !v);
      } else if (id === MENU_COMMANDS.VIEW_NOTES) {
        setShowNotes((v) => !v);
      } else if (id === MENU_COMMANDS.VIEW_SIDEBAR) {
        setShowSidebar((v) => !v);
      } else if (id === MENU_COMMANDS.VIEW_CHAT) {
        setShowSidebar(true);
        setSidebarTab("chat");
      } else if (id === MENU_COMMANDS.VIEW_TRANSCRIPT) {
        setShowSidebar(true);
        setSidebarTab("transcript");
      } else if (id === MENU_COMMANDS.VIEW_EDITS) {
        setShowSidebar(true);
        setSidebarTab("edits");
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
    });
  }, [current, fitZoom, setCurrent, zoomIn, zoomOut]);

  useEffect(() => {
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
        { id: MENU_COMMANDS.VIEW_MEDIA, enabled: projectLoaded },
        { id: MENU_COMMANDS.VIEW_TIMELINE, enabled: projectLoaded },
        { id: MENU_COMMANDS.VIEW_PROPERTIES, enabled: projectLoaded },
        { id: MENU_COMMANDS.VIEW_NOTES, enabled: projectLoaded },
        { id: MENU_COMMANDS.VIEW_SIDEBAR, enabled: projectLoaded },
        { id: MENU_COMMANDS.VIEW_CHAT, enabled: projectLoaded },
        { id: MENU_COMMANDS.VIEW_TRANSCRIPT, enabled: projectLoaded },
        { id: MENU_COMMANDS.VIEW_EDITS, enabled: projectLoaded },
        { id: MENU_COMMANDS.TIMELINE_ZOOM_IN, enabled: projectLoaded },
        { id: MENU_COMMANDS.TIMELINE_ZOOM_OUT, enabled: projectLoaded },
        { id: MENU_COMMANDS.TIMELINE_ZOOM_FIT, enabled: projectLoaded },
      ],
    }).catch((e) => console.warn("set_menu_item_enabled failed", e));
  }, [activeProposal, current, items, selectedClipKey, timelineDuration]);

  useEffect(() => {
    clearAgent();
    clearProposal();
    clearMediaSelection(null);
    clearSelection();
    clearNotes();
    if (current !== null) {
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

  const projectReady = current !== null;
  const workspaceColumns = [
    showSidebar ? "clamp(330px, 25vw, 460px)" : null,
    showMedia ? "minmax(540px, 1fr)" : null,
    showProperties ? "clamp(250px, 17vw, 330px)" : null,
  ]
    .filter(Boolean)
    .join(" ");
  const sidebarTitle =
    sidebarTab === "chat"
      ? "Agent"
      : sidebarTab === "transcript"
        ? "Transcript"
        : "Edit history";

  return (
    <main className="app-shell">
      <header className="app-header">
        <div className="app-brand">
          <h1>Awidat</h1>
          <span>Video agent workspace</span>
        </div>
        <ProjectBanner onChange={setCurrent} />
      </header>
      {projectReady && <ActionBar />}
      {projectReady ? (
        <div className="workspace">
          <div
            className="workspace-top"
            style={{
              gridTemplateColumns: workspaceColumns || "minmax(0, 1fr)",
            }}
          >
            {showSidebar && (
            <div className="workspace-chat">
              <header className="sidebar-header">
                <div>
                  <span className="sidebar-kicker">Workspace</span>
                  <strong>{sidebarTitle}</strong>
                </div>
                <span className="sidebar-chip">
                  {sidebarTab === "chat"
                    ? "Prompt"
                    : sidebarTab === "transcript"
                      ? "Words"
                      : "Cuts"}
                </span>
              </header>
              <div className="sidebar-tabs" role="tablist" aria-label="Workspace sidebar">
                <button
                  type="button"
                  role="tab"
                  aria-selected={sidebarTab === "chat"}
                  className={sidebarTab === "chat" ? "sidebar-tab-active" : ""}
                  onClick={() => setSidebarTab("chat")}
                >
                  Chat
                </button>
                <button
                  type="button"
                  role="tab"
                  aria-selected={sidebarTab === "transcript"}
                  className={sidebarTab === "transcript" ? "sidebar-tab-active" : ""}
                  onClick={() => setSidebarTab("transcript")}
                >
                  Transcript
                </button>
                <button
                  type="button"
                  role="tab"
                  aria-selected={sidebarTab === "edits"}
                  className={sidebarTab === "edits" ? "sidebar-tab-active" : ""}
                  onClick={() => setSidebarTab("edits")}
                >
                  Edits
                </button>
              </div>
              {sidebarTab === "chat" ? (
                <>
                  <SessionBar />
                  <ChatStream />
                  {showNotes && <NotesPanel />}
                  <Composer projectReady={projectReady} />
                </>
              ) : sidebarTab === "transcript" ? (
                <TranscriptSidebar />
              ) : (
                <VeditPanel />
              )}
            </div>
            )}
            {showMedia && (
            <div className="workspace-media">
              <MediaPane />
            </div>
            )}
            {showProperties && (
            <div className="workspace-properties">
              <PropertiesPane />
            </div>
            )}
          </div>
          {showTimeline && <TimelinePane />}
        </div>
      ) : (
        <ChatStream />
      )}
      {!projectReady && <Composer projectReady={projectReady} />}
    </main>
  );
}

export default App;
