// Stage insights — the two-panel block under the program monitor.
//
//   ┌─ AI suggestions ───────────────────┐ ┌─ review queue ─────────┐
//   │ [Trim silences: 5 over 3s, −27s]   │ │ 00:45 Silence (4.2s)   │
//   │ [Remove fillers: 12 found, −18s]   │ │ 03:12 Filler “um”      │
//   │ [starter prompts as cards…]        │ │ …click → seek          │
//   └────────────────────────────────────┘ └────────────────────────┘
//
// Every number is computed from data the app already has (whisper
// words, silence sidecars, proposal changes) — cards with no backing
// data simply don't render. The right queue shows the pending
// proposal's cuts when one exists, otherwise the raw detections;
// detections that are no longer on the timeline (already cut) are
// dropped via timelineTimeForSourceTime.

import { useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  ChevronRight,
  Film,
  Quote,
  Scissors,
  Sparkles,
  Timer,
  Wand2,
  type LucideIcon,
} from "lucide-react";
import { cn } from "../ui";
import { useStarterPrompts } from "../agent/useStarterPrompts";
import { useProjectStore } from "../app/state";
import { useMediaStore } from "../media/store";
import { useTranscriptStore } from "../transcript/store";
import { usePlaySegments } from "../timeline/usePlaySegments";
import { onTimelineSpan, timelineTimeForSourceTime } from "../timeline/sourceTimeMap";
import {
  detectFillerMoments,
  detectSilenceMoments,
  estimatedSavingsS,
  type DetectedMoment,
} from "./insights";
export type PreviewChange = {
  id: string;
  index: number;
  kind: "pending";
  timeS: number;
  label?: string;
};

const SILENCE_MIN_S = 2;
const QUEUE_CAP = 9;

type Silence = { start_s: number; end_s: number; duration_s: number };

export function PreviewInsights({
  changes,
  activeChangeId,
  onSelectChange,
  onAcceptProposal,
  onRejectProposal,
}: {
  changes: PreviewChange[];
  activeChangeId?: string;
  onSelectChange?: (change: PreviewChange) => void;
  onAcceptProposal?: () => void;
  onRejectProposal?: () => void;
}) {
  const { prompts, running, hasProject, send } = useStarterPrompts();
  const projectRoot = useProjectStore((s) => s.current);
  const requestTimelineSeek = useMediaStore((s) => s.requestTimelineSeek);
  const segments = usePlaySegments();
  const stems = useMemo(
    () => [...new Set(segments.map((s) => s.proxyStem))],
    [segments],
  );

  // Transcripts come through the shared store cache; silences are
  // fetched once per stem here (cheap sidecar reads).
  const transcriptsByStem = useTranscriptStore((s) => s.byStem);
  const loadTranscript = useTranscriptStore((s) => s.load);
  useEffect(() => {
    stems.forEach((stem) => void loadTranscript(stem));
  }, [stems, loadTranscript]);
  const [silencesByStem, setSilencesByStem] = useState<Record<string, Silence[]>>({});
  useEffect(() => {
    if (!projectRoot) return;
    let cancelled = false;
    for (const stem of stems) {
      const cacheKey = silenceCacheKey(projectRoot, stem);
      if (silencesByStem[cacheKey]) continue;
      invoke<Silence[]>("read_silences", { projectPath: projectRoot, stem })
        .then((ranges) => {
          if (!cancelled) setSilencesByStem((prev) => ({ ...prev, [cacheKey]: ranges }));
        })
        .catch(() => {
          if (!cancelled) setSilencesByStem((prev) => ({ ...prev, [cacheKey]: [] }));
        });
    }
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [projectRoot, stems]);

  // Detections, restricted to material still on the timeline.
  const detected = useMemo(() => {
    const moments: (DetectedMoment & { timelineTimeS: number })[] = [];
    for (const stem of stems) {
      const transcript = transcriptsByStem[stem];
      const words =
        transcript?.state === "loaded" ? transcript.transcript.words : [];
      const raw = [
        ...detectSilenceMoments(
          silencesByStem[silenceCacheKey(projectRoot ?? "", stem)] ?? [],
          stem,
          SILENCE_MIN_S,
        ),
        ...detectFillerMoments(words, stem),
      ];
      for (const m of raw) {
        // Re-measure against the timeline: an edit that cut INTO the
        // moment shrinks (or removes) it, so threshold on what's left.
        const span = onTimelineSpan(
          segments,
          m.stem,
          m.sourceTimeS,
          m.sourceTimeS + m.durationS,
        );
        if (!span) continue;
        if (m.kind === "silence" && span.overlapS < SILENCE_MIN_S) continue;
        if (m.kind === "filler" && span.overlapS < m.durationS * 0.6) continue;
        const t = timelineTimeForSourceTime(segments, m.stem, span.firstSourceS);
        if (t === null) continue;
        moments.push({
          ...m,
          durationS: span.overlapS,
          label:
            m.kind === "silence" ? `Silence (${span.overlapS.toFixed(1)}s)` : m.label,
          timelineTimeS: t,
        });
      }
    }
    return moments.sort((a, b) => a.timelineTimeS - b.timelineTimeS);
  }, [stems, transcriptsByStem, silencesByStem, segments]);

  const silenceMoments = detected.filter((m) => m.kind === "silence");
  const fillerMoments = detected.filter((m) => m.kind === "filler");

  // Suggestion cards — data-backed first, then the starter prompts
  // (minus the ones the data cards already cover).
  const cards: SuggestionCard[] = [];
  if (fillerMoments.length > 0) {
    cards.push({
      title: "Remove filler words",
      lines: [
        `${fillerMoments.length} filler words found`,
        `Estimated savings: ${formatSeconds(estimatedSavingsS(fillerMoments))}`,
      ],
      action: "Review",
      icon: Scissors,
      tint: "#34d399",
      prompt:
        "Remove filler words and verbal tics (um, uh, you know, i mean) across the episode. Judge every instance in context before cutting: only remove it when it is a meaningless tic — keep phrases that carry meaning (e.g. a real question like 'do you know…', or emphasis the sentence needs). Keep the speech natural; when in doubt, leave it in.",
    });
  }
  if (silenceMoments.length > 0) {
    cards.push({
      title: "Trim long silences",
      lines: [
        `${silenceMoments.length} silences over ${SILENCE_MIN_S}s`,
        `Estimated savings: ${formatSeconds(estimatedSavingsS(silenceMoments))}`,
      ],
      action: "Review",
      icon: Timer,
      tint: "#e8c040",
      prompt: `Trim silences longer than ${SILENCE_MIN_S} seconds across the episode, keeping natural pauses.`,
    });
  }
  for (const p of prompts) {
    if (cards.length >= 6) break;
    if (/filler|silence/i.test(p) && cards.length > 0) continue;
    cards.push({
      title: p,
      lines: [],
      action: "Ask Montage",
      ...starterCardLook(p),
      prompt: p,
    });
  }

  const queueRows: QueueRow[] =
    changes.length > 0
      ? changes.map((c) => ({
          key: c.id,
          timeS: c.timeS,
          label: c.label ?? `Change ${c.index}`,
          detail: c.kind,
          icon: Scissors,
          tint: "#ef4444",
          active: c.id === activeChangeId,
          onClick: () => onSelectChange?.(c),
        }))
      : detected.slice(0, QUEUE_CAP).map((m, i) => ({
          key: `${m.stem}:${m.kind}:${i}:${m.sourceTimeS}`,
          timeS: m.timelineTimeS,
          label: m.label,
          detail: m.detail,
          icon: m.kind === "silence" ? Timer : Quote,
          tint: m.kind === "silence" ? "#34d399" : "#a78bfa",
          active: false,
          onClick: () => requestTimelineSeek(m.timelineTimeS),
        }));
  const queueTitle =
    changes.length > 0
      ? `Review queue · ${changes.length}`
      : `Detected · ${detected.length}`;
  const queueOverflow = changes.length === 0 ? detected.length - QUEUE_CAP : 0;

  if (cards.length === 0 && queueRows.length === 0) return null;

  return (
    <div className="grid min-h-0 flex-1 grid-cols-[minmax(0,1fr)_minmax(260px,340px)] gap-3 overflow-hidden">
      <section className="flex min-h-0 flex-col gap-2.5 overflow-hidden rounded-[14px] border border-[var(--color-border-subtle)] bg-[rgba(255,255,255,0.022)] p-3">
        <header className="flex shrink-0 items-center">
          <span className="inline-flex h-7 items-center rounded-full border border-[rgba(239,68,68,0.4)] bg-[rgba(239,68,68,0.12)] px-3 text-[11.5px] font-bold text-[var(--color-text-primary)]">
            AI Suggestions ({cards.length})
          </span>
        </header>
        <div className="grid auto-rows-min grid-cols-[repeat(auto-fill,minmax(200px,1fr))] gap-2.5 overflow-y-auto">
          {cards.map((card) => (
            <div
              key={card.title}
              className="flex flex-col gap-1.5 rounded-[12px] border border-[var(--color-border-subtle)] bg-[rgba(255,255,255,0.035)] p-3"
            >
              <div className="flex items-center gap-2.5">
                <IconTile icon={card.icon} tint={card.tint} />
                <span className="text-[12.5px] font-bold leading-snug text-[var(--color-text-primary)]">
                  {card.title}
                </span>
              </div>
              {card.lines.map((line) => (
                <span key={line} className="text-[11px] leading-snug text-[var(--color-text-muted)]">
                  {line}
                </span>
              ))}
              <button
                type="button"
                onClick={() => void send(card.prompt)}
                disabled={running || !hasProject}
                className="mt-auto h-7 w-full rounded-[8px] border border-[var(--color-border)] bg-[rgba(255,255,255,0.04)] pt-px text-[11px] font-bold text-[var(--color-text-secondary)] transition-colors hover:border-[rgba(239,68,68,0.45)] hover:bg-[rgba(239,68,68,0.08)] hover:text-[var(--color-text-primary)] disabled:cursor-not-allowed disabled:opacity-45"
              >
                {card.action}
              </button>
            </div>
          ))}
        </div>
      </section>

      <section className="flex min-h-0 flex-col gap-2 overflow-hidden rounded-[14px] border border-[var(--color-border-subtle)] bg-[rgba(255,255,255,0.022)] p-3">
        <header className="flex h-7 shrink-0 items-center">
          <span className="text-[12.5px] font-bold text-[var(--color-text-primary)]">
            {queueTitle}
          </span>
          {changes.length > 0 ? (
            <span className="ml-auto flex gap-1.5">
              <HeaderAction label="Accept all" tone="gold" onClick={onAcceptProposal} />
              <HeaderAction label="Reject all" onClick={onRejectProposal} />
            </span>
          ) : null}
        </header>
        <div className="flex min-h-0 flex-col gap-1.5 overflow-y-auto">
          {queueRows.length === 0 ? (
            <span className="px-1 text-[11.5px] text-[var(--color-text-muted)]">
              Nothing detected yet — indexing may still be running.
            </span>
          ) : (
            queueRows.map((row) => (
              <button
                key={row.key}
                type="button"
                onClick={row.onClick}
                className={cn(
                  "flex shrink-0 items-center gap-2.5 rounded-[10px] border px-2.5 py-1.5 text-left transition-colors",
                  row.active
                    ? "border-[rgba(217,165,75,0.55)] bg-[rgba(217,165,75,0.1)]"
                    : "border-transparent bg-[rgba(255,255,255,0.03)] hover:border-[rgba(217,165,75,0.4)] hover:bg-[rgba(217,165,75,0.06)]",
                )}
              >
                <IconTile icon={row.icon} tint={row.tint} size="sm" />
                <span className="font-mono text-[10.5px] font-semibold text-[#e8c040]">
                  {formatQueueTime(row.timeS)}
                </span>
                <span className="truncate text-[12px] font-bold text-[var(--color-text-primary)]">
                  {row.label}
                </span>
                <span className="ml-auto truncate pl-1 text-[10.5px] text-[var(--color-text-muted)]">
                  {row.detail}
                </span>
                <ChevronRight className="h-3.5 w-3.5 shrink-0 stroke-[2] text-[var(--color-text-muted)]" />
              </button>
            ))
          )}
          {queueOverflow > 0 ? (
            <span className="px-1 pt-0.5 text-[10.5px] text-[var(--color-text-muted)]">
              +{queueOverflow} more on the timeline
            </span>
          ) : null}
        </div>
      </section>
    </div>
  );
}

function silenceCacheKey(projectRoot: string, stem: string): string {
  return `${projectRoot}\u0000${stem}`;
}

function IconTile({
  icon: Icon,
  tint,
  size,
}: {
  icon: LucideIcon;
  tint: string;
  size?: "sm";
}) {
  return (
    <span
      className={cn(
        "grid shrink-0 place-items-center rounded-[7px]",
        size === "sm" ? "h-5.5 w-5.5 rounded-[6px]" : "h-7 w-7",
      )}
      style={{ backgroundColor: `${tint}26`, color: tint }}
      aria-hidden
    >
      <Icon className={size === "sm" ? "h-3 w-3 stroke-[2.25]" : "h-3.5 w-3.5 stroke-[2.25]"} />
    </span>
  );
}

/** Icon + tint for the prompt-only starter cards, keyed off the
 *  prompt text so podcast/interview/highlight sets all land sane. */
function starterCardLook(prompt: string): { icon: LucideIcon; tint: string } {
  const p = prompt.toLowerCase();
  if (/highlight|teaser|short|moment/.test(p)) return { icon: Film, tint: "#60a5fa" };
  if (/punchline|quote/.test(p)) return { icon: Quote, tint: "#a78bfa" };
  if (/default|cleanup/.test(p)) return { icon: Wand2, tint: "#fb923c" };
  return { icon: Sparkles, tint: "#a78bfa" };
}

type SuggestionCard = {
  title: string;
  lines: string[];
  action: string;
  icon: LucideIcon;
  tint: string;
  prompt: string;
};

type QueueRow = {
  key: string;
  timeS: number;
  label: string;
  detail: string;
  icon: LucideIcon;
  tint: string;
  active: boolean;
  onClick: () => void;
};

function HeaderAction({
  label,
  tone,
  onClick,
}: {
  label: string;
  tone?: "gold";
  onClick?: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      disabled={!onClick}
      className={cn(
        "h-6 rounded-md border px-2.5 text-[11px] font-bold transition-colors disabled:cursor-not-allowed disabled:opacity-40",
        tone === "gold"
          ? "border-[rgba(217,165,75,0.4)] bg-[rgba(217,165,75,0.1)] text-[#e8c040] hover:bg-[rgba(217,165,75,0.18)]"
          : "border-[var(--color-border-subtle)] bg-transparent text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)]",
      )}
    >
      {label}
    </button>
  );
}

function formatSeconds(totalSeconds: number): string {
  if (totalSeconds >= 60) {
    const m = Math.floor(totalSeconds / 60);
    const s = Math.round(totalSeconds % 60);
    return `${m}m ${s}s`;
  }
  return `${Math.round(totalSeconds)}s`;
}

function formatQueueTime(totalSeconds: number): string {
  if (!Number.isFinite(totalSeconds) || totalSeconds < 0) return "0:00";
  const h = Math.floor(totalSeconds / 3600);
  const m = Math.floor((totalSeconds % 3600) / 60);
  const s = Math.floor(totalSeconds % 60);
  return h > 0
    ? `${h}:${String(m).padStart(2, "0")}:${String(s).padStart(2, "0")}`
    : `${m}:${String(s).padStart(2, "0")}`;
}
