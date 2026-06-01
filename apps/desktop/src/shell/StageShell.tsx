import { type ReactNode, useEffect, useState } from "react";
import { useCursorGlass } from "../ui/glass";
import { useBriefProposalsStore, type BriefMedium } from "../state/briefProposals";
import type { Stage } from "../state/stages";
import { ChatStream } from "../agent/ChatStream";
import mark from "../brand/awidat-mark.svg";

/**
 * StageShell — the 2026 "Stage" application shell (replaces the
 * three-rail AppShell cockpit).
 *
 *   • The footage preview is the hero, centered + large.
 *   • The agent's proposals ride alongside as a swipeable glass deck,
 *     wired to the real useBriefProposalsStore (accept / reject).
 *   • A persistent command bar drives edits AND navigation.
 *   • Deliver / Skills / History are summoned via the thin left dock
 *     (or command routes) and slide in as glass sheets over the dimmed
 *     stage — the command bar + dock persist, so context is never lost.
 *
 * This component is pure layout: every real surface (preview, timeline,
 * delivery/skills/history) is passed in as a node by App.tsx, which owns
 * the data wiring. The proposal deck reads the store directly.
 */

const ORANGE = "#FF7A18";

const MEDIUM_COLOR: Record<string, string> = {
  cut: "#67E8F9",
  color: "#FCD34D",
  broll: "#D8B4FE",
  audio: "#CBD5E1",
  transition: "#D8B4FE",
  title: "#5EEAD4",
  caption: "#5EEAD4",
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
  { id: "skills", glyph: "✦", label: "Skills" },
  { id: "history", glyph: "◷", label: "History" },
];

export type StageShellProps = {
  hasProject: boolean;
  landing: ReactNode;
  /** The video preview hero (PreviewSurface), already composed by App. */
  preview: ReactNode;
  /** Real timeline node (TimelineHybrid + TimelinePane). */
  timeline: ReactNode;
  deliver: ReactNode;
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
    hasProject, landing, preview, timeline, deliver, skills, history,
    stage, onStage, onCommand, running, onCancel,
    projectLabel, projectType, timecode, agentRead,
  } = props;

  // Subscribe to the store so the deck re-renders on proposal changes,
  // then read the unified pending() view (same pattern as BriefSurface).
  useBriefProposalsStore((s) => s.approvals);
  const pending = useBriefProposalsStore.getState().pending();
  const accept = useBriefProposalsStore((s) => s.accept);
  const reject = useBriefProposalsStore((s) => s.reject);

  const [active, setActive] = useState(0);
  const [draft, setDraft] = useState("");

  const onStage_ = stage === "edit";
  const cur = pending[Math.min(active, Math.max(0, pending.length - 1))];

  // Conversation home: the command bar expands into a glass thread.
  // Auto-opens whenever the agent is working so its replies have a place
  // to land (the gap the old cockpit's rail used to fill).
  const [convoOpen, setConvoOpen] = useState(false);
  useEffect(() => {
    if (running) setConvoOpen(true);
  }, [running]);

  const submit = () => {
    const text = draft.trim();
    if (!text) return;
    // Command routes: bare destination words navigate; everything else
    // is an editorial instruction to the agent.
    const lower = text.toLowerCase().replace(/^\//, "");
    if (lower === "deliver" || lower === "skills" || lower === "history" || lower === "stage" || lower === "edit") {
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
        <img src={mark} width={26} height={26} alt="" className="rounded-xl" style={{ boxShadow: "0 0 0 1px rgba(255,122,24,0.25), 0 4px 16px rgba(255,122,24,0.30)" }} />
        <span className="font-mono text-[12px] tracking-[0.18em] text-[var(--color-text-secondary)]">AWIDAT</span>
        {projectLabel ? (
          <span className="glass-ghost rounded-lg px-2.5 py-1 font-mono text-[11px] text-[var(--color-text-muted)]">
            {projectLabel}{projectType ? <> · <span className="text-[var(--color-text-secondary)]">{projectType}</span></> : null}
          </span>
        ) : null}
        <div className="ml-auto flex items-center gap-3">
          <span className="flex items-center gap-1.5 text-[11px] text-[var(--color-text-muted)]">
            <span className="h-1.5 w-1.5 rounded-full" style={{ background: running ? ORANGE : "#20C997", boxShadow: `0 0 8px ${running ? ORANGE : "#20C997"}` }} />
            {running ? "working" : "ready"}
          </span>
          {timecode ? <span className="font-mono text-[11px] text-[var(--color-text-muted)]">{timecode}</span> : null}
        </div>
      </div>

      {/* left dock */}
      <div className="group/dock absolute left-3 top-1/2 z-40 -translate-y-1/2">
        <div className="glass glass-strong flex flex-col gap-1 p-1.5" style={{ borderRadius: 16 }}>
          {DOCK.map((d) => {
            const on = stage === d.id;
            return (
              <button key={d.id} onClick={() => onStage(d.id)}
                className="flex items-center gap-2.5 rounded-xl px-2.5 py-2 text-left transition"
                style={{
                  background: on ? "linear-gradient(180deg,#FF8B33,#FF7A18)" : "transparent",
                  color: on ? "#1A0E04" : "var(--color-text-muted)",
                  boxShadow: on ? "0 0 18px rgba(255,122,24,0.45)" : "none",
                }}>
                <span className="grid w-5 place-items-center text-[13px]">{d.glyph}</span>
                <span className="max-w-0 overflow-hidden whitespace-nowrap text-[12px] font-semibold opacity-0 transition-all duration-200 group-hover/dock:max-w-[80px] group-hover/dock:opacity-100">{d.label}</span>
              </button>
            );
          })}
        </div>
      </div>

      {/* STAGE LAYER — preview hero + proposal deck (dims when a destination opens) */}
      <div className="absolute inset-0 z-10 flex items-stretch justify-center gap-6 px-20 pt-16 pb-44 transition-all duration-300"
        style={{ filter: onStage_ ? "none" : "blur(8px) brightness(0.5)", transform: onStage_ ? "none" : "scale(0.98)", pointerEvents: onStage_ ? "auto" : "none" }}>
        <div className="flex min-w-0 flex-1 flex-col gap-2">
          <div className="glass relative min-h-0 flex-1 overflow-hidden" style={{ borderRadius: 18 }}>
            {preview}
          </div>
          {agentRead ? (
            <div className="flex items-center gap-2 pl-1 text-[11px] text-[var(--color-text-muted)]">
              <span className="text-[#FF9A45]">◇</span> {agentRead}
            </div>
          ) : null}
        </div>

        {pending.length > 0 && cur ? (
          <div className="flex w-[300px] shrink-0 flex-col">
            <div className="mb-2 flex items-center gap-2 pl-1">
              <span className="text-[13px] font-semibold text-[var(--color-text-primary)]">{pending.length} proposal{pending.length === 1 ? "" : "s"}</span>
              <span className="text-[11px] text-[var(--color-text-muted)]">waiting</span>
            </div>
            <ProposalCard
              key={cur.id}
              title={cur.title}
              medium={cur.medium}
              rationale={cur.rationale}
              index={active}
              total={pending.length}
              onAccept={() => void accept(cur.id)}
              onReject={() => void reject(cur.id)}
              onNext={() => setActive((a) => (a + 1) % pending.length)}
            />
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

      {/* timeline glass strip */}
      <div className="absolute inset-x-20 bottom-24 z-20 transition-opacity duration-300" style={{ opacity: onStage_ ? 1 : 0.25, pointerEvents: onStage_ ? "auto" : "none" }}>
        <div className="glass glass-soft overflow-hidden" style={{ borderRadius: 14, height: 96 }}>
          {timeline}
        </div>
      </div>

      {/* destination sheets slide over the dimmed stage */}
      {!onStage_ ? (
        <div className="absolute inset-0 z-30 flex items-stretch px-20 pt-16 pb-44">
          <div className="glass glass-strong relative mx-auto flex h-full w-full max-w-[1000px] flex-col overflow-hidden" style={{ borderRadius: 22 }}>
            <div className="flex items-center gap-3 border-b border-[var(--glass-border)] px-5 py-3">
              <span className="text-[14px] font-bold capitalize text-[var(--color-text-primary)]">{stage}</span>
              <button onClick={() => onStage("edit")} className="glass-ghost ml-auto rounded-lg px-3 py-1.5 text-[12px]">← Stage</button>
            </div>
            <div className="min-h-0 flex-1 overflow-auto">
              {stage === "deliver" ? deliver : stage === "skills" ? skills : stage === "history" ? history : null}
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
            style={{ background: convoOpen ? "rgba(255,122,24,0.30)" : "rgba(255,122,24,0.16)", color: "#FF9A45" }}
          >◇</button>
          <input
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={(e) => { if (e.key === "Enter") submit(); }}
            placeholder="ask, trim, propose…  or type a destination: deliver · skills · history"
            className="min-w-0 flex-1 bg-transparent text-[13px] text-[var(--color-text-primary)] placeholder:text-[var(--color-text-muted)] outline-none"
          />
          <span className="hidden items-center gap-1 md:flex">
            {(["deliver", "skills", "history"] as Stage[]).map((s) => (
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

function ProposalCard({
  title, medium, rationale, index, total, onAccept, onReject, onNext,
}: {
  title: string; medium: BriefMedium; rationale: string | undefined;
  index: number; total: number; onAccept: () => void; onReject: () => void; onNext: () => void;
}) {
  const { ref, onMouseMove } = useCursorGlass<HTMLDivElement>();
  const c = mediumColor(medium);
  return (
    <div ref={ref} onMouseMove={onMouseMove} className="glass glass-strong glass-reactive relative overflow-hidden p-3"
      style={{ borderRadius: 16, boxShadow: `0 0 0 1px ${c}55, 0 0 28px ${c}30, var(--glass-shadow-lift)` }}>
      <div className="flex items-center gap-2">
        <span className="rounded-md px-1.5 py-0.5 font-mono text-[9px] uppercase" style={{ color: c, background: `${c}22`, border: `1px solid ${c}44` }}>{medium}</span>
        <span className="ml-auto font-mono text-[10px] text-[var(--color-text-muted)]">{index + 1} / {total}</span>
      </div>
      <div className="mt-2 text-[13px] font-semibold text-[var(--color-text-primary)]">{title}</div>
      {rationale ? <div className="mt-0.5 text-[11px] italic leading-snug text-[var(--color-text-muted)]">{rationale}</div> : null}
      <div className="mt-3 flex items-center gap-2">
        <button onClick={onAccept} className="glass-cta h-8 flex-1 rounded-lg text-[12px]">✓ Accept</button>
        <button onClick={onReject} className="glass-ghost h-8 rounded-lg px-3 text-[12px]">✕</button>
        {total > 1 ? <button onClick={onNext} className="glass-ghost h-8 rounded-lg px-3 text-[12px]">→</button> : null}
      </div>
    </div>
  );
}
