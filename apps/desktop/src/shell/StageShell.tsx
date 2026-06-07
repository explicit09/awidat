import { type ReactNode, useEffect, useRef, useState } from "react";
import { useBriefProposalsStore, type BriefMedium } from "../state/briefProposals";
import { usePendingProposals } from "../timeline/pendingProposals";
import { ProposalCard } from "./brief/ProposalCard";
import { useTimelineStore } from "../timeline/store";
import type { Stage } from "../state/stages";
import { ChatStream } from "../agent/ChatStream";
import { useSettings } from "../state/settings";
import { Settings as SettingsIcon } from "lucide-react";
import mark from "../brand/awidat-mark.svg";

/**
 * StageShell — the 2026 "Stage" application shell (replaces the
 * three-rail AppShell cockpit).
 *
 *   • The footage preview is the hero, centered + large.
 *   • The agent's proposals ride alongside as a swipeable glass deck,
 *     wired to the real useBriefProposalsStore (accept / reject).
 *   • A persistent command bar drives edits AND navigation.
 *   • Deliver / Schedule / Skills / History are summoned via the thin left dock
 *     (or command routes) and slide in as glass sheets over the dimmed
 *     stage — the command bar + dock persist, so context is never lost.
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

type DockItem = { id: Stage; glyph: string; label: string };
const DOCK: DockItem[] = [
  { id: "edit", glyph: "▶", label: "Stage" },
  { id: "deliver", glyph: "↑", label: "Deliver" },
  { id: "schedule", glyph: "◷", label: "Schedule" },
  { id: "skills", glyph: "✦", label: "Skills" },
  { id: "history", glyph: "◷", label: "History" },
];

// Summonable cockpit tools — slide in as a right-side glass panel over the
// (still-visible) stage. Transcript leads: it's the click-a-word→seek surface.
type ToolKey = "transcript" | "media" | "inspector" | "index" | "vedit";
type ToolItem = { id: ToolKey; glyph: string; label: string };
const TOOLS: ToolItem[] = [
  { id: "transcript", glyph: "¶", label: "Transcript" },
  { id: "media", glyph: "▦", label: "Media" },
  { id: "inspector", glyph: "⚙", label: "Inspector" },
  { id: "index", glyph: "☰", label: "Index" },
  { id: "vedit", glyph: "✎", label: "Vedit" },
];

// Timeline strip sizing — fits ALL tracks without vertical scroll.
const TL_BASE = 48; // header/padding chrome
const TL_ROW = 58; // per-track lane height
const TL_MAX_VH = 52; // soft cap; beyond this the strip scrolls (never clips)

// Tool side-panel geometry — single source of truth (was 3 duplicated
// magic numbers 372/380/380 that had to stay in sync by hand).
const TOOL_W = 372; // panel width
const TOOL_GUTTER = 12; // gap from the right edge
const TOOL_DOCK_W = 44; // right dock width
const TOOL_RESERVE = TOOL_W + TOOL_GUTTER + TOOL_DOCK_W + 12; // stage reflow when a tool is open

export type StageShellProps = {
  hasProject: boolean;
  landing: ReactNode;
  /** The video preview hero (PreviewSurface), already composed by App. */
  preview: ReactNode;
  /** Real timeline node (TimelineHybrid + TimelinePane). */
  timeline: ReactNode;
  /** Track count drives the timeline strip height (fit all, no scroll). */
  trackCount?: number;
  /** Summonable cockpit tools, opened from the right-edge dock. */
  tools?: {
    media: ReactNode;
    inspector: ReactNode;
    index: ReactNode;
    transcript: ReactNode;
    vedit: ReactNode;
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
    stage, onStage, onCommand, running, onCancel,
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

  // Conversation home: the command bar expands into a glass thread.
  // Auto-opens whenever the agent is working so its replies have a place
  // to land (the gap the old cockpit's rail used to fill).
  const [convoOpen, setConvoOpen] = useState(false);
  useEffect(() => {
    if (running) setConvoOpen(true);
  }, [running]);

  // Summonable tool side-panel — one open at a time.
  // Dev-only: VITE_AWIDAT_TOOL pre-opens a tool (native screenshot verify).
  const devTool = (import.meta.env?.VITE_AWIDAT_TOOL as ToolKey | undefined) ?? null;
  const [tool, setTool] = useState<ToolKey | null>(devTool);
  const toggleTool = (id: ToolKey) => setTool((t) => (t === id ? null : id));

  // Auto-open the Inspector on the rising edge of a selection — never fight
  // the user (don't reopen if they've closed it while the selection holds).
  const prevAutoInspect = useRef(false);
  useEffect(() => {
    if (autoInspect && !prevAutoInspect.current) setTool("inspector");
    prevAutoInspect.current = !!autoInspect;
  }, [autoInspect]);

  // Track-sized timeline strip — fit every track, never scroll vertically.
  // Falls back to the live store count if App didn't pass one.
  const storeTrackCount = useTimelineStore((s) => s.snapshot.tracks.length);
  const tracks = trackCount || storeTrackCount;
  const timelineHeight = `min(${TL_MAX_VH}vh, ${TL_BASE + Math.max(1, tracks) * TL_ROW}px)`;

  // Empty stage: a project is loaded but there's NO footage on the timeline
  // yet (no tracks) AND nothing proposed. "No proposals" alone is not empty —
  // the user may already have footage they're working with.
  const stageEmpty = pending.length === 0 && tracks === 0;
  const node = tool ? toolNode(tool, tools) : null;

  const submit = () => {
    const text = draft.trim();
    if (!text) return;
    // Command routes: bare destination words navigate; everything else
    // is an editorial instruction to the agent.
    const lower = text.toLowerCase().replace(/^\//, "");
    if (lower === "transcript" || lower === "media" || lower === "inspector" || lower === "index" || lower === "vedit") {
      onStage("edit"); // tools live on the Stage
      setTool(lower as ToolKey);
    } else if (lower === "deliver" || lower === "schedule" || lower === "skills" || lower === "history" || lower === "stage" || lower === "edit") {
      onStage(lower === "stage" ? "edit" : (lower as Stage));
    } else {
      onCommand(text);
      setConvoOpen(true); // show the thread so the reply is visible
    }
    setDraft("");
  };

  if (!hasProject) {
    return (
      <div className="relative z-10 h-full w-full">{landing}</div>
    );
  }

  return (
    <div className="relative z-10 h-full w-full overflow-hidden font-sans text-[var(--color-text-primary)]">
      {/* floating top chrome */}
      <div className="absolute inset-x-0 top-0 z-30 flex items-center gap-3 px-5 py-3" data-tauri-drag-region>
        <img src={mark} width={26} height={26} alt="" className="rounded-xl" style={{ boxShadow: "0 0 0 1px rgba(239,68,68,0.30), 0 4px 16px rgba(239,68,68,0.34)" }} />
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

      {/* left dock */}
      <div className="group/dock absolute left-3 top-1/2 z-40 -translate-y-1/2">
        <div className="glass glass-strong flex flex-col gap-1 p-1.5" style={{ borderRadius: 16 }}>
          {DOCK.map((d) => {
            const on = stage === d.id;
            return (
              <button
                key={d.id}
                onClick={() => onStage(d.id)}
                data-perf-stage-switch={d.id}
                data-active={on ? "true" : "false"}
                className="flex items-center gap-2.5 rounded-xl px-2.5 py-2 text-left transition"
                style={{
                  background: on ? "linear-gradient(180deg,#FB7185,#EF4444)" : "transparent",
                  color: on ? "#FFFFFF" : "var(--color-text-muted)",
                  boxShadow: on ? "0 0 18px rgba(239,68,68,0.45)" : "none",
                }}>
                <span className="grid w-5 place-items-center text-[13px]">{d.glyph}</span>
                <span className="max-w-0 overflow-hidden whitespace-nowrap text-[12px] font-semibold opacity-0 transition-all duration-200 group-hover/dock:max-w-[80px] group-hover/dock:opacity-100">{d.label}</span>
              </button>
            );
          })}
        </div>
      </div>

      {/* right-edge tool dock — summon cockpit tools as a side panel.
          Hidden while a destination sheet (deliver/schedule/skills/history) is open. */}
      {tools && onStage_ ? (
        <div
          className="group/tools absolute top-1/2 z-40 -translate-y-1/2 transition-[right] duration-300"
          style={{ right: tool ? TOOL_W + TOOL_GUTTER + 8 : 12 }}
        >
          <div className="glass glass-strong flex flex-col gap-1 p-1.5" style={{ borderRadius: 16 }}>
            {TOOLS.map((t) => {
              const on = tool === t.id;
              return (
                <button key={t.id} onClick={() => toggleTool(t.id)}
                  className="flex items-center gap-2.5 rounded-xl px-2.5 py-2 text-left transition"
                  style={{
                    background: on ? "linear-gradient(180deg,#FB7185,#EF4444)" : "transparent",
                    color: on ? "#FFFFFF" : "var(--color-text-muted)",
                    boxShadow: on ? "0 0 18px rgba(239,68,68,0.45)" : "none",
                  }}>
                  <span className="order-2 max-w-0 overflow-hidden whitespace-nowrap text-[12px] font-semibold opacity-0 transition-all duration-200 group-hover/tools:max-w-[90px] group-hover/tools:opacity-100">{t.label}</span>
                  <span className="order-1 grid w-5 place-items-center text-[13px]">{t.glyph}</span>
                </button>
              );
            })}
          </div>
        </div>
      ) : null}

      {/* tool side-panel — slides in from the right; the stage stays visible
          to its left (NOT a full sheet like the destinations). */}
      {tools && onStage_ && tool ? (
        <div
          className="stage-tool-panel glass glass-strong z-30 flex flex-col overflow-hidden"
          style={{
            position: "absolute",
            top: 64,
            bottom: 88,
            right: TOOL_GUTTER,
            left: "auto",
            width: TOOL_W,
            maxWidth: "calc(100vw - 24px)",
            borderRadius: 18,
          }}>
          <div className="flex items-center gap-2 border-b border-[var(--glass-border)] px-4 py-2.5">
            <span className="text-[13px] font-semibold capitalize text-[var(--color-text-primary)]">{tool}</span>
            <button onClick={() => setTool(null)} className="gk-close ml-auto" aria-label="Close">×</button>
          </div>
          <div className="stage-tool-body flex min-h-0 flex-1 flex-col overflow-auto">{node}</div>
        </div>
      ) : null}

      {/* STAGE LAYER — preview hero + proposal deck (dims when a destination
          opens). Bottom padding tracks the (track-sized) timeline height so
          the hero flexes; right padding makes room for an open tool panel. */}
      <div className="absolute inset-0 z-10 flex items-stretch justify-center gap-6 px-20 pt-16"
        style={{
          filter: onStage_ ? "none" : "brightness(0.58)",
          pointerEvents: onStage_ ? "auto" : "none",
          paddingBottom: convoOpen ? 120 : `calc(96px + 56px + ${timelineHeight})`,
          paddingRight: tool ? TOOL_RESERVE : undefined,
        }}>
        <div className="relative flex min-w-0 flex-1 flex-col gap-2">
          <div className="glass relative min-h-0 flex-1 overflow-hidden" style={{ borderRadius: 18 }}>
            {preview}
            {/* purposeful empty state — overlays the black hero when there's
                nothing to review yet (no pending proposals). */}
            {stageEmpty ? (
              <div className="absolute inset-0 z-10 grid place-items-center p-8 text-center">
                <div className="flex max-w-[420px] flex-col items-center gap-4">
                  <div className="text-[22px] font-bold tracking-tight text-[var(--color-text-primary)]">Direct the edit.</div>
                  <div className="text-[13px] leading-relaxed text-[var(--color-text-secondary)]">
                    Drop footage, pick media, or ask me to prepare a cut — I'll propose edits you review here.
                  </div>
                  <div className="mt-1 flex items-center gap-2">
                    <button
                      onClick={() => onCommand("Prepare a starting cut for this project using AGENTS.md and the indexed signals.")}
                      className="glass-cta rounded-xl px-4 py-2 text-[13px] font-semibold"
                    >Prepare a starting cut</button>
                    <button
                      onClick={() => { onStage("edit"); setTool("media"); }}
                      className="glass-ghost rounded-xl px-4 py-2 text-[13px]"
                    >Open media</button>
                  </div>
                </div>
              </div>
            ) : null}
          </div>
          {agentRead ? (
            <div className="flex items-center gap-2 pl-1 text-[11px] text-[var(--color-text-muted)]">
              <span className="text-[var(--color-brand-hover)]">◇</span> {agentRead}
            </div>
          ) : null}
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
          track). Hidden while the conversation panel is open so the two don't
          fight for the bottom region. */}
      {!convoOpen ? (
        <div className="absolute inset-x-20 bottom-24 z-20" style={{ opacity: onStage_ ? 1 : 0.25, pointerEvents: onStage_ ? "auto" : "none", right: tool ? TOOL_RESERVE : undefined }}>
          <div className="glass glass-soft overflow-y-auto" style={{ borderRadius: 14, height: timelineHeight }}>
            {timeline}
          </div>
        </div>
      ) : null}

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

      {/* command bar + conversation home — edits, navigates, and shows
          the agent's replies in a glass thread that grows from the bar */}
      <div className="absolute inset-x-0 bottom-0 z-40 flex flex-col items-center px-8 pb-6">
        {convoOpen ? (
          <div className="glass glass-strong stage-convo mb-2 flex w-full max-w-[760px] flex-col overflow-hidden" style={{ borderRadius: 18, maxHeight: "min(46vh, 460px)" }}>
            <div className="flex items-center gap-2 border-b border-[var(--glass-border)] px-4 py-2">
              <span className="text-[11px] font-semibold text-[var(--color-text-secondary)]">Conversation</span>
              {agentRead ? <span className="text-[10px] text-[var(--color-text-muted)]">· {agentRead}</span> : null}
              <button onClick={() => setConvoOpen(false)} className="glass-ghost ml-auto rounded-md px-2 py-0.5 text-[11px]">▾ collapse</button>
            </div>
            <div className="min-h-0 flex-1 overflow-auto">
              <ChatStream />
            </div>
          </div>
        ) : null}
        <div className="glass glass-strong glass-reactive flex w-full max-w-[760px] items-center gap-3 rounded-2xl px-4 py-3" style={{ borderRadius: 18 }}>
          <button
            onClick={() => setConvoOpen((o) => !o)}
            title={convoOpen ? "Hide conversation" : "Show conversation"}
            className="grid h-7 w-7 shrink-0 place-items-center rounded-lg transition"
            style={{ background: convoOpen ? "rgba(239,68,68,0.30)" : "rgba(239,68,68,0.16)", color: "#FCA5A5" }}
          >◇</button>
          <input
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={(e) => { if (e.key === "Enter") submit(); }}
            placeholder="ask, trim, propose…  or type a destination: deliver · schedule · skills · history"
            className="min-w-0 flex-1 bg-transparent text-[13px] text-[var(--color-text-primary)] placeholder:text-[var(--color-text-muted)] outline-none"
          />
          <span className="hidden items-center gap-1 md:flex">
            {(["deliver", "schedule", "skills", "history"] as Stage[]).map((s) => (
              <button key={s} onClick={() => onStage(s)} className="glass-ghost rounded-lg px-2 py-1 text-[11px]">/{s}</button>
            ))}
          </span>
          {running ? (
            <button onClick={onCancel} className="glass-ghost grid h-8 w-8 place-items-center rounded-xl text-[13px]">■</button>
          ) : (
            <button onClick={submit} className="glass-cta grid h-8 w-8 place-items-center rounded-xl text-[13px]">▸</button>
          )}
        </div>
      </div>
    </div>
  );
}

function toolNode(key: ToolKey, tools: StageShellProps["tools"]): ReactNode {
  if (!tools) return null;
  return tools[key];
}
