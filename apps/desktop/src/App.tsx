// App shell.
//
// Layout when a project is loaded: header → action bar → split workspace
// (chat on the left, media pane on the right) → composer pinned to the
// bottom. Layout when no project is loaded: header → ChatStream's
// "open or create a project" placeholder → disabled composer.

import { useEffect } from "react";
import { ProjectBanner } from "./app/ProjectBanner";
import { ActionBar } from "./app/ActionBar";
import { useProjectStore } from "./app/state";
import { useAgentStore } from "./agent/store";
import { ChatStream } from "./agent/ChatStream";
import { Composer } from "./agent/Composer";
import { MediaPane } from "./media/MediaPane";
import { useMediaStore } from "./media/store";
import { TimelinePane } from "./timeline/TimelinePane";
import { useProposalStore } from "./timeline/proposal";
import { useTimelineStore } from "./timeline/store";
import "./App.css";

function App() {
  const current = useProjectStore((s) => s.current);
  const setCurrent = useProjectStore((s) => s.setCurrent);
  const refresh = useProjectStore((s) => s.refresh);
  const clearAgent = useAgentStore((s) => s.clear);
  const clearProposal = useProposalStore((s) => s.clear);
  const clearMediaSelection = useMediaStore((s) => s.select);
  const refreshTimeline = useTimelineStore((s) => s.refresh);

  // Keep the banner's local-state callback wired to the store so any
  // path change (open / new / future close) propagates to Composer's
  // disabled flag and any future panes that key off `current`.
  useEffect(() => {
    refresh().catch(() => {});
  }, [refresh]);

  useEffect(() => {
    clearAgent();
    clearProposal();
    clearMediaSelection(null);
    if (current !== null) {
      refreshTimeline().catch(() => {});
    }
  }, [current, clearAgent, clearProposal, clearMediaSelection, refreshTimeline]);

  const projectReady = current !== null;

  return (
    <main className="app-shell">
      <header className="app-header">
        <h1>Awidat</h1>
        <ProjectBanner onChange={setCurrent} />
      </header>
      {projectReady && <ActionBar />}
      {projectReady ? (
        <div className="workspace">
          <div className="workspace-top">
            <div className="workspace-chat">
              <ChatStream />
            </div>
            <div className="workspace-media">
              <MediaPane />
            </div>
          </div>
          <TimelinePane />
        </div>
      ) : (
        <ChatStream />
      )}
      <Composer projectReady={projectReady} />
    </main>
  );
}

export default App;
