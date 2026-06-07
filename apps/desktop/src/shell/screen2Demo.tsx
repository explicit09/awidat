import { CheckCircle2, Circle, Scissors, ShieldCheck, Sparkles } from "lucide-react";
import beforeFrame from "./assets/podcast-before.jpg";
import afterFrame from "./assets/podcast-after.jpg";
import wideFrame from "./assets/podcast-wide.jpg";
import type {
  ActivityEntry,
  ContextChip,
  PlanItem,
  PreviewChange,
  SuggestedAction,
} from ".";
import type { ProposalInspectorData } from "./ProposalInspector";

export const SCREEN2_DURATION_S = 492;
export const SCREEN2_CURRENT_TIME_S = 402.46;

export const screen2ContextChips: ContextChip[] = [
  { label: "Clip: Interview_A", kind: "media" },
  { label: "Range: 00:12-18:40", kind: "selection" },
  { label: "Transcript region selected", kind: "selection" },
  { label: "Target: YouTube 16:9", kind: "lens" },
];

export const screen2Plan: PlanItem[] = [
  { id: "index", text: "Index footage", status: "complete" },
  { id: "pauses", text: "Detect pauses & fillers", status: "complete" },
  { id: "assembly", text: "Build assembly (rough cut)", status: "in_progress" },
  { id: "transitions", text: "Review transitions (J/L-cuts)", status: "pending" },
  { id: "audio", text: "Normalize audio", status: "pending" },
];

export const screen2Suggestions: SuggestedAction[] = [
  {
    id: "rough-cut",
    label: "Review rough cut",
    prompt: "Preview current proposal",
  },
  {
    id: "changed-regions",
    label: "Inspect 12 changed regions",
    prompt: "Review edits and reasoning",
  },
  {
    id: "shorts",
    label: "Generate 3 shorts",
    prompt: "Vertical clips for social",
  },
];

export const screen2Activity: ActivityEntry[] = [
  { id: "act1", timestamp: "12:39", text: "Transcript boundaries aligned", kind: "tool" },
  { id: "act2", timestamp: "12:41", text: "Audio energy map updated", kind: "tool" },
  { id: "act3", timestamp: "12:42", text: "12 proposal changes scored", kind: "result" },
];

export const screen2Changes: PreviewChange[] = [
  { id: "c01", index: 1, kind: "accepted", timeS: 52, label: "Open trim accepted" },
  { id: "c02", index: 2, kind: "pending", timeS: 86, label: "Remove 1.3s filler" },
  { id: "c03", index: 3, kind: "pending", timeS: 123, label: "Tighten speaker handoff" },
  { id: "c04", index: 4, kind: "warning", timeS: 171, label: "Pacing risk" },
  { id: "c05", index: 5, kind: "accepted", timeS: 216, label: "Keep natural pause" },
  { id: "c06", index: 6, kind: "rejected", timeS: 265, label: "Rejected hard cut" },
  { id: "c07", index: 7, kind: "selected", timeS: SCREEN2_CURRENT_TIME_S, label: "Cut 07 · J-cut" },
  { id: "c08", index: 8, kind: "pending", timeS: 426, label: "Trim trailing pause" },
  { id: "c09", index: 9, kind: "pending", timeS: 451, label: "Caption boundary" },
  { id: "c10", index: 10, kind: "accepted", timeS: 466, label: "Audio crossfade" },
  { id: "c11", index: 11, kind: "pending", timeS: 478, label: "Transcript repair" },
  { id: "c12", index: 12, kind: "warning", timeS: 486, label: "Check breath cut" },
];

export const screen2AudioPeaks = Array.from({ length: 220 }, (_, i) => {
  const carrier = Math.sin(i * 0.18) * 0.45 + Math.sin(i * 0.61) * 0.24;
  const phrase = i % 37 > 24 ? 0.32 : 0.82;
  return Math.max(0.08, Math.min(1, 0.42 + carrier + phrase * 0.38));
});

export const screen2Inspector: ProposalInspectorData = {
  title: "Cut 07 · J-cut",
  proposalState: "proposed",
  statusLabel: "Pending",
  timeRange: "00:06:35:21 – 00:06:42:05",
  duration: "6.7s",
  cutType: "J-cut (audio leads)",
  intent:
    "Remove filler phrase and trailing pause while preserving speaker cadence. Use a J-cut to lead into the next speaker for a smoother handoff.",
  confidence: 0.88,
  risk: "low",
  evidence: [
    { id: "e1", kind: "transcript", label: "Transcript boundary", confidenceLevel: "high" },
    { id: "e2", kind: "audio_energy", label: "Audio energy drop", confidenceLevel: "high" },
    { id: "e3", kind: "speaker_handoff", label: "Speaker handoff", confidenceLevel: "high" },
    { id: "e4", kind: "visual", label: "Visual continuity", confidenceLevel: "medium" },
  ],
  explanation:
    "The guest finishes a thought, then leaves a short low-energy filler before the host responds. Leading the response audio under the outgoing shot preserves momentum without hiding the handoff.",
  alternatives: [
    { id: "alt1", label: "Keep full pause", detail: "+1.3s" },
    { id: "alt2", label: "Tighter cut", detail: "-0.4s" },
    { id: "alt3", label: "Use cutaway", detail: "B-cam" },
  ],
};

export function Screen2MediaSlot() {
  return (
    <div className="relative h-full w-full overflow-hidden bg-black">
      <img src={wideFrame} alt="" className="absolute inset-0 h-full w-full object-cover opacity-30 blur-[2px] scale-105" />
      <div className="absolute inset-0 bg-[radial-gradient(circle_at_center,transparent_0%,rgba(0,0,0,0.32)_68%,rgba(0,0,0,0.76)_100%)]" />
      <div className="absolute inset-5 grid grid-cols-2 gap-1 rounded-[var(--radius-md)] border border-[var(--color-border-subtle)] bg-black shadow-[0_22px_70px_rgba(0,0,0,0.55)]">
        <Frame label="Before" src={beforeFrame} muted />
        <Frame label="After proposal" src={afterFrame} />
      </div>
      <div className="absolute left-1/2 top-5 bottom-5 w-px -translate-x-1/2 bg-[var(--color-border-focus)] shadow-[0_0_18px_rgba(239,68,68,0.7)]" />
      <div className="absolute left-1/2 top-1/2 grid h-10 w-10 -translate-x-1/2 -translate-y-1/2 place-items-center rounded-full border border-[var(--color-border-focus)] bg-[rgba(8,13,20,0.92)] text-[var(--color-brand-secondary)] shadow-[0_0_24px_rgba(239,68,68,0.3)]">
        <Scissors className="h-4 w-4 stroke-[1.7]" />
      </div>
      <div className="absolute left-8 top-8 flex items-center gap-2 rounded-[var(--radius-sm)] border border-[rgba(245,158,11,0.45)] bg-[rgba(58,38,5,0.76)] px-2 py-1 text-[var(--text-caption)] font-semibold uppercase tracking-[var(--text-label--letter-spacing)] text-[var(--color-job-idle-text)]">
        <Sparkles className="h-3.5 w-3.5" />
        Active proposal overlay
      </div>
      <div className="absolute right-8 bottom-8 grid grid-cols-3 gap-1.5 rounded-[var(--radius-md)] border border-[var(--color-border-subtle)] bg-[rgba(8,13,20,0.86)] p-2 backdrop-blur">
        {[
          ["Cadence", "88"],
          ["Risk", "Low"],
          ["Delta", "-06.7s"],
        ].map(([label, value]) => (
          <div key={label} className="min-w-16">
            <div className="text-[9px] uppercase tracking-[0.08em] text-[var(--color-text-muted)]">{label}</div>
            <div className="font-mono text-[var(--text-caption)] text-[var(--color-text-primary)]">{value}</div>
          </div>
        ))}
      </div>
    </div>
  );
}

function Frame({ label, src, muted = false }: { label: string; src: string; muted?: boolean }) {
  return (
    <div className="relative min-w-0 overflow-hidden">
      <img src={src} alt="" className="h-full w-full object-cover" />
      {muted ? <div className="absolute inset-0 bg-[rgba(0,0,0,0.22)]" /> : null}
      <div className="absolute left-3 top-3 rounded-[var(--radius-xs)] border border-white/10 bg-black/60 px-2 py-1 text-[10px] font-semibold uppercase tracking-[0.08em] text-white/82">
        {label}
      </div>
      <div className="absolute bottom-3 left-3 flex items-center gap-1.5 text-[10px] font-medium text-white/80">
        {muted ? <Circle className="h-3 w-3" /> : <CheckCircle2 className="h-3 w-3 text-[var(--color-success)]" />}
        {muted ? "Source timing" : "Proposed cut 07"}
      </div>
      {!muted ? (
        <div className="absolute right-3 bottom-3 flex items-center gap-1 rounded-[var(--radius-xs)] border border-[rgba(34,197,94,0.35)] bg-[rgba(11,47,26,0.72)] px-2 py-1 text-[10px] font-semibold uppercase tracking-[0.08em] text-[var(--color-proposal-accepted-text)]">
          <ShieldCheck className="h-3 w-3" />
          Review safe
        </div>
      ) : null}
    </div>
  );
}
