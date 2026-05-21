import React from "react";
import ReactDOM from "react-dom/client";
import "./ui/tokens.css";
import { AppShell, CommandRail, LensNav, StageIndicator } from "./shell";
import { AgentStatusBadge, Inline, Pill } from "./ui";
import wordmark from "./brand/awidat-wordmark.svg";
import { useStageStore } from "./state";

function Root() {
  // Pre-warm two stages so the StageIndicator shows the "complete" treatment
  // for prior stages — purely a preview convenience.
  if (useStageStore.getState().visited.size === 1) {
    useStageStore.getState().set("indexing");
    useStageStore.getState().set("proposal");
  }
  return (
    <AppShell
      topChromeStart={
        <Inline gap="3" align="center">
          <img src={wordmark} alt="Awidat" className="h-7" />
        </Inline>
      }
      topChromeCenter={<StageIndicator />}
      topChromeEnd={
        <Inline gap="2" align="center">
          <Pill status="proposed">12 pending</Pill>
          <AgentStatusBadge status="awaiting-review" detail="12 pending changes" />
        </Inline>
      }
      lensRow={<LensNav />}
      commandRail={
        <CommandRail
          hasProject
          contextChips={[
            { label: "ep_01_full.mp4", kind: "media" },
            { label: "00:12:04 → 00:24:18", kind: "selection" },
          ]}
          plan={[
            { id: "p1", status: "complete", text: "Indexed transcript and waveform" },
            { id: "p2", status: "complete", text: "Found 18 candidate cuts" },
            { id: "p3", status: "in_progress", text: "Scoring cuts by intent + risk" },
            { id: "p4", status: "pending", text: "Surface top 12 as proposals" },
          ]}
          taskProgress={{ label: "Scoring cuts · 12 of 18", progress: 67 }}
          suggestions={[
            { id: "s1", label: "Show me why you made this cut.", prompt: "..." },
            { id: "s2", label: "Make this section slower.", prompt: "..." },
            { id: "s3", label: "Use fewer transitions.", prompt: "..." },
          ]}
          activity={[
            { id: "a1", timestamp: "00:42", text: "Read 8,432 transcript tokens." },
            { id: "a2", timestamp: "00:51", text: "Computed silence boundaries." },
            { id: "a3", timestamp: "01:03", text: "Scored 18 candidates against intent." },
          ]}
        />
      }
    />
  );
}

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <Root />
  </React.StrictMode>,
);
