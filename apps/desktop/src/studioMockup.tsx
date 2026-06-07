import React, { useState } from "react";
import ReactDOM from "react-dom/client";
import "./ui/tokens.css";
import "./ui/glass.css";
import { AmbientBackground, useCursorGlass } from "./ui/glass";

/* ====================================================================
   Montage "Stage" — NEW 2026 UX (v2, product-driven decisions).

   Core bet: the command bar is the OS of the app. One input drives
   BOTH editing ("trim filler") and navigation ("deliver"). Destinations
   are not tabs — they slide in as glass sheets over the dimmed stage,
   so you never lose the footage or the command line.

     • Cinematic Stage  — footage is the hero, chrome floats
     • Conversation     — persistent command bar = edit + navigate
     • Proposal Deck    — proposals ride alongside AND pin to the timeline
     • Thin left dock   — Stage / Deliver / Skills / History (hover-expand)
     • Agent "read"     — always-legible Copilot relationship
   ==================================================================== */

const ORANGE = "#FF7A18";
const MEDIUM = {
  cut: { c: "#67E8F9", label: "cut" },
  broll: { c: "#D8B4FE", label: "b-roll" },
  color: { c: "#FCD34D", label: "color" },
  audio: { c: "#CBD5E1", label: "audio" },
} as const;
type Medium = keyof typeof MEDIUM;

type Proposal = { id: string; medium: Medium; title: string; range: string; at: number; rationale: string };
const PROPOSALS: Proposal[] = [
  { id: "1", medium: "cut", title: "Trim cold open", range: "00:12 – 00:18", at: 0.06, rationale: "Silence > 300ms exceeded the podcast threshold in AGENTS.md." },
  { id: "2", medium: "cut", title: "Cut filler", range: "00:24 – 00:32", at: 0.14, rationale: 'Transcript flagged "um, like…" — 7s removed.' },
  { id: "3", medium: "broll", title: "Insert B-roll", range: "00:41 – 00:46", at: 0.33, rationale: "Generated · replicate/sd-3 · 'studio skyline'. Disclosure auto-added." },
  { id: "4", medium: "color", title: "Warm grade", range: "scene 3", at: 0.58, rationale: "Shadows read cold at 3200K against the 5600K key." },
];

type View = "stage" | "deliver" | "skills" | "history";

/* --------------------------------- mark --------------------------------- */
function Mark({ size = 26 }: { size?: number }) {
  return (
    <div className="grid shrink-0 place-items-center rounded-xl" style={{
      width: size, height: size, background: "linear-gradient(160deg,#1b1b1f,#0e0e11)",
      border: "1px solid rgba(255,255,255,0.12)",
      boxShadow: "0 0 0 1px rgba(255,122,24,0.25), 0 4px 16px rgba(255,122,24,0.30)",
    }}>
      <svg width={size * 0.5} height={size * 0.5} viewBox="0 0 24 24" aria-hidden>
        <defs><linearGradient id="t" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stopColor="#FFB040" /><stop offset="1" stopColor="#FF7A18" /></linearGradient></defs>
        <path d="M12 3 L21 20 L3 20 Z" fill="url(#t)" />
      </svg>
    </div>
  );
}

/* ----------------------------- left dock ----------------------------- */
const DOCK: { id: View; glyph: string; label: string }[] = [
  { id: "stage", glyph: "▶", label: "Stage" },
  { id: "deliver", glyph: "↑", label: "Deliver" },
  { id: "skills", glyph: "✦", label: "Skills" },
  { id: "history", glyph: "◷", label: "History" },
];
function LeftDock({ view, setView }: { view: View; setView: (v: View) => void }) {
  return (
    <div className="group/dock absolute left-3 top-1/2 z-40 -translate-y-1/2">
      <div className="glass glass-strong flex flex-col gap-1 p-1.5" style={{ borderRadius: 16 }}>
        {DOCK.map((d) => {
          const on = view === d.id;
          return (
            <button key={d.id} onClick={() => setView(d.id)}
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
  );
}

/* --------------------------- proposal deck --------------------------- */
function ProposalDeck({ items, active, setActive, onDecide }: {
  items: Proposal[]; active: number; setActive: (i: number) => void; onDecide: (id: string) => void;
}) {
  const cur = items[active]; const m = MEDIUM[cur.medium];
  const { ref, onMouseMove } = useCursorGlass<HTMLDivElement>();
  return (
    <div className="flex w-[290px] flex-col gap-2">
      <div ref={ref} onMouseMove={onMouseMove} className="glass glass-strong glass-reactive relative overflow-hidden p-3"
        style={{ borderRadius: 16, boxShadow: `0 0 0 1px ${m.c}55, 0 0 28px ${m.c}30, var(--glass-shadow-lift)` }}>
        <div className="flex items-center gap-2">
          <span className="rounded-md px-1.5 py-0.5 font-mono text-[9px] uppercase" style={{ color: m.c, background: `${m.c}22`, border: `1px solid ${m.c}44` }}>{m.label}</span>
          <span className="font-mono text-[10px] text-[var(--color-text-muted)]">{cur.range}</span>
          <span className="ml-auto font-mono text-[10px] text-[var(--color-text-muted)]">{active + 1} / {items.length}</span>
        </div>
        <div className="mt-2 grid h-[116px] place-items-center overflow-hidden rounded-lg" style={{ background: "linear-gradient(135deg,#11202b,#0c1620)", border: "1px solid rgba(255,255,255,0.08)" }}>
          <div className="text-[11px] text-[var(--color-text-muted)]">▶ before / after</div>
        </div>
        <div className="mt-2 text-[13px] font-semibold text-[var(--color-text-primary)]">{cur.title}</div>
        <div className="mt-0.5 text-[11px] italic leading-snug text-[var(--color-text-muted)]">{cur.rationale}</div>
        <div className="mt-3 flex items-center gap-2">
          <button onClick={() => onDecide(cur.id)} className="glass-cta h-8 flex-1 rounded-lg text-[12px]">✓ Accept</button>
          <button onClick={() => onDecide(cur.id)} className="glass-ghost h-8 rounded-lg px-3 text-[12px]">✕</button>
          <button onClick={() => setActive((active + 1) % items.length)} className="glass-ghost h-8 rounded-lg px-3 text-[12px]">→</button>
        </div>
      </div>
      <button className="glass-ghost rounded-xl px-3 py-2 text-[11px] text-[var(--color-text-secondary)]">Review all {items.length} →</button>
    </div>
  );
}

/* ----------------------- destination sheets ----------------------- */
function SheetShell({ title, sub, onClose, children }: { title: string; sub: string; onClose: () => void; children: React.ReactNode }) {
  return (
    <div className="glass glass-strong relative mx-auto flex h-full w-full max-w-[980px] flex-col overflow-hidden" style={{ borderRadius: 22 }}>
      <div className="flex items-center gap-3 border-b border-[var(--glass-border)] px-5 py-3.5">
        <span className="text-[15px] font-bold text-[var(--color-text-primary)]">{title}</span>
        <span className="text-[11px] text-[var(--color-text-muted)]">{sub}</span>
        <button onClick={onClose} className="glass-ghost ml-auto rounded-lg px-3 py-1.5 text-[12px]">← Stage</button>
      </div>
      <div className="min-h-0 flex-1 overflow-auto p-5">{children}</div>
    </div>
  );
}
function DeliverSheet({ onClose }: { onClose: () => void }) {
  const targets = [["YouTube", "1080p · 16:9"], ["TikTok", "1080p · 9:16"], ["Instagram", "1:1 / 9:16"], ["Captions", "SRT + VTT"]];
  return (
    <SheetShell title="Deliver" sub="0:38 timeline · preflight passed" onClose={onClose}>
      <div className="grid grid-cols-[1fr_300px] gap-5">
        <div className="space-y-2">
          {targets.map(([n, f], i) => (
            <div key={n} className="glass-content flex items-center gap-3 px-4 py-3">
              <span className="grid h-7 w-7 place-items-center rounded-lg" style={{ background: "rgba(255,122,24,0.14)", color: "#FF9A45" }}>{n[0]}</span>
              <div><div className="text-[13px] font-semibold text-[var(--color-text-primary)]">{n}</div><div className="font-mono text-[10px] text-[var(--color-text-muted)]">{f}</div></div>
              <span className="ml-auto h-4 w-7 rounded-full p-0.5" style={{ background: i < 1 ? ORANGE : "rgba(255,255,255,0.12)" }}><span className="block h-3 w-3 rounded-full bg-white transition-all" style={{ transform: i < 1 ? "translateX(12px)" : "none" }} /></span>
            </div>
          ))}
        </div>
        <div className="glass-content flex flex-col gap-3 p-4">
          <div className="text-[12px] font-semibold text-[var(--color-text-primary)]">Render summary</div>
          <div className="grid grid-cols-2 gap-2 font-mono text-[10px] text-[var(--color-text-muted)]">
            <div>duration 0:38</div><div>outputs 1</div><div>conf 90</div><div>AI ⚠ disclosed</div>
          </div>
          <button className="glass-cta mt-1 h-9 rounded-xl text-[12px]">↑ Export now</button>
          <button className="glass-ghost h-8 rounded-xl text-[11px]">Save preset</button>
        </div>
      </div>
    </SheetShell>
  );
}
function SkillsSheet({ onClose }: { onClose: () => void }) {
  const skills = ["auto-cutter", "cut-director", "pacing-optimizer", "b-roll-suggester", "color-corrector", "interview-tightener", "beat-sync-editor", "podcast-editor"];
  return (
    <SheetShell title="Skills" sub="8 loaded · the agent's editorial loadout" onClose={onClose}>
      <div className="grid grid-cols-2 gap-2.5">
        {skills.map((s, i) => (
          <div key={s} className="glass-content flex items-center gap-3 px-4 py-3">
            <span className="grid h-7 w-7 place-items-center rounded-lg" style={{ background: "rgba(56,189,248,0.14)", color: "#67E8F9" }}>✦</span>
            <div><div className="text-[13px] font-semibold text-[var(--color-text-primary)]">{s}</div><div className="font-mono text-[10px] text-[var(--color-text-muted)]">{i % 3 === 0 ? "bundled" : i % 3 === 1 ? "user" : "project"} · v1.{i}.0</div></div>
            <span className="ml-auto h-4 w-7 rounded-full p-0.5" style={{ background: ORANGE }}><span className="block h-3 w-3 translate-x-3 rounded-full bg-white" /></span>
          </div>
        ))}
      </div>
    </SheetShell>
  );
}
function HistorySheet({ onClose }: { onClose: () => void }) {
  const rows = [
    ["accepted", "#5EEAD4", "Trim cold open", "cut · silence threshold"],
    ["accepted", "#5EEAD4", "Cut filler 0:24", "transcript · um/like"],
    ["rejected", "#FCA5A5", "Insert B-roll 0:41", "reason: off-topic"],
    ["revised", "#FCD34D", "Warm grade scene 3", "−200K from proposed"],
  ];
  return (
    <SheetShell title="History" sub="4 decisions · learned: tighter cuts, less B-roll" onClose={onClose}>
      <div className="space-y-1.5">
        {rows.map(([state, c, title, why], i) => (
          <div key={i} className="glass-content flex items-center gap-3 px-4 py-2.5">
            <span className="rounded-md px-1.5 py-0.5 text-[10px] font-semibold" style={{ color: c as string, background: `${c}1f` }}>{state}</span>
            <span className="text-[12px] text-[var(--color-text-primary)]">{title}</span>
            <span className="text-[11px] italic text-[var(--color-text-muted)]">{why}</span>
            <span className="ml-auto font-mono text-[10px] text-[var(--color-text-muted)]">{i + 2}m ago</span>
          </div>
        ))}
      </div>
    </SheetShell>
  );
}

/* --------------------------------- app --------------------------------- */
function Stage() {
  const [view, setView] = useState<View>("stage");
  const [active, setActive] = useState(0);
  const [pending, setPending] = useState(PROPOSALS);
  const [draft, setDraft] = useState("");
  const decide = (id: string) => setPending((p) => { const n = p.filter((x) => x.id !== id); setActive((a) => Math.min(a, Math.max(0, n.length - 1))); return n; });

  return (
    <div className="relative h-screen w-screen overflow-hidden font-sans text-[var(--color-text-primary)]">
      <AmbientBackground />

      {/* floating top chrome */}
      <div className="absolute inset-x-0 top-0 z-30 flex items-center gap-3 px-5 py-3">
        <Mark />
        <span className="font-mono text-[12px] tracking-[0.18em] text-[var(--color-text-secondary)]">MONTAGE</span>
        <span className="glass-ghost rounded-lg px-2.5 py-1 font-mono text-[11px] text-[var(--color-text-muted)]">new_cast · <span className="text-[var(--color-text-secondary)]">podcast</span></span>
        <div className="ml-auto flex items-center gap-3">
          <span className="flex items-center gap-1.5 text-[11px] text-[var(--color-text-muted)]"><span className="h-1.5 w-1.5 rounded-full bg-[#20C997] shadow-[0_0_8px_#20C997]" /> ready</span>
          <div className="glass-ghost flex items-center gap-1 rounded-full p-0.5 text-[11px]"><span className="rounded-full bg-[rgba(255,122,24,0.18)] px-2.5 py-0.5 font-semibold text-[#FF9A45]">Pro</span><span className="px-2 text-[var(--color-text-muted)]">Creator</span></div>
          <span className="font-mono text-[11px] text-[var(--color-text-muted)]">00:06:35:21</span>
        </div>
      </div>

      <LeftDock view={view} setView={setView} />

      {/* STAGE LAYER (always rendered; dims when a destination is open) */}
      <div className="absolute inset-0 z-10 flex items-center justify-center gap-7 px-20 pt-16 pb-40 transition-all duration-300"
        style={{ filter: view === "stage" ? "none" : "blur(8px) brightness(0.5)", transform: view === "stage" ? "none" : "scale(0.98)" }}>
        <div className="relative w-full max-w-[820px]">
          <div className="relative aspect-video w-full overflow-hidden rounded-2xl" style={{ background: "radial-gradient(ellipse at 38% 30%, #21384a, #0c151d 70%)", border: "1px solid rgba(255,255,255,0.10)", boxShadow: "0 30px 80px rgba(0,0,0,0.55)" }}>
            <div className="absolute inset-0 grid place-items-center"><div className="text-center"><div className="text-[13px] font-semibold tracking-wide text-[var(--color-text-secondary)]">▶ PREVIEW</div><div className="mt-1 text-[11px] text-[var(--color-text-muted)]">podcast two-shot · 1920×1080</div></div></div>
            <div className="absolute inset-x-0 bottom-0 flex items-center gap-3 px-4 py-3" style={{ background: "linear-gradient(0deg, rgba(0,0,0,0.5), transparent)" }}>
              <button className="glass-ghost grid h-8 w-8 place-items-center rounded-full text-[12px]">▶</button>
              <div className="h-1 flex-1 rounded-full bg-[rgba(255,255,255,0.14)]"><div className="h-full w-[34%] rounded-full" style={{ background: ORANGE, boxShadow: `0 0 10px ${ORANGE}` }} /></div>
              <span className="font-mono text-[10px] text-[var(--color-text-secondary)]">06:35 / 19:12</span>
            </div>
          </div>
          {/* agent read line */}
          <div className="mt-3 flex items-center gap-2 pl-1 text-[11px] text-[var(--color-text-muted)]">
            <span className="text-[#FF9A45]">◇</span> read AGENTS.md · 9 signals · <span className="text-[var(--color-text-secondary)]">{pending.length} proposals ready</span>
          </div>
        </div>

        {pending.length > 0 && (
          <div className="z-20">
            <div className="mb-2 flex items-center gap-2 pl-1"><span className="text-[13px] font-semibold text-[var(--color-text-primary)]">{pending.length} proposals</span><span className="text-[11px] text-[var(--color-text-muted)]">waiting</span></div>
            <ProposalDeck items={pending} active={active} setActive={setActive} onDecide={decide} />
          </div>
        )}
      </div>

      {/* timeline glass strip with proposal markers pinned at timecodes */}
      <div className="absolute inset-x-20 bottom-24 z-20 transition-opacity duration-300" style={{ opacity: view === "stage" ? 1 : 0.25 }}>
        <div className="glass glass-soft relative flex items-center gap-2 rounded-xl px-3 py-2" style={{ borderRadius: 14 }}>
          <span className="font-mono text-[10px] text-[var(--color-text-muted)]">19:12</span>
          <div className="relative h-9 flex-1">
            <div className="flex h-full items-center gap-px overflow-hidden rounded-md">
              {Array.from({ length: 64 }).map((_, i) => (<div key={i} className="h-full flex-1" style={{ background: "rgba(255,255,255,0.06)" }} />))}
            </div>
            {/* proposal markers */}
            {pending.map((p) => (
              <div key={p.id} className="absolute top-0 h-full w-0.5 -translate-x-1/2" style={{ left: `${p.at * 100}%`, background: MEDIUM[p.medium].c, boxShadow: `0 0 8px ${MEDIUM[p.medium].c}` }}>
                <span className="absolute -top-1 left-1/2 h-1.5 w-1.5 -translate-x-1/2 rounded-full" style={{ background: MEDIUM[p.medium].c }} />
              </div>
            ))}
            {/* playhead */}
            <div className="absolute top-0 h-full w-0.5 -translate-x-1/2" style={{ left: "34%", background: ORANGE, boxShadow: `0 0 10px ${ORANGE}` }} />
          </div>
          <span className="font-mono text-[10px] text-[var(--color-text-muted)]">2 tracks ▾</span>
        </div>
      </div>

      {/* destination sheets slide over the dimmed stage */}
      {view !== "stage" && (
        <div className="absolute inset-0 z-30 flex items-stretch px-20 pt-16 pb-40">
          {view === "deliver" && <DeliverSheet onClose={() => setView("stage")} />}
          {view === "skills" && <SkillsSheet onClose={() => setView("stage")} />}
          {view === "history" && <HistorySheet onClose={() => setView("stage")} />}
        </div>
      )}

      {/* command bar — edits AND navigates */}
      <div className="absolute inset-x-0 bottom-0 z-40 flex justify-center px-8 pb-6">
        <div className="glass glass-strong glass-reactive flex w-full max-w-[760px] items-center gap-3 rounded-2xl px-4 py-3" style={{ borderRadius: 18 }}>
          <span className="grid h-7 w-7 shrink-0 place-items-center rounded-lg" style={{ background: "rgba(255,122,24,0.16)", color: "#FF9A45" }}>◇</span>
          <input value={draft} onChange={(e) => setDraft(e.target.value)}
            placeholder="ask, trim, propose…  or type a destination: deliver · skills · history"
            className="min-w-0 flex-1 bg-transparent text-[13px] text-[var(--color-text-primary)] placeholder:text-[var(--color-text-muted)] outline-none" />
          <span className="hidden items-center gap-1 md:flex">
            {(["deliver", "skills", "history"] as View[]).map((s) => (
              <button key={s} onClick={() => setView(s)} className="glass-ghost rounded-lg px-2 py-1 text-[11px]">/{s}</button>
            ))}
          </span>
          <button className="glass-cta grid h-8 w-8 place-items-center rounded-xl text-[13px]">▸</button>
        </div>
      </div>

      <div className="absolute bottom-7 left-20 z-40 font-mono text-[10px] text-[var(--color-text-disabled)]">⌘K command · ⌘↵ accept · the bar edits AND navigates</div>
    </div>
  );
}

ReactDOM.createRoot(document.getElementById("root")!).render(<React.StrictMode><Stage /></React.StrictMode>);
