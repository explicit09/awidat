// App shell.
//
// Layout for Step 1: a top banner showing the current project, the
// chat pane filling the middle, and a composer pinned to the bottom.
// Step 3 will introduce a side-by-side pane layout (chat | video);
// Step 4 adds a timeline strip below.

import { useState } from "react";
import { ProjectBanner } from "./app/ProjectBanner";
import { ChatStream } from "./agent/ChatStream";
import { Composer } from "./agent/Composer";
import "./App.css";

function App() {
  const [projectRoot, setProjectRoot] = useState<string | null>(null);
  const projectReady = projectRoot !== null;

  return (
    <main className="app-shell">
      <header className="app-header">
        <h1>Awidat</h1>
        <ProjectBanner onChange={setProjectRoot} />
      </header>
      <ChatStream />
      <Composer projectReady={projectReady} />
    </main>
  );
}

export default App;
