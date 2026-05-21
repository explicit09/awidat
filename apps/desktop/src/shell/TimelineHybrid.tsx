import { Filter, MoreHorizontal } from "lucide-react";
import type { ReactNode } from "react";
import { Inline, Pill, Stack, cn, type PillStatus } from "../ui";

/**
 * TimelineHybrid — the bottom surface from the design spec (Screen 2 + Screen 4).
 *
 * Sub-tabs (own state): Timeline / Transcript / Selects / Changes / Evidence
 * View toggle (own state): Proposed Timeline / Current Timeline
 *
 * Lanes (rendered for the Timeline sub-tab):
 *   - VIDEO       : thumbnail strip (placeholder until 2.11 wires real frames)
 *   - AUDIO       : waveform strip
 *   - AGENT EDITS : per-cut markers with status colors
 *   - TRANSCRIPT  : word/sentence cards
 *   - DIFF        : kept / trimmed / removed chips beneath the cut points
 */

export const TIMELINE_TABS = ["timeline", "transcript", "selects", "changes", "evidence"] as const;
export type TimelineTab = (typeof TIMELINE_TABS)[number];

export const TIMELINE_TAB_LABEL: Record<TimelineTab, string> = {
  timeline: "Timeline",
  transcript: "Transcript",
  selects: "Selects",
  changes: "Changes",
  evidence: "Evidence",
};

export type TimelineViewMode = "proposed" | "current";

export type AgentEdit = {
  id: string;
  startS: number;
  endS: number;
  /** Maps to the same Pill statuses we use elsewhere. */
  status: PillStatus;
  label?: string;
};

export type TranscriptCell = {
  id: string;
  startS: number;
  endS: number;
  text: string;
  speakerColor?: string;
};

export type DiffChip = {
  id: string;
  startS: number;
  endS: number;
  kind: "keep" | "trim" | "remove";
  label?: string;
};

export type TimelineHybridProps = {
  tab?: TimelineTab;
  onChangeTab?: (t: TimelineTab) => void;
  viewMode?: TimelineViewMode;
  onChangeViewMode?: (m: TimelineViewMode) => void;
  durationS?: number;
  /** Source frames for the VIDEO lane (placeholder strip if absent). */
  videoFrames?: string[];
  /** Audio waveform peaks (0..1 normalized) for the AUDIO lane. */
  audioPeaks?: number[];
  agentEdits?: AgentEdit[];
  transcript?: TranscriptCell[];
  diff?: DiffChip[];
  currentTimeS?: number;
  /** Per-cut changes counter shown in the "Changes" tab pill (e.g. 12). */
  changeCount?: number;
  /** Allows the parent to slot custom content per tab (overrides built-in lane render). */
  contentForTab?: Partial<Record<TimelineTab, ReactNode>>;
};

export function TimelineHybrid({
  tab = "timeline",
  onChangeTab,
  viewMode = "proposed",
  onChangeViewMode,
  durationS = 0,
  videoFrames = [],
  audioPeaks = [],
  agentEdits = [],
  transcript = [],
  diff = [],
  currentTimeS = 0,
  changeCount = 0,
  contentForTab,
}: TimelineHybridProps) {
  return (
    <Stack gap="0" className="h-full w-full bg-[var(--color-surface-panel)]">
      {/* Header: tabs + view toggle + actions */}
      <div className="px-3 h-10 border-b border-[var(--color-border-subtle)] grid grid-cols-[1fr_auto_auto] items-center gap-3 shrink-0">
        <Inline gap="0" align="center" className="h-full">
          {TIMELINE_TABS.map((t) => (
            <button
              key={t}
              type="button"
              role="tab"
              aria-selected={t === tab}
              onClick={() => onChangeTab?.(t)}
              className={cn(
                "relative inline-flex items-center gap-1.5 h-full px-3",
                "text-[var(--text-body-sm)] font-medium",
                "transition-[color,background-color] duration-[120ms]",
                t === tab
                  ? "text-[var(--color-text-primary)]"
                  : "text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)]",
              )}
            >
              <span>{TIMELINE_TAB_LABEL[t]}</span>
              {t === "changes" && changeCount > 0 ? (
                <Pill status="warning" dot={false}>{changeCount}</Pill>
              ) : null}
              {t === tab ? (
                <span
                  aria-hidden
                  className="absolute inset-x-3 bottom-0 h-0.5 rounded-full bg-[var(--color-brand)]"
                />
              ) : null}
            </button>
          ))}
        </Inline>
        <Inline gap="2" align="center">
          <span className="text-[var(--text-caption)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-muted)]">
            View
          </span>
          <ViewToggle value={viewMode} onChange={onChangeViewMode} />
        </Inline>
        <Inline gap="0" align="center">
          <button
            type="button"
            className="h-7 w-7 inline-flex items-center justify-center rounded-[var(--radius-sm)] text-[var(--color-text-muted)] hover:bg-[var(--color-surface-hover)] hover:text-[var(--color-text-primary)] transition-colors"
            aria-label="Filter"
          >
            <Filter className="h-3.5 w-3.5 stroke-[1.75]" />
          </button>
          <button
            type="button"
            className="h-7 w-7 inline-flex items-center justify-center rounded-[var(--radius-sm)] text-[var(--color-text-muted)] hover:bg-[var(--color-surface-hover)] hover:text-[var(--color-text-primary)] transition-colors"
            aria-label="More"
          >
            <MoreHorizontal className="h-3.5 w-3.5 stroke-[1.75]" />
          </button>
        </Inline>
      </div>

      {/* Tab body */}
      <div className="flex-1 min-h-0 min-w-0 overflow-hidden">
        {contentForTab?.[tab] ?? (
          <DefaultTabBody
            tab={tab}
            durationS={durationS}
            videoFrames={videoFrames}
            audioPeaks={audioPeaks}
            agentEdits={agentEdits}
            transcript={transcript}
            diff={diff}
            currentTimeS={currentTimeS}
          />
        )}
      </div>

      {/* Legend */}
      <div className="px-3 h-9 border-t border-[var(--color-border-subtle)] flex items-center justify-between shrink-0">
        <Inline gap="3" align="center">
          <Legend />
        </Inline>
        <span className="font-mono text-[var(--text-caption)] text-[var(--color-text-muted)]">
          {viewMode === "proposed" ? "Proposed Timeline" : "Current Timeline"}
        </span>
      </div>
    </Stack>
  );
}

function ViewToggle({
  value,
  onChange,
}: {
  value: TimelineViewMode;
  onChange?: (m: TimelineViewMode) => void;
}) {
  return (
    <div className="inline-flex items-center rounded-[var(--radius-sm)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-card)] p-0.5">
      {(
        [
          { key: "proposed", label: "Proposed Timeline" },
          { key: "current", label: "Current Timeline" },
        ] as const
      ).map((opt) => (
        <button
          key={opt.key}
          type="button"
          onClick={() => onChange?.(opt.key)}
          aria-pressed={value === opt.key}
          className={cn(
            "h-6 px-2.5 rounded-[var(--radius-xs)] text-[var(--text-caption)] font-medium transition-colors",
            value === opt.key
              ? "bg-[var(--color-surface-selected)] text-[var(--color-text-primary)]"
              : "text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)]",
          )}
        >
          {opt.label}
        </button>
      ))}
    </div>
  );
}

function Legend() {
  const items = [
    { color: "var(--color-success)", label: "Accepted" },
    { color: "var(--color-warning)", label: "Pending" },
    { color: "var(--color-danger)", label: "Removed" },
    { color: "var(--color-risk)", label: "Warning" },
  ];
  return (
    <>
      <span className="text-[var(--text-caption)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-muted)]">
        Legend
      </span>
      {items.map((it) => (
        <Inline key={it.label} gap="1" align="center">
          <span className="h-2 w-2 rounded-full" style={{ backgroundColor: it.color }} aria-hidden />
          <span className="text-[var(--text-caption)] text-[var(--color-text-secondary)]">{it.label}</span>
        </Inline>
      ))}
    </>
  );
}

type DefaultTabBodyProps = {
  tab: TimelineTab;
  durationS: number;
  videoFrames: string[];
  audioPeaks: number[];
  agentEdits: AgentEdit[];
  transcript: TranscriptCell[];
  diff: DiffChip[];
  currentTimeS: number;
};

function DefaultTabBody({
  tab,
  durationS,
  videoFrames,
  audioPeaks,
  agentEdits,
  transcript,
  diff,
  currentTimeS,
}: DefaultTabBodyProps) {
  if (tab === "timeline") {
    return (
      <TimelineLanes
        durationS={durationS}
        videoFrames={videoFrames}
        audioPeaks={audioPeaks}
        agentEdits={agentEdits}
        transcript={transcript}
        diff={diff}
        currentTimeS={currentTimeS}
      />
    );
  }
  if (tab === "transcript") {
    return (
      <Stack gap="2" className="p-4 overflow-y-auto h-full">
        <span className="text-[var(--text-caption)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-muted)]">
          Transcript — semantic edit surface
        </span>
        <span className="text-[var(--text-body-sm)] text-[var(--color-text-secondary)]">
          Sentence-level transcript editing lands in Phase 3.2 (Review lens semantic affordances).
        </span>
      </Stack>
    );
  }
  return (
    <div className="flex h-full items-center justify-center">
      <span className="text-[var(--text-caption)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-muted)]">
        {TIMELINE_TAB_LABEL[tab]} — populated in Phase 3
      </span>
    </div>
  );
}

function TimelineLanes({
  durationS,
  videoFrames,
  audioPeaks,
  agentEdits,
  transcript,
  diff,
  currentTimeS,
}: Omit<DefaultTabBodyProps, "tab">) {
  const safeDuration = durationS > 0 ? durationS : 1;
  const playheadPct = Math.max(0, Math.min(100, (currentTimeS / safeDuration) * 100));

  return (
    <div className="relative h-full overflow-auto">
      <div className="min-w-full">
        <Lane label="VIDEO" rowHeight={36}>
          <VideoStrip frames={videoFrames} />
        </Lane>
        <Lane label="AUDIO" rowHeight={48}>
          <AudioWaveform peaks={audioPeaks} />
        </Lane>
        <Lane label="AGENT EDITS" rowHeight={36}>
          <AgentEditsLane edits={agentEdits} durationS={safeDuration} />
        </Lane>
        <Lane label="TRANSCRIPT" rowHeight={48}>
          <TranscriptStrip cells={transcript} durationS={safeDuration} />
        </Lane>
        <Lane label="DIFF" rowHeight={36} last>
          <DiffStrip diff={diff} durationS={safeDuration} />
        </Lane>
      </div>
      {/* Global playhead overlay */}
      <div
        aria-hidden
        className="pointer-events-none absolute inset-y-0 w-0.5 -translate-x-1/2 bg-[var(--color-viz-playhead)]"
        style={{
          left: `calc(80px + ${playheadPct}% * (1 - 80px / 100%))`,
          boxShadow: "0 0 8px var(--color-viz-playhead)",
        }}
      />
    </div>
  );
}

function Lane({
  label,
  rowHeight,
  last = false,
  children,
}: {
  label: string;
  rowHeight: number;
  last?: boolean;
  children: ReactNode;
}) {
  return (
    <div className={cn("grid grid-cols-[80px_1fr] items-stretch", !last && "border-b border-[var(--color-border-subtle)]")}>
      <div
        className="flex items-center justify-end pr-3 border-r border-[var(--color-border-subtle)] bg-[var(--color-surface-panel)] sticky left-0"
        style={{ height: rowHeight }}
      >
        <span className="text-[var(--text-micro)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-muted)]">
          {label}
        </span>
      </div>
      <div className="relative" style={{ height: rowHeight }}>
        {children}
      </div>
    </div>
  );
}

function VideoStrip({ frames }: { frames: string[] }) {
  if (frames.length === 0) {
    // 16 placeholder slats with subtle gradient — looks like a thumbnail strip.
    return (
      <div className="absolute inset-0 flex gap-px p-1">
        {Array.from({ length: 16 }).map((_, i) => (
          <div
            key={i}
            className="flex-1 rounded-[2px]"
            style={{
              background: `linear-gradient(180deg, hsl(210 ${10 + (i * 3) % 20}% ${12 + (i * 3) % 8}%) 0%, #0B0F14 100%)`,
            }}
          />
        ))}
      </div>
    );
  }
  return (
    <div className="absolute inset-0 flex gap-px p-1">
      {frames.map((src, i) => (
        <img
          key={i}
          src={src}
          alt=""
          className="flex-1 object-cover rounded-[2px] min-w-0"
        />
      ))}
    </div>
  );
}

function AudioWaveform({ peaks }: { peaks: number[] }) {
  const data =
    peaks.length > 0
      ? peaks
      : Array.from({ length: 240 }).map((_, i) => {
          // Synthetic but plausible-looking waveform.
          const t = i / 240;
          const env = Math.sin(t * Math.PI * 3) * 0.5 + 0.5;
          const noise = Math.sin(i * 0.83) * 0.3 + Math.sin(i * 1.7) * 0.2;
          return Math.max(0.06, Math.min(1, env * 0.85 + noise * 0.35));
        });
  return (
    <svg
      className="absolute inset-0 w-full h-full"
      viewBox={`0 0 ${data.length} 100`}
      preserveAspectRatio="none"
      aria-hidden
    >
      {data.map((v, i) => {
        const h = v * 80;
        return (
          <rect
            key={i}
            x={i}
            y={50 - h / 2}
            width={0.85}
            height={h}
            fill="var(--color-viz-audio)"
          />
        );
      })}
    </svg>
  );
}

const STATUS_TO_VIZ: Partial<Record<PillStatus, string>> = {
  accepted: "var(--color-viz-accepted)",
  pending: "var(--color-viz-pending)",
  rejected: "var(--color-viz-rejected)",
  warning: "var(--color-viz-warning)",
  reviewing: "var(--color-brand-purple)",
};

function AgentEditsLane({ edits, durationS }: { edits: AgentEdit[]; durationS: number }) {
  return (
    <div className="absolute inset-0 p-1">
      {edits.map((e) => {
        const left = (e.startS / durationS) * 100;
        const width = Math.max(0.4, ((e.endS - e.startS) / durationS) * 100);
        const color = STATUS_TO_VIZ[e.status] ?? "var(--color-text-muted)";
        return (
          <div
            key={e.id}
            className="absolute top-1 bottom-1 rounded-[var(--radius-xs)] border opacity-90"
            style={{
              left: `${left}%`,
              width: `${width}%`,
              backgroundColor: color,
              borderColor: color,
              filter: "brightness(1.05)",
            }}
            title={e.label}
          />
        );
      })}
    </div>
  );
}

function TranscriptStrip({ cells, durationS }: { cells: TranscriptCell[]; durationS: number }) {
  return (
    <div className="absolute inset-0">
      {cells.map((c) => {
        const left = (c.startS / durationS) * 100;
        const width = Math.max(2, ((c.endS - c.startS) / durationS) * 100);
        return (
          <div
            key={c.id}
            className="absolute top-1 bottom-1 px-1.5 py-1 rounded-[var(--radius-xs)] bg-[var(--color-surface-card)] border border-[var(--color-border-subtle)] text-[var(--text-caption)] text-[var(--color-text-secondary)] truncate"
            style={{
              left: `${left}%`,
              width: `${width}%`,
              borderTopColor: c.speakerColor ?? "var(--color-viz-speaker-a)",
              borderTopWidth: 2,
            }}
            title={c.text}
          >
            {c.text}
          </div>
        );
      })}
    </div>
  );
}

function DiffStrip({ diff, durationS }: { diff: DiffChip[]; durationS: number }) {
  return (
    <div className="absolute inset-0 p-1">
      {diff.map((d) => {
        const left = (d.startS / durationS) * 100;
        const width = Math.max(0.6, ((d.endS - d.startS) / durationS) * 100);
        const color =
          d.kind === "keep"
            ? "var(--color-viz-accepted)"
            : d.kind === "trim"
              ? "var(--color-viz-pending)"
              : "var(--color-viz-rejected)";
        return (
          <div
            key={d.id}
            className="absolute top-1 bottom-1 rounded-[var(--radius-xs)] flex items-center justify-center text-[var(--text-micro)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[#070A0F]"
            style={{
              left: `${left}%`,
              width: `${width}%`,
              backgroundColor: color,
            }}
            title={d.label ?? d.kind}
          >
            {width > 5 ? (d.label ?? d.kind) : null}
          </div>
        );
      })}
    </div>
  );
}
