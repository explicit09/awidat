// App shell.
//
// Layout when a project is loaded: header → action bar → split workspace
// (chat on the left, media pane on the right) → composer pinned to the
// bottom. Layout when no project is loaded: header → ChatStream's
// "open or create a project" placeholder → disabled composer.

import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
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
import { PropertiesPane } from "./properties/PropertiesPane";
import { useTimelineSelectionStore } from "./properties/store";
import { NotesPanel } from "./notes/NotesPanel";
import { useNotesStore } from "./notes/store";
import { TranscriptSidebar } from "./transcript/TranscriptSidebar";
import { VeditPanel } from "./vedit/VeditPanel";
import {
  ITEM_EVENT,
  TURN_END_EVENT,
  type ItemEvent,
  type TurnEndEvent,
} from "./protocol";
import "./App.css";

function App() {
  const [sidebarTab, setSidebarTab] =
    useState<"chat" | "transcript" | "edits">("chat");
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
  const clearSelection = useTimelineSelectionStore((s) => s.clear);
  const clearNotes = useNotesStore((s) => s.clear);
  const ingestNote = useNotesStore((s) => s.ingest);

  // Keep the banner's local-state callback wired to the store so any
  // path change (open / new / future close) propagates to Composer's
  // disabled flag and any future panes that key off `current`.
  useEffect(() => {
    refresh().catch(() => {});
  }, [refresh]);

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
    return () => {
      itemsUnlisten.then((unlisten) => unlisten());
      endUnlisten.then((unlisten) => unlisten());
    };
  }, [
    upsertItem,
    setRunning,
    setTurnError,
    ingestProposal,
    ingestNote,
  ]);

  useEffect(() => {
    clearAgent();
    clearProposal();
    clearMediaSelection(null);
    clearSelection();
    clearNotes();
    if (current !== null) {
      refreshTimeline().catch(() => {});
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
          <div className="workspace-top">
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
                  <NotesPanel />
                  <Composer projectReady={projectReady} />
                </>
              ) : sidebarTab === "transcript" ? (
                <TranscriptSidebar />
              ) : (
                <VeditPanel />
              )}
            </div>
            <div className="workspace-media">
              <MediaPane />
            </div>
            <div className="workspace-properties">
              <PropertiesPane />
            </div>
          </div>
          <TimelinePane />
        </div>
      ) : (
        <ChatStream />
      )}
      {!projectReady && <Composer projectReady={projectReady} />}
    </main>
  );
}

export default App;
