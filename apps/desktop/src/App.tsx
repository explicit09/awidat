// App shell.
//
// Layout: top banner (project picker), chat pane filling the middle,
// composer pinned to the bottom. Step 3 will introduce a side-by-side
// pane layout (chat | video); Step 4 adds a timeline strip below.

import { useEffect } from "react";
import { ProjectBanner } from "./app/ProjectBanner";
import { ActionBar } from "./app/ActionBar";
import { useProjectStore } from "./app/state";
import { ChatStream } from "./agent/ChatStream";
import { Composer } from "./agent/Composer";
import "./App.css";

function App() {
  const current = useProjectStore((s) => s.current);
  const setCurrent = useProjectStore((s) => s.setCurrent);
  const refresh = useProjectStore((s) => s.refresh);

  // Keep the banner's local-state callback wired to the store so any
  // path change (open / new / future close) propagates to Composer's
  // disabled flag and any future panes that key off `current`.
  useEffect(() => {
    refresh().catch(() => {});
  }, [refresh]);

  const projectReady = current !== null;

  return (
    <main className="app-shell">
      <header className="app-header">
        <h1>Awidat</h1>
        <ProjectBanner onChange={setCurrent} />
      </header>
      {projectReady && <ActionBar />}
      <ChatStream />
      <Composer projectReady={projectReady} />
    </main>
  );
}

export default App;
