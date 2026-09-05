import { type PointerEvent as ReactPointerEvent, type ReactNode, useEffect, useRef, useState } from "react";
import { useBriefProposalsStore, type BriefMedium } from "../state/briefProposals";
import { usePendingProposals } from "../timeline/pendingProposals";
import { ProposalCard } from "./brief/ProposalCard";
import { useTimelineStore } from "../timeline/store";
import { useTimelineSelectionStore } from "../properties/store";
import type { Stage } from "../state/stages";
import { useSettings } from "../state/settings";
import { FolderOpen, Settings as SettingsIcon } from "lucide-react";
import { BrandMark } from "../brand/BrandMark";
import { ConversationPanel } from "./StageConversation";
import type { AgentProfile, PermissionMode } from "../protocol";
import type { ChatSessionSummary, MediaSuggestion } from "./CommandRail";
import { MENU_COMMANDS, onMenuCommand } from "../app/menuCommands";

/**
 * StageShell — the 2026 "Stage" application shell (replaces the
 * three-rail AppShell cockpit).
 *
 *   • The footage preview is the hero, centered + large.
 *   • The agent's proposals ride alongside as a swipeable glass deck,
 *     wired to the real useBriefProposalsStore (accept / reject).
 *   • A persistent command bar drives edits AND navigation.
 *   • Source tools live in a left pane; chat and output tools live in a
 *     right pane. Larger destinations such as Schedule and Skills stay as
 *     full-page surfaces.
 *
 * This component is pure layout: every real surface (preview, timeline,
 * delivery/skills/history) is passed in as a node by App.tsx, which owns
 * the data wiring. The proposal deck reads the store directly.
 */

const BRAND_RED = "#EF4444";

const MEDIUM_COLOR: Record<string, string> = {
  cut: "#EF4444",
  color: "#FCD34D",
  broll: "#FB7185",
  audio: "#CBD5E1",
  transition: "#FB7185",
  title: "#FCA5A5",
  caption: "#FCA5A5",
  mixed: "#E2E8F0",
  other: "#94A3B8",
};
function mediumColor(m: BriefMedium): string {
  return MEDIUM_COLOR[m] ?? "#94A3B8";
}

// Side-pane tools: source/context tools on the left, output/session tools on the right.
type ToolKey = "transcript" | "media" | "inspector" | "index" | "vedit" | "notes";

// Timeline strip sizing — fits the common V1 + A1 layout without vertical scroll.
const TL_HEADER_PX = 40;
const TL_RULER_PX = 22;
const TL_ROW = 62;
const TL_MIN_VISIBLE_TRACKS = 2;
const TL_MIN_PX = TL_HEADER_PX + TL_RULER_PX + TL_MIN_VISIBLE_TRACKS * TL_ROW;
const TL_MAX_VH = 0.58; // soft cap; beyond this the strip scrolls (never clips)

const SIDE_PANE_W = 360;
const SIDE_PANE_MIN_W = 260;
const SIDE_PANE_MAX_W = 520;
const SIDE_PANE_GUTTER = 12;
type LeftPaneKey = "transcript" | "media" | "index";
type RightPaneKey = "chat" | "deliver" | "inspector" | "vedit" | "notes";
const LEFT_PANES: { id: LeftPaneKey; label: string }[] = [
  { id: "media", label: "Media" },
  { id: "transcript", label: "Transcript" },
  { id: "index", label: "Index" },
];
const RIGHT_PANES: { id: RightPaneKey; label: string }[] = [
  { id: "chat", label: "Chat" },
  { id: "deliver", label: "Deliver" },
  { id: "inspector", label: "Inspector" },
  { id: "vedit", label: "Vedit" },
  { id: "notes", label: "Notes" },
];
type PaneSide = "left" | "right";
type PaneResize = { side: PaneSide; startX: number; startWidth: number };
type TimelineResize = { startY: number; startHeight: number };

export type StageShellProps = {
  hasProject: boolean;
  landing: ReactNode;
  /** The video preview hero (PreviewSurface), already composed by App. */
  preview: ReactNode;
  /** Real timeline node (TimelineHybrid + TimelinePane). */
  timeline: ReactNode;
  /** Track count drives the timeline strip height (fit all, no scroll). */
  trackCount?: number;
  /** Summonable cockpit tools, opened in the stage side panes. */
  tools?: {
    media: ReactNode;
    inspector: ReactNode;
    index: ReactNode;
    transcript: ReactNode;
    vedit: ReactNode;
    notes: ReactNode;
  };
  /** Rising edge auto-opens the Inspector tool (proposal/clip selected). */
  autoInspect?: boolean;
  deliver: ReactNode;
  schedule: ReactNode;
  skills: ReactNode;
  history: ReactNode;
  stage: Stage;
  onStage: (s: Stage) => void;
  /** Fire an agent turn from the command bar. */
  onCommand: (text: string) => void;
  running: boolean;
  onCancel: () => void;
  mediaSuggestions?: MediaSuggestion[];
  onPickMedia?: (suggestion: MediaSuggestion) => void;
  chatSessions?: ChatSessionSummary[];
  activeChatSession?: ChatSessionSummary | null;
  chatLoading?: boolean;
  onOpenHistory?: () => void;
  onSelectChatSession?: (session: ChatSessionSummary) => void;
  onNewChat?: () => void;
  permissionMode?: PermissionMode;
  onSetPermissionMode?: (mode: PermissionMode) => void;
  agentProfile?: AgentProfile;
  onSetAgentProfile?: (profile: AgentProfile) => void;
  /** Floating-chrome bits. */
  projectLabel?: string;
  projectType?: string;
  timecode?: string;
  /** Agent "read" line — what the agent knows right now. */
  agentRead?: string;
  footer?: ReactNode;
};

export function StageShell(props: StageShellProps) {
  const {
    hasProject, landing, preview, timeline, trackCount = 0, tools, autoInspect,
    deliver, schedule, skills, history,
    stage, onStage, onCommand, running, onCancel, mediaSuggestions = [], onPickMedia,
    chatSessions = [], activeChatSession = null, chatLoading = false,
    onOpenHistory, onSelectChatSession, onNewChat,
    permissionMode = "manual", onSetPermissionMode,
    agentProfile = "balanced", onSetAgentProfile,
    projectLabel, projectType, timecode, agentRead,
  } = props;

  // pending() merges three reactive sources — approvals + broll (both on
  // the brief store) and proposed_edits (on usePendingProposals). Subscribe
  // to ALL of them so the deck re-renders when any source changes; reading
  // only s.approvals (the prior bug) missed proposed_edit + b-roll arrivals.
  useBriefProposalsStore((s) => s.approvals);
  useBriefProposalsStore((s) => s.brollProposals);
  usePendingProposals((s) => s.pending);
  const pending = useBriefProposalsStore.getState().pending();

  const [active, setActive] = useState(0);
  const [draft, setDraft] = useState("");

  const onStage_ = stage === "edit";
  const [visibleDestination, setVisibleDestination] = useState<Stage | null>(
    onStage_ ? null : stage,
  );
  useEffect(() => {
    if (!onStage_) {
      setVisibleDestination(stage);
      return undefined;
    }
    let secondFrame: number | null = null;
    const firstFrame = window.requestAnimationFrame(() => {
      secondFrame = window.requestAnimationFrame(() => setVisibleDestination(null));
    });
    return () => {
      window.cancelAnimationFrame(firstFrame);
      if (secondFrame !== null) window.cancelAnimationFrame(secondFrame);
    };
  }, [onStage_, stage]);

  const cur = pending[Math.min(active, Math.max(0, pending.length - 1))];

  // Stable editor panes: source/navigation tools on the left, chat/output
  // tools on the right.
  const devTool = (import.meta.env?.VITE_MONTAGE_TOOL as ToolKey | undefined) ?? null;
  const [leftPane, setLeftPane] = useState<LeftPaneKey>(
    devTool === "media" || devTool === "index" ? devTool : "transcript",
  );
  const [rightPane, setRightPane] = useState<RightPaneKey>(
    devTool === "inspector" || devTool === "vedit" ? devTool : "chat",
  );
  const [leftPaneWidth, setLeftPaneWidth] = useState(SIDE_PANE_W);
  const [rightPaneWidth, setRightPaneWidth] = useState(SIDE_PANE_W);
  const [timelineHeightPx, setTimelineHeightPx] = useState(TL_MIN_PX);
  const [timelineHeightManual, setTimelineHeightManual] = useState(false);
  const paneResize = useRef<PaneResize | null>(null);
  const timelineResize = useRef<TimelineResize | null>(null);
  const selectedClipKey = useTimelineSelectionStore((s) => s.selectedClipKey);
  useEffect(() => {
    if (running) setRightPane("chat");
  }, [running]);

  useEffect(() => {
    if (selectedClipKey && onStage_) setRightPane("inspector");
  }, [onStage_, selectedClipKey]);

  // Auto-open the Inspector on the rising edge of a selection — never fight
  // the user (don't reopen if they've closed it while the selection holds).
  const prevAutoInspect = useRef(false);
  useEffect(() => {
    if (autoInspect && !prevAutoInspect.current) setRightPane("inspector");
    prevAutoInspect.current = !!autoInspect;
  }, [autoInspect]);

  useEffect(
    () => onMenuCommand((id) => {
      if (id === MENU_COMMANDS.VIEW_NOTES) {
        onStage("edit");
        setRightPane("notes");
      }
    }),
    [onStage],
  );

  // Track-sized timeline strip — fit every track, never scroll vertically.
  // Falls back to the live store count if App didn't pass one.
  const storeTrackCount = useTimelineStore((s) => s.snapshot.tracks.length);
  const tracks = trackCount || storeTrackCount;
  const autoTimelineHeightPx = clampTimelineHeight(
    TL_HEADER_PX + TL_RULER_PX + Math.max(TL_MIN_VISIBLE_TRACKS, tracks) * TL_ROW,
  );
  useEffect(() => {
    if (!timelineHeightManual) setTimelineHeightPx(autoTimelineHeightPx);
  }, [autoTimelineHeightPx, timelineHeightManual]);
  const timelineHeight = `${timelineHeightPx}px`;
  const LEFT_PANE_RESERVE = leftPaneWidth + SIDE_PANE_GUTTER + 12;
  const RIGHT_PANE_RESERVE = rightPaneWidth + SIDE_PANE_GUTTER + 12;
  const paneBottom = `calc(36px + ${timelineHeight})`;

  const beginPaneResize = (side: PaneSide, event: ReactPointerEvent<HTMLDivElement>) => {
    paneResize.current = {
      side,
      startX: event.clientX,
      startWidth: side === "left" ? leftPaneWidth : rightPaneWidth,
    };
    event.currentTarget.setPointerCapture(event.pointerId);
  };
  const movePaneResize = (event: ReactPointerEvent<HTMLDivElement>) => {
    const resize = paneResize.current;
    if (!resize) return;
    const delta = event.clientX - resize.startX;
    const nextWidth = resize.side === "left"
      ? resize.startWidth + delta
      : resize.startWidth - delta;
    const clamped = clamp(nextWidth, SIDE_PANE_MIN_W, SIDE_PANE_MAX_W);
    if (resize.side === "left") {
      setLeftPaneWidth(clamped);
    } else {
      setRightPaneWidth(clamped);
    }
  };
  const endPaneResize = (event: ReactPointerEvent<HTMLDivElement>) => {
    paneResize.current = null;
    if (event.currentTarget.hasPointerCapture(event.pointerId)) {
      event.currentTarget.releasePointerCapture(event.pointerId);
    }
  };
  const beginTimelineResize = (event: ReactPointerEvent<HTMLDivElement>) => {
    timelineResize.current = {
      startY: event.clientY,
      startHeight: timelineHeightPx,
    };
    setTimelineHeightManual(true);
    event.currentTarget.setPointerCapture(event.pointerId);
  };
  const moveTimelineResize = (event: ReactPointerEvent<HTMLDivElement>) => {
    const resize = timelineResize.current;
    if (!resize) return;
    setTimelineHeightPx(clampTimelineHeight(resize.startHeight - (event.clientY - resize.startY)));
  };
  const endTimelineResize = (event: ReactPointerEvent<HTMLDivElement>) => {
    timelineResize.current = null;
    if (event.currentTarget.hasPointerCapture(event.pointerId)) {
      event.currentTarget.releasePointerCapture(event.pointerId);
    }
  };

  // Empty stage: a project is loaded but there's NO footage on the timeline
  // yet (no tracks) AND nothing proposed. "No proposals" alone is not empty —
  // the user may already have footage they're working with.
  const stageEmpty = pending.length === 0 && tracks === 0;
  const leftNode = toolNode(leftPane, tools);
  const rightNode = rightPane === "chat" ? null : rightPaneNode(rightPane, {
    deliver, tools,
  });

  const submit = () => {
    const text = draft.trim();
    if (!text) return;
    // Command routes: bare destination words navigate; everything else
    // is an editorial instruction to the agent.
    const lower = text.toLowerCase().replace(/^\//, "");
    if (lower === "transcript" || lower === "media" || lower === "index") {
      onStage("edit");
      setLeftPane(lower as LeftPaneKey);
    } else if (lower === "inspector" || lower === "vedit" || lower === "notes") {
      onStage("edit");
      setRightPane(lower as RightPaneKey);
    } else if (lower === "deliver") {
      onStage("edit");
      setRightPane(lower as RightPaneKey);
    } else if (lower === "history" || lower === "schedule" || lower === "skills" || lower === "stage" || lower === "edit") {
      onStage(lower === "stage" ? "edit" : (lower as Stage));
    } else {
      onCommand(text);
      setRightPane("chat"); // show the thread so the reply is visible
    }
    setDraft("");
  };

  return (
    <>
    {!hasProject ? (
      <div className="relative z-10 h-full w-full">{landing}</div>
    ) : null}
    <div
      aria-hidden={!hasProject}
      className="relative z-10 h-full w-full overflow-hidden font-sans text-[var(--color-text-primary)]"
      style={{ display: hasProject ? undefined : "none" }}
    >
      {/* floating top chrome */}
      <div className="absolute inset-x-0 top-0 z-30 flex items-center gap-3 px-5 py-3" data-tauri-drag-region>
        <BrandMark size={26} className="rounded-[8px]" />
        <span className="font-mono text-[12px] tracking-[0.18em] text-[var(--color-text-secondary)]">MONTAGE</span>
        {projectLabel ? (
          <span className="glass-ghost rounded-lg px-2.5 py-1 font-mono text-[11px] text-[var(--color-text-muted)]">
            {projectLabel}{projectType ? <> · <span className="text-[var(--color-text-secondary)]">{projectType}</span></> : null}
          </span>
        ) : null}
        <div className="ml-auto flex items-center gap-3">
          <span className="flex items-center gap-1.5 text-[11px] text-[var(--color-text-muted)]">
            <span className="h-1.5 w-1.5 rounded-full" style={{ background: running ? BRAND_RED : "#20C997", boxShadow: `0 0 8px ${running ? BRAND_RED : "#20C997"}` }} />
            {running ? "working" : "ready"}
          </span>
          {timecode ? <span className="font-mono text-[11px] text-[var(--color-text-muted)]">{timecode}</span> : null}
          <button onClick={() => useSettings.getState().open()} className="gk-close" aria-label="Settings" title="Settings (⌘,)">
            <SettingsIcon className="h-3.5 w-3.5 stroke-[1.75]" />
          </button>
        </div>
      </div>

      {/* Left source pane — readable tabs for source/navigation tools. */}
      {onStage_ ? (
        <div
          className="stage-left-pane glass glass-strong z-30 flex flex-col overflow-hidden"
          style={{
            position: "absolute",
            top: 64,
            bottom: paneBottom,
            left: SIDE_PANE_GUTTER,
            right: "auto",
            width: leftPaneWidth,
            maxWidth: "calc(100vw - 24px)",
            borderRadius: 10,
          }}>
          <div className="stage-left-tabs flex items-center gap-1 overflow-x-auto border-b border-[var(--glass-border)] px-2 py-2">
            {LEFT_PANES.map((pane) => (
              <button
                key={pane.id}
                onClick={() => setLeftPane(pane.id)}
                className="stage-left-tab"
                data-active={leftPane === pane.id ? "true" : "false"}
              >
                {pane.label}
              </button>
            ))}
          </div>
          <div className="stage-tool-body flex min-h-0 flex-1 flex-col overflow-auto">
            {leftNode}
          </div>
          <div
            className="stage-pane-resize stage-pane-resize-left"
            onPointerDown={(event) => beginPaneResize("left", event)}
            onPointerMove={movePaneResize}
            onPointerUp={endPaneResize}
            onPointerCancel={endPaneResize}
            role="separator"
            aria-orientation="vertical"
          />
        </div>
      ) : null}

      {/* Right output pane — DaVinci-style readable tabs for chat and
          delivery/session output tools. */}
      {onStage_ ? (
        <div
          className="stage-right-pane glass glass-strong z-30 flex flex-col overflow-hidden"
          style={{
            position: "absolute",
            top: 64,
            bottom: paneBottom,
            right: SIDE_PANE_GUTTER,
            left: "auto",
            width: rightPaneWidth,
            maxWidth: "calc(100vw - 24px)",
            borderRadius: 10,
          }}>
          <div className="stage-right-tabs flex items-center gap-1 overflow-x-auto border-b border-[var(--glass-border)] px-2 py-2">
            {RIGHT_PANES.map((pane) => (
              <button
                key={pane.id}
                onClick={() => setRightPane(pane.id)}
                className="stage-right-tab"
                data-active={rightPane === pane.id ? "true" : "false"}
              >
                {pane.label}
              </button>
            ))}
          </div>
          <div className="stage-tool-body flex min-h-0 flex-1 flex-col overflow-auto">
            {rightPane === "chat" ? (
              <ConversationPanel
                agentRead={agentRead}
                draft={draft}
                running={running}
                onDraft={setDraft}
                onSubmit={submit}
                onCancel={onCancel}
                mediaSuggestions={mediaSuggestions}
                onPickMedia={onPickMedia}
                chatSessions={chatSessions}
                activeChatSession={activeChatSession}
                chatLoading={chatLoading}
                onOpenHistory={onOpenHistory}
                onSelectChatSession={onSelectChatSession}
                onNewChat={onNewChat}
                permissionMode={permissionMode}
                onSetPermissionMode={onSetPermissionMode}
                agentProfile={agentProfile}
                onSetAgentProfile={onSetAgentProfile}
              />
            ) : rightNode}
          </div>
          <div
            className="stage-pane-resize stage-pane-resize-right"
            onPointerDown={(event) => beginPaneResize("right", event)}
            onPointerMove={movePaneResize}
            onPointerUp={endPaneResize}
            onPointerCancel={endPaneResize}
            role="separator"
            aria-orientation="vertical"
          />
        </div>
      ) : null}

      {/* STAGE LAYER — preview hero + proposal deck. Bottom padding tracks the
          timeline height; side padding makes room for open side panes. */}
      <div className="absolute inset-0 z-10 flex items-stretch justify-center gap-4 px-10 pt-12"
        style={{
          filter: onStage_ ? "none" : "brightness(0.58)",
          pointerEvents: onStage_ ? "auto" : "none",
          // Same bottom line as the side panes (paneBottom) so the
          // center panels and the side rails end flush.
          paddingBottom: paneBottom,
          paddingLeft: LEFT_PANE_RESERVE,
          paddingRight: RIGHT_PANE_RESERVE,
        }}>
        <div className="stage-hero-col relative flex min-w-0 flex-1 flex-col gap-2">
          <div className="stage-hero-card glass relative min-h-0 overflow-hidden" style={{ borderRadius: 18 }}>
            {stageEmpty ? <div className="absolute inset-0 bg-black/55" /> : preview}
            {/* purposeful empty state — overlays the black hero when there's
                nothing to review yet (no pending proposals). */}
            {stageEmpty ? (
              <div className="absolute inset-0 z-10 grid place-items-center p-8 text-center">
                <div className="flex min-h-[132px] w-full max-w-[320px] flex-col items-center justify-center gap-4">
                  <div className="text-[21px] font-bold tracking-tight text-[var(--color-text-primary)]">No timeline media</div>
                  <button
                    onClick={() => { onStage("edit"); setLeftPane("media"); }}
                    className="glass-cta inline-flex h-10 items-center gap-2 rounded-xl px-4 text-[13px] font-semibold"
                  >
                    <FolderOpen size={15} aria-hidden="true" />
                    Open media
                  </button>
                </div>
              </div>
            ) : null}
          </div>
        </div>

        {pending.length > 0 && cur ? (
          <div className="flex w-[340px] shrink-0 flex-col">
            <div className="mb-2 flex items-center gap-2 pl-1">
              <span className="text-[13px] font-semibold text-[var(--color-text-primary)]">{pending.length} proposal{pending.length === 1 ? "" : "s"}</span>
              <span className="text-[11px] text-[var(--color-text-muted)]">waiting</span>
            </div>
            {/* Reuse the canonical Brief ProposalCard so the deck keeps the
                full contract: reason picker on reject, "Review on …" focus
                routing, and generated-b-roll disclosure. */}
            <ProposalCard key={cur.id} proposal={cur} />
            <div className="mt-2 flex flex-col gap-1.5 overflow-auto">
              {pending.map((p, i) => i === active ? null : (
                <button key={p.id} onClick={() => setActive(i)}
                  className="glass glass-reactive flex items-center gap-2 rounded-xl px-3 py-2 text-left">
                  <span className="h-1.5 w-1.5 shrink-0 rounded-full" style={{ background: mediumColor(p.medium), boxShadow: `0 0 8px ${mediumColor(p.medium)}` }} />
                  <span className="truncate text-[12px] text-[var(--color-text-secondary)]">{p.title}</span>
                </button>
              ))}
            </div>
          </div>
        ) : null}
      </div>

      {/* timeline glass strip — height grows with track count so every track
          is visible at once; past the soft cap it scrolls (never clips a
          track). Chat lives on the right, so the bottom timeline remains
          available while the conversation is open. */}
      <div className="absolute bottom-6 z-20" style={{ opacity: onStage_ ? 1 : 0.25, pointerEvents: onStage_ ? "auto" : "none", left: 0, right: 0 }}>
        <div className="glass glass-soft overflow-y-auto" style={{ borderRadius: 14, height: timelineHeight, position: "relative" }}>
          <div
            className="stage-timeline-resize-handle"
            onPointerDown={beginTimelineResize}
            onPointerMove={moveTimelineResize}
            onPointerUp={endTimelineResize}
            onPointerCancel={endTimelineResize}
            role="separator"
            aria-orientation="horizontal"
            aria-label="Resize timeline"
          />
          {timeline}
        </div>
      </div>

      {/* destination sheets slide over the dimmed stage */}
      {visibleDestination ? (
        <div
          className="absolute inset-0 z-30 flex items-stretch px-20 pt-16 pb-44"
          style={{
            opacity: onStage_ ? 0 : 1,
            pointerEvents: onStage_ ? "none" : "auto",
          }}
        >
          <div className="glass glass-strong relative mx-auto flex h-full w-full max-w-[1000px] flex-col overflow-hidden" style={{ borderRadius: 22 }}>
            <div className="flex items-center gap-3 border-b border-[var(--glass-border)] px-5 py-3">
              <span className="text-[14px] font-bold capitalize text-[var(--color-text-primary)]">{visibleDestination}</span>
              <button onClick={() => onStage("edit")} className="glass-ghost ml-auto rounded-lg px-3 py-1.5 text-[12px]">← Stage</button>
            </div>
            <div className="min-h-0 flex-1 overflow-auto">
              {visibleDestination === "deliver" ? deliver : visibleDestination === "schedule" ? schedule : visibleDestination === "skills" ? skills : visibleDestination === "history" ? history : null}
            </div>
          </div>
        </div>
      ) : null}

    </div>
    </>
  );
}

function toolNode(key: ToolKey, tools: StageShellProps["tools"]): ReactNode {
  if (!tools) return null;
  return tools[key];
}

function rightPaneNode(
  key: RightPaneKey,
  nodes: Pick<StageShellProps, "deliver" | "tools">,
): ReactNode {
  if (key === "deliver") return nodes.deliver;
  if (key === "inspector" || key === "vedit" || key === "notes") {
    return toolNode(key, nodes.tools);
  }
  return null;
}

function clamp(value: number, min: number, max: number): number {
  return Math.min(Math.max(value, min), max);
}

function clampTimelineHeight(value: number): number {
  const viewportCap =
    typeof window === "undefined" ? 520 : Math.max(TL_MIN_PX, Math.round(window.innerHeight * TL_MAX_VH));
  return Math.round(clamp(value, TL_MIN_PX, viewportCap));
}
