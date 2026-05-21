/**
 * Awidat v2 App — composes the AppShell from src/shell with real wiring.
 *
 * The legacy 3-column layout has been retired (history: see git on
 * `ui-v2`). Side effects that the legacy App.tsx owned (Tauri channel
 * subscribers, menu-command routing, project lifecycle) live in
 * `src/state/appGlue.ts`. Everything below this point is pure
 * composition + glue between Zustand stores and shell components.
 */

import { invoke, isTauri } from "@tauri-apps/api/core";
import { ChevronDown, Settings as SettingsIcon, Share2 } from "lucide-react";
import { useMemo } from "react";
import wordmark from "./brand/awidat-wordmark.svg";
import { useAgentStore } from "./agent/store";
import { useProjectStore } from "./app/state";
import { useMediaStore } from "./media/store";
import {
  AppShell,
  CommandRail,
  LensNav,
  PreviewSurface,
  ProposalInspector,
  StageIndicator,
  StageStub,
  TimelineHybrid,
  type ActivityEntry,
  type PlanItem,
  type PreviewChange,
  type TimelineTab,
  type TimelineViewMode,
} from "./shell";
import { AgentStatusBadge, IconButton, Inline, Pill } from "./ui";
import { useStageStore } from "./state";
import { useAppGlue } from "./state/appGlue";
import { useProposalInspectorData } from "./state/proposalAdapter";
import { useTimelineStore } from "./timeline/store";
import { useProposalStore } from "./timeline/proposal";
import { MENU_COMMANDS, emitMenuCommand } from "./app/menuCommands";
import "./ui/tokens.css";

import { useState } from "react";

function App() {
  // Side effects (Tauri channels, menu routing, project lifecycle).
  useAppGlue();

  const current = useProjectStore((s) => s.current);
  const items = useAgentStore((s) => s.items);
  const running = useAgentStore((s) => s.running);
  const stage = useStageStore((s) => s.current);
  const setStage = useStageStore((s) => s.set);

  const timelineDuration = useTimelineStore((s) => s.snapshot.duration_s);
  const currentTimeS = useMediaStore((s) => s.timelineTime);
  const isPlaying = useMediaStore((s) => s.isPlaying);

  const activeProposal = useProposalStore((s) => s.active);
  const inspectorData = useProposalInspectorData();

  const [timelineTab, setTimelineTab] = useState<TimelineTab>("timeline");
  const [timelineViewMode, setTimelineViewMode] = useState<TimelineViewMode>("proposed");

  const hasProject = current !== null;

  // Distill agent items into the CommandRail's plan + activity lists.
  // Real agent producers populate `Plan` items, `ToolCall` items, etc.
  // We translate them into shell-friendly shapes here.
  const plan: PlanItem[] = useMemo(() => {
    const planItems = items.filter(
      (it): it is Extract<typeof items[number], { kind: "plan" }> => it.kind === "plan",
    );
    if (planItems.length === 0) return [];
    // Use the latest Plan item — the agent emits incremental updates.
    const latest = planItems[planItems.length - 1];
    return latest.items.map((step, i) => ({
      id: `${latest.id.toString()}-${i}`,
      text: step.step,
      status: stepStatus(step.status),
    }));
  }, [items]);

  const activity: ActivityEntry[] = useMemo(() => {
    return items
      .filter(
        (it): it is ActivitySourceItem =>
          it.kind === "tool_call" || it.kind === "text" || it.kind === "job",
      )
      .slice(-12)
      .map((it) => activityFor(it));
  }, [items]);

  const previewChanges: PreviewChange[] = useMemo(() => {
    if (!activeProposal) return [];
    // Until backend ships per-cut surfaces, render one chip per diff hint.
    return activeProposal.diffHints.map((hint, i) => ({
      id: `${activeProposal.callId}-${i}`,
      index: i + 1,
      kind: "pending" as const,
      timeS: estimateTimeForHint(hint, activeProposal.snapshot.duration_s, i, activeProposal.diffHints.length),
    }));
  }, [activeProposal]);

  // The PROPOSAL stage is the working surface. All other stages render
  // the StageStub until Phase 3/4 builds them.
  const isProposalStage = stage === "proposal";

  return (
    <AppShell
      topChromeStart={
        <Inline gap="3" align="center">
          <img src={wordmark} alt="Awidat" className="h-7" />
          {current ? (
            <button
              type="button"
              className="inline-flex items-center gap-1 h-7 px-2 rounded-[var(--radius-sm)] hover:bg-[var(--color-surface-hover)] transition-colors"
            >
              <span className="text-[var(--text-caption)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-muted)]">
                Project
              </span>
              <span className="text-[var(--text-body-sm)] text-[var(--color-text-primary)] truncate max-w-[200px]">
                {projectName(current)}
              </span>
              <ChevronDown className="h-3 w-3 stroke-[1.75] text-[var(--color-text-muted)]" />
            </button>
          ) : null}
        </Inline>
      }
      topChromeCenter={<StageIndicator />}
      topChromeEnd={
        <Inline gap="2" align="center">
          {activeProposal ? (
            <Pill status="warning">{previewChanges.length} pending</Pill>
          ) : null}
          <AgentStatusBadge
            status={
              running
                ? "analyzing"
                : activeProposal
                  ? "awaiting-review"
                  : current
                    ? "online"
                    : "idle"
            }
            detail={running ? "Working" : activeProposal ? "Review pending" : current ? "Ready" : "No project"}
          />
          <IconButton icon={<Share2 />} label="Share" size="md" />
          <IconButton icon={<SettingsIcon />} label="Settings" size="md" />
        </Inline>
      }
      lensRow={isProposalStage ? <LensNav /> : <span />}
      commandRail={
        <CommandRail
          hasProject={hasProject}
          running={running}
          plan={plan}
          activity={activity}
          onSubmit={(command) => {
            if (!isTauri()) return;
            invoke("start_turn", { command }).catch((e) =>
              console.warn("start_turn failed", e),
            );
          }}
          onCancel={() => {
            if (!isTauri()) return;
            invoke("cancel_turn").catch((e) =>
              console.warn("cancel_turn failed", e),
            );
          }}
        />
      }
      preview={
        isProposalStage ? (
          <PreviewSurface
            proposalName={activeProposal?.summary ?? "No proposal"}
            pendingCount={previewChanges.length}
            changes={previewChanges}
            currentTimeS={currentTimeS}
            durationS={timelineDuration}
            isPlaying={isPlaying}
          />
        ) : (
          <StageStub stage={stage} onPrimaryAction={() => setStage("proposal")} />
        )
      }
      timeline={
        isProposalStage ? (
          <TimelineHybrid
            tab={timelineTab}
            onChangeTab={setTimelineTab}
            viewMode={timelineViewMode}
            onChangeViewMode={setTimelineViewMode}
            durationS={timelineDuration}
            currentTimeS={currentTimeS}
            changeCount={previewChanges.length}
          />
        ) : (
          <span />
        )
      }
      inspector={
        isProposalStage ? (
          <ProposalInspector
            data={inspectorData}
            onAccept={() => {
              if (!isTauri() || !activeProposal) return;
              invoke("accept_proposal", { callId: activeProposal.callId }).catch((e) =>
                console.warn("accept_proposal failed", e),
              );
            }}
            onReject={() => {
              if (!isTauri() || !activeProposal) return;
              invoke("reject_proposal", { callId: activeProposal.callId }).catch((e) =>
                console.warn("reject_proposal failed", e),
              );
            }}
            onInspectDeeper={() => emitMenuCommand(MENU_COMMANDS.ACCEPT_PROPOSAL)}
          />
        ) : (
          <span />
        )
      }
      footer={<Footer />}
    />
  );
}

function Footer() {
  const running = useAgentStore((s) => s.running);
  return (
    <>
      <Inline gap="3" align="center">
        <span
          className="h-2 w-2 rounded-full"
          style={{ backgroundColor: running ? "var(--color-warning)" : "var(--color-success)" }}
          aria-hidden
        />
        <span className="text-[var(--text-caption)] text-[var(--color-text-secondary)] font-mono">
          {running ? "Agent · working" : "Agent · online"}
        </span>
        <span className="text-[var(--text-caption)] text-[var(--color-text-muted)] font-mono">
          local · disk OK
        </span>
      </Inline>
      <Inline gap="3" align="center">
        <span className="text-[var(--text-caption)] text-[var(--color-text-muted)] font-mono">
          Awidat v2 · ui-v2
        </span>
      </Inline>
    </>
  );
}

function projectName(path: string): string {
  const parts = path.split("/").filter(Boolean);
  return parts[parts.length - 1] ?? path;
}

type AnyAgentItem = ReturnType<typeof useAgentStore.getState>["items"][number];
type ToolCallItem = Extract<AnyAgentItem, { kind: "tool_call" }>;
type TextItem = Extract<AnyAgentItem, { kind: "text" }>;
type JobItem = Extract<AnyAgentItem, { kind: "job" }>;
type ActivitySourceItem = ToolCallItem | TextItem | JobItem;

function activityFor(item: ActivitySourceItem): ActivityEntry {
  const id = item.id.toString();
  if (item.kind === "tool_call") {
    return {
      id,
      timestamp: shortTime(),
      text: `${item.name} ${item.phase === "completed" ? "complete" : "running"}`,
      kind: "tool",
    };
  }
  if (item.kind === "text") {
    return {
      id,
      timestamp: shortTime(),
      text: item.text.slice(0, 80),
      kind: "thought",
    };
  }
  return {
    id,
    timestamp: shortTime(),
    text: `${item.job_kind} · ${item.status}`,
    kind: "result",
  };
}

function shortTime(): string {
  const d = new Date();
  return `${d.getHours().toString().padStart(2, "0")}:${d.getMinutes().toString().padStart(2, "0")}`;
}

function stepStatus(s: string): PlanItem["status"] {
  if (s === "complete" || s === "done") return "complete";
  if (s === "in_progress" || s === "running") return "in_progress";
  if (s === "failed" || s === "error") return "failed";
  return "pending";
}

// Estimate a time-on-the-timeline for a diff hint when we don't have one in the
// protocol yet. Distributes hints evenly across the timeline duration so the
// jump chips and scrubber dots aren't all stacked at zero.
function estimateTimeForHint(_hint: unknown, durationS: number, i: number, total: number): number {
  if (!durationS || total === 0) return 0;
  return ((i + 1) / (total + 1)) * durationS;
}

export default App;
