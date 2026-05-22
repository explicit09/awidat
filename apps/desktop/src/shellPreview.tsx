import React from "react";
import ReactDOM from "react-dom/client";
import "./ui/tokens.css";
import { useState } from "react";
import {
  AppShell,
  CommandRail,
  PreviewSurface,
  ProposalInspector,
  StageIndicator,
  TimelineHybrid,
} from "./shell";
import type {
  AgentEdit,
  DiffChip,
  PreviewChange,
  PreviewViewMode,
  ProposalInspectorData,
  TimelineTab,
  TimelineViewMode,
  TranscriptCell,
} from "./shell";
import { AgentStatusBadge, Inline, Pill } from "./ui";
import wordmark from "./brand/awidat-wordmark.svg";
import { useStageStore } from "./state";

const MOCK_CHANGES: PreviewChange[] = [
  { id: "c1", index: 1, kind: "accepted", timeS: 24 },
  { id: "c2", index: 2, kind: "accepted", timeS: 52 },
  { id: "c3", index: 3, kind: "pending", timeS: 88 },
  { id: "c4", index: 4, kind: "warning", timeS: 124 },
  { id: "c5", index: 5, kind: "pending", timeS: 158 },
  { id: "c6", index: 6, kind: "rejected", timeS: 198 },
  { id: "c7", index: 7, kind: "pending", timeS: 242, label: "Cut 07 · J-cut" },
  { id: "c8", index: 8, kind: "accepted", timeS: 296 },
  { id: "c9", index: 9, kind: "warning", timeS: 342 },
  { id: "c10", index: 10, kind: "pending", timeS: 384 },
  { id: "c11", index: 11, kind: "accepted", timeS: 422 },
  { id: "c12", index: 12, kind: "pending", timeS: 462 },
];

const MOCK_AGENT_EDITS: AgentEdit[] = [
  { id: "e1", startS: 12, endS: 28, status: "accepted" },
  { id: "e2", startS: 44, endS: 60, status: "accepted" },
  { id: "e3", startS: 80, endS: 96, status: "pending" },
  { id: "e4", startS: 116, endS: 132, status: "warning" },
  { id: "e5", startS: 150, endS: 166, status: "pending" },
  { id: "e6", startS: 190, endS: 206, status: "rejected" },
  { id: "e7", startS: 234, endS: 250, status: "pending" },
  { id: "e8", startS: 288, endS: 304, status: "accepted" },
  { id: "e9", startS: 334, endS: 350, status: "warning" },
  { id: "e10", startS: 376, endS: 392, status: "pending" },
  { id: "e11", startS: 414, endS: 430, status: "accepted" },
  { id: "e12", startS: 454, endS: 470, status: "pending" },
];

const MOCK_TRANSCRIPT: TranscriptCell[] = [
  { id: "t1", startS: 0, endS: 60, text: "Welcome back to the show. Today we're talking about…", speakerColor: "var(--color-viz-speaker-a)" },
  { id: "t2", startS: 60, endS: 130, text: "Yeah, and what's interesting is the way these systems compose…", speakerColor: "var(--color-viz-speaker-b)" },
  { id: "t3", startS: 130, endS: 200, text: "Exactly — and that's the part people miss when they first try it.", speakerColor: "var(--color-viz-speaker-a)" },
  { id: "t4", startS: 200, endS: 270, text: "Right, so the way we approached it…", speakerColor: "var(--color-viz-speaker-b)" },
  { id: "t5", startS: 270, endS: 340, text: "Build, ship, repeat — that's the loop we kept hitting.", speakerColor: "var(--color-viz-speaker-a)" },
  { id: "t6", startS: 340, endS: 410, text: "Yeah and to your earlier point about feedback loops…", speakerColor: "var(--color-viz-speaker-b)" },
  { id: "t7", startS: 410, endS: 492, text: "Anyway, we'll wrap there. Thanks for tuning in.", speakerColor: "var(--color-viz-speaker-a)" },
];

const MOCK_INSPECTOR: ProposalInspectorData = {
  title: "Cut 07 · J-cut",
  status: "pending",
  timeRange: "00:06:35:21 - 00:06:42:05",
  duration: "6.7s",
  cutType: "J-cut (audio leads)",
  intent:
    "Remove filler phrase and trailing pause while preserving speaker cadence. Use J-cut to lead into the next speaker for smoother handoff.",
  explanation:
    "Detected 1.8s of low-energy filler (\"um, so basically\") followed by a 0.9s pause. Removing this segment maintains the flow and keeps the answer concise while the next speaker starts naturally.",
  confidence: 0.88,
  risk: "low",
  evidence: [
    { id: "ev1", kind: "transcript", label: "Transcript boundary", confidence: 0.91 },
    { id: "ev2", kind: "audio_energy", label: "Audio energy drop", confidence: 0.84 },
    { id: "ev3", kind: "speaker_handoff", label: "Speaker handoff", confidence: 0.87 },
    { id: "ev4", kind: "visual", label: "Visual continuity", confidence: 0.67 },
  ],
  alternatives: [
    { id: "alt1", label: "Keep more context", detail: "+0.9s" },
    { id: "alt2", label: "Tighter cut", detail: "-1.2s" },
    { id: "alt3", label: "Hard cut (no J-cut)", detail: "—" },
  ],
  alternativesCountLabel: "View all (3)",
};

const MOCK_DIFF: DiffChip[] = [
  { id: "d1", startS: 0, endS: 60, kind: "keep" },
  { id: "d2", startS: 60, endS: 70, kind: "trim", label: "Trim 0.7s" },
  { id: "d3", startS: 70, endS: 130, kind: "keep" },
  { id: "d4", startS: 130, endS: 142, kind: "remove", label: "Remove 1.3s" },
  { id: "d5", startS: 142, endS: 210, kind: "keep" },
  { id: "d6", startS: 210, endS: 222, kind: "remove", label: "Remove 2.1s" },
  { id: "d7", startS: 222, endS: 296, kind: "keep" },
  { id: "d8", startS: 296, endS: 308, kind: "trim", label: "Trim 0.6s" },
  { id: "d9", startS: 308, endS: 380, kind: "keep" },
  { id: "d10", startS: 380, endS: 388, kind: "trim", label: "Trim 0.4s" },
  { id: "d11", startS: 388, endS: 492, kind: "keep" },
];

function Root() {
  // Pre-warm two stages so the StageIndicator shows the "complete" treatment
  // for prior stages — purely a preview convenience.
  if (useStageStore.getState().visited.size === 1) {
    useStageStore.getState().set("indexing");
    useStageStore.getState().set("edit");
  }
  const [activeChangeId, setActiveChangeId] = useState<string | undefined>("c7");
  const [currentTime, setCurrentTime] = useState(242);
  const [isPlaying, setIsPlaying] = useState(false);
  const [viewMode, setViewMode] = useState<PreviewViewMode>("before-after");
  const [volume, setVolume] = useState(0.7);
  const [rate, setRate] = useState(1);
  const [timelineTab, setTimelineTab] = useState<TimelineTab>("timeline");
  const [timelineViewMode, setTimelineViewMode] = useState<TimelineViewMode>("proposed");

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
      inspector={<ProposalInspector data={MOCK_INSPECTOR} />}
      timeline={
        <TimelineHybrid
          tab={timelineTab}
          onChangeTab={setTimelineTab}
          viewMode={timelineViewMode}
          onChangeViewMode={setTimelineViewMode}
          durationS={492}
          agentEdits={MOCK_AGENT_EDITS}
          transcript={MOCK_TRANSCRIPT}
          diff={MOCK_DIFF}
          currentTimeS={currentTime}
          changeCount={12}
        />
      }
      preview={
        <PreviewSurface
          proposalName="Podcast Tightening v1"
          pendingCount={12}
          changes={MOCK_CHANGES}
          activeChangeId={activeChangeId}
          currentTimeS={currentTime}
          durationS={492}
          isPlaying={isPlaying}
          volume={volume}
          rate={rate}
          viewMode={viewMode}
          onSelectChange={(c) => {
            setActiveChangeId(c.id);
            setCurrentTime(c.timeS);
          }}
          onPlayPause={() => setIsPlaying((p) => !p)}
          onSeek={setCurrentTime}
          onSetVolume={setVolume}
          onSetRate={setRate}
          onSetViewMode={setViewMode}
        />
      }
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
