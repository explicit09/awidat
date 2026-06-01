import React, { useState } from "react";
import ReactDOM from "react-dom/client";
import "./ui/tokens.css";
import "./ui/glass.css";
import { AmbientBackground, useCursorGlass } from "./ui/glass";

/* ====================================================================
   Awidat "Stage" — a NEW 2026 UX (not a reskin of the cockpit).
   Combination of three directions:
     • Cinematic Stage  → the footage is the hero, chrome is minimal + floating
     • Conversation     → a persistent command bar drives every edit
     • Proposal Deck    → the agent's proposals float in as a reviewable glass deck
   Built from the Obsidian Glass material. Browser-openable mockup.
   ==================================================================== */

function Mark({ size = 26 }: { size?: number }) {
  return (
    <div
      className="grid place-items-center rounded-xl"
      style={{
        width: size, height: size,
        background: "linear-gradient(160deg,#1b1b1f,#0e0e11)",
        border: "1px solid rgba(255,255,255,0.12)",
        boxShadow: "0 0 0 1px rgba(255,122,24,0.25), 0 4px 16px rgba(255,122,24,0.30)",
      }}
    >
      <svg width={size * 0.5} height={size * 0.5} viewBox="0 0 24 24" aria-hidden>
        <defs>
          <linearGradient id="t" x1="0" y1="0" x2="0" y2="1">
            <stop offset="0" stopColor="#FFB040" /><stop offset="1" stopColor="#FF7A18" />
          </linearGradient>
        </defs>
        <path d="M12 3 L21 20 L3 20 Z" fill="url(#t)" />
      </svg>
    </div>
  );
}

const MEDIUM = {
  cut: { c: "#67E8F9", label: "cut" },
  broll: { c: "#D8B4FE", label: "b-roll" },
  color: { c: "#FCD34D", label: "color" },
  audio: { c: "#CBD5E1", label: "audio" },
} as const;
type Medium = keyof typeof MEDIUM;

type Proposal = {
  id: string; medium: Medium; title: string; range: string; rationale: string;
};
const PROPOSALS: Proposal[] = [
  { id: "1", medium: "cut", title: "Trim cold open", range: "00:12 – 00:18", rationale: "Silence > 300ms exceeded the podcast threshold in AGENTS.md." },
  { id: "2", medium: "cut", title: "Cut filler", range: "00:24 – 00:32", rationale: 'Transcript flagged "um, like…" — 7s removed.' },
  { id: "3", medium: "broll", title: "Insert B-roll", range: "00:41 – 00:46", rationale: "Generated · replicate/sd-3 · 'studio skyline'. Disclosure auto-added." },
  { id: "4", medium: "color", title: "Warm grade", range: "scene 3", rationale: "Shadows read cold at 3200K against the 5600K key." },
];

/* ---- the floating proposal deck (right side, over the stage) ---- */
function ProposalDeck({
  items, active, setActive, onDecide,
}: {
  items: Proposal[]; active: number; setActive: (i: number) => void;
  onDecide: (id: string) => void;
}) {
  const cur = items[active];
  const m = MEDIUM[cur.medium];
  const { ref, onMouseMove } = useCursorGlass<HTMLDivElement>();
  return (
    <div className="pointer-events-auto flex w-[300px] flex-col gap-2">
      {/* focused card */}
      <div
        ref={ref} onMouseMove={onMouseMove}
        className="glass glass-strong glass-reactive relative overflow-hidden p-3"
        style={{ borderRadius: 16, boxShadow: `0 0 0 1px ${m.c}55, 0 0 28px ${m.c}30, var(--glass-shadow-lift)` }}
      >
        <div className="flex items-center gap-2">
          <span className="rounded-md px-1.5 py-0.5 font-mono text-[9px] uppercase"
            style={{ color: m.c, background: `${m.c}22`, border: `1px solid ${m.c}44` }}>
            {m.label}
          </span>
          <span className="font-mono text-[10px] text-[var(--color-text-muted)]">{cur.range}</span>
          <span className="ml-auto font-mono text-[10px] text-[var(--color-text-muted)]">
            {active + 1} / {items.length}
          </span>
        </div>
        {/* preview thumb */}
        <div className="mt-2 grid h-[120px] place-items-center overflow-hidden rounded-lg"
          style={{ background: "linear-gradient(135deg,#11202b,#0c1620)", border: "1px solid rgba(255,255,255,0.08)" }}>
          <div className="text-[11px] text-[var(--color-text-muted)]">▶ before / after</div>
        </div>
        <div className="mt-2 text-[13px] font-semibold text-[var(--color-text-primary)]">{cur.title}</div>
        <div className="mt-0.5 text-[11px] italic leading-snug text-[var(--color-text-muted)]">{cur.rationale}</div>
        <div className="mt-3 flex items-center gap-2">
          <button onClick={() => onDecide(cur.id)}
            className="glass-cta h-8 flex-1 rounded-lg text-[12px]">✓ Accept</button>
          <button onClick={() => onDecide(cur.id)}
            className="glass-ghost h-8 rounded-lg px-3 text-[12px]">✕</button>
          <button onClick={() => setActive((active + 1) % items.length)}
            className="glass-ghost h-8 rounded-lg px-3 text-[12px]">→</button>
        </div>
      </div>
      {/* stacked peek of the rest */}
      <div className="flex flex-col gap-1.5">
        {items.map((p, i) => i === active ? null : (
          <button key={p.id} onClick={() => setActive(i)}
            className="glass glass-reactive flex items-center gap-2 rounded-xl px-3 py-2 text-left">
            <span className="h-1.5 w-1.5 rounded-full" style={{ background: MEDIUM[p.medium].c, boxShadow: `0 0 8px ${MEDIUM[p.medium].c}` }} />
            <span className="text-[12px] text-[var(--color-text-secondary)]">{p.title}</span>
            <span className="ml-auto font-mono text-[10px] text-[var(--color-text-muted)]">{p.range}</span>
          </button>
        ))}
      </div>
    </div>
  );
}

function Stage() {
  const [active, setActive] = useState(0);
  const [pending, setPending] = useState(PROPOSALS);
  const [draft, setDraft] = useState("");
  const decide = (id: string) => {
    setPending((p) => {
      const next = p.filter((x) => x.id !== id);
      setActive((a) => Math.min(a, Math.max(0, next.length - 1)));
      return next;
    });
  };

  return (
    <div className="relative h-screen w-screen overflow-hidden font-sans text-[var(--color-text-primary)]">
      <AmbientBackground />

      {/* floating top chrome — thin, glass, doesn't box the content */}
      <div className="absolute inset-x-0 top-0 z-30 flex items-center gap-3 px-5 py-3">
        <Mark />
        <span className="font-mono text-[12px] tracking-[0.18em] text-[var(--color-text-secondary)]">AWIDAT</span>
        <span className="glass-ghost rounded-lg px-2.5 py-1 font-mono text-[11px] text-[var(--color-text-muted)]">
          new_cast · <span className="text-[var(--color-text-secondary)]">podcast</span>
        </span>
        <div className="ml-auto flex items-center gap-3">
          <span className="flex items-center gap-1.5 text-[11px] text-[var(--color-text-muted)]">
            <span className="h-1.5 w-1.5 rounded-full bg-[#20C997] shadow-[0_0_8px_#20C997]" /> ready
          </span>
          <div className="glass-ghost flex items-center gap-1 rounded-full p-0.5 text-[11px]">
            <span className="rounded-full bg-[rgba(255,122,24,0.18)] px-2.5 py-0.5 font-semibold text-[#FF9A45]">Pro</span>
            <span className="px-2 text-[var(--color-text-muted)]">Creator</span>
          </div>
          <span className="font-mono text-[11px] text-[var(--color-text-muted)]">00:06:35:21</span>
        </div>
      </div>

      {/* THE STAGE — footage is the hero; proposal deck rides alongside */}
      <div className="absolute inset-0 z-10 flex items-center justify-center gap-7 px-10 pt-16 pb-40">
        <div className="relative w-full max-w-[860px]">
          <div className="relative aspect-video w-full overflow-hidden rounded-2xl"
            style={{
              background: "radial-gradient(ellipse at 38% 30%, #21384a, #0c151d 70%)",
              border: "1px solid rgba(255,255,255,0.10)",
              boxShadow: "0 30px 80px rgba(0,0,0,0.55), 0 0 0 1px rgba(255,255,255,0.04)",
            }}>
            {/* mock two-shot */}
            <div className="absolute inset-0 grid place-items-center">
              <div className="text-center">
                <div className="text-[13px] font-semibold tracking-wide text-[var(--color-text-secondary)]">▶ PREVIEW</div>
                <div className="mt-1 text-[11px] text-[var(--color-text-muted)]">podcast two-shot · 1920×1080</div>
              </div>
            </div>
            {/* play scrubber overlay */}
            <div className="absolute inset-x-0 bottom-0 flex items-center gap-3 px-4 py-3"
              style={{ background: "linear-gradient(0deg, rgba(0,0,0,0.5), transparent)" }}>
              <button className="grid h-8 w-8 place-items-center rounded-full glass-ghost text-[12px]">▶</button>
              <div className="h-1 flex-1 rounded-full bg-[rgba(255,255,255,0.14)]">
                <div className="h-full w-[34%] rounded-full" style={{ background: "#FF7A18", boxShadow: "0 0 10px #FF7A18" }} />
              </div>
              <span className="font-mono text-[10px] text-[var(--color-text-secondary)]">06:35 / 19:12</span>
            </div>
          </div>

        </div>

        {/* proposal deck — rides alongside the stage as a glass column */}
        {pending.length > 0 && (
          <div className="z-20">
            <div className="mb-2 flex items-center gap-2 pl-1">
              <span className="text-[13px] font-semibold text-[var(--color-text-primary)]">
                {pending.length} proposals
              </span>
              <span className="text-[11px] text-[var(--color-text-muted)]">waiting</span>
            </div>
            <ProposalDeck items={pending} active={active} setActive={setActive} onDecide={decide} />
          </div>
        )}
      </div>

      {/* timeline as a glass overlay strip */}
      <div className="absolute inset-x-8 bottom-24 z-20">
        <div className="glass glass-soft flex items-center gap-2 rounded-xl px-3 py-2" style={{ borderRadius: 14 }}>
          <span className="font-mono text-[10px] text-[var(--color-text-muted)]">19:12</span>
          <div className="flex h-9 flex-1 items-center gap-px overflow-hidden rounded-md">
            {Array.from({ length: 48 }).map((_, i) => (
              <div key={i} className="h-full flex-1"
                style={{ background: i % 7 === 0 ? "rgba(255,122,24,0.30)" : "rgba(255,255,255,0.06)" }} />
            ))}
          </div>
          <span className="font-mono text-[10px] text-[var(--color-text-muted)]">2 tracks</span>
        </div>
      </div>

      {/* conversation command bar — always present, drives every edit */}
      <div className="absolute inset-x-0 bottom-0 z-30 flex justify-center px-8 pb-6">
        <div className="glass glass-strong glass-reactive flex w-full max-w-[760px] items-center gap-3 rounded-2xl px-4 py-3"
          style={{ borderRadius: 18 }}>
          <span className="grid h-7 w-7 shrink-0 place-items-center rounded-lg"
            style={{ background: "rgba(255,122,24,0.16)", color: "#FF9A45" }}>◇</span>
          <input
            value={draft} onChange={(e) => setDraft(e.target.value)}
            placeholder="tighten this to 10 min · punch up the open · find the best pull-quote…"
            className="min-w-0 flex-1 bg-transparent text-[13px] text-[var(--color-text-primary)] placeholder:text-[var(--color-text-muted)] outline-none"
          />
          <span className="hidden items-center gap-1 sm:flex">
            {["trim", "b-roll", "caption"].map((s) => (
              <button key={s} className="glass-ghost rounded-lg px-2 py-1 text-[11px]">{s}</button>
            ))}
          </span>
          <button className="glass-cta grid h-8 w-8 place-items-center rounded-xl text-[13px]">▸</button>
        </div>
      </div>

      {/* tiny mode hint bottom-left */}
      <div className="absolute bottom-7 left-6 z-30 font-mono text-[10px] text-[var(--color-text-disabled)]">
        Stage · ⌘K command · ⌘↵ accept
      </div>
    </div>
  );
}

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <Stage />
  </React.StrictMode>,
);
