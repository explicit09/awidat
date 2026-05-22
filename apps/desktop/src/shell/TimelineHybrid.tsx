import { Filter, MoreHorizontal } from "lucide-react";
import type { ReactNode } from "react";
import {
  Inline,
  Pill,
  Stack,
  cn,
} from "../ui";

/**
 * TimelineHybrid — the bottom surface from the design spec (Screen 2 + Screen 4).
 *
 * Sub-tabs (own state): Timeline / Changes / Evidence
 * View toggle (own state): Proposed Timeline / Current Timeline
 *
 * Lanes (rendered for the Timeline sub-tab):
 *   - AUDIO       : waveform strip
 */

export const TIMELINE_TABS = ["timeline", "changes", "evidence"] as const;
export type TimelineTab = (typeof TIMELINE_TABS)[number];

export const TIMELINE_TAB_LABEL: Record<TimelineTab, string> = {
  timeline: "Timeline",
  changes: "Changes",
  evidence: "Evidence",
};

export type TimelineViewMode = "proposed" | "current";

export type TimelineHybridProps = {
  tab?: TimelineTab;
  onChangeTab?: (t: TimelineTab) => void;
  viewMode?: TimelineViewMode;
  onChangeViewMode?: (m: TimelineViewMode) => void;
  durationS?: number;
  /** Audio waveform peaks (0..1 normalized) for the AUDIO lane. */
  audioPeaks?: number[];
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
  audioPeaks = [],
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
                  className="absolute inset-x-3 bottom-0 h-0.5 rounded-full bg-[var(--color-brand-secondary)]"
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
            audioPeaks={audioPeaks}
            currentTimeS={currentTimeS}
            changeCount={changeCount}
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
    { color: "var(--color-viz-accepted)", label: "Accepted" },
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
  audioPeaks: number[];
  currentTimeS: number;
  changeCount: number;
};

function DefaultTabBody({
  tab,
  durationS,
  audioPeaks,
  currentTimeS,
  changeCount,
}: DefaultTabBodyProps) {
  if (tab === "timeline") {
    return (
      <TimelineLanes
        durationS={durationS}
        audioPeaks={audioPeaks}
        currentTimeS={currentTimeS}
      />
    );
  }
  if (tab === "changes") return <CompactEmpty label="Changes" detail={changeCount > 0 ? `${changeCount} proposal changes available in the preview and inspector.` : "No proposal changes yet."} />;
  if (tab === "evidence") return <CompactEmpty label="Evidence" detail="Evidence lives with the selected proposal in the inspector." />;
  return (
    <div className="flex h-full items-center justify-center">
      <span className="text-[var(--text-caption)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-muted)]">
        {TIMELINE_TAB_LABEL[tab]} — populated in Phase 3
      </span>
    </div>
  );
}

function CompactEmpty({ label, detail }: { label: string; detail: string }) {
  return (
    <div className="flex h-full items-center justify-center px-4 text-center">
      <Stack gap="1" align="center">
        <span className="text-[var(--text-caption)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-muted)]">
          {label}
        </span>
        <span className="max-w-xl text-[var(--text-body-sm)] text-[var(--color-text-secondary)]">
          {detail}
        </span>
      </Stack>
    </div>
  );
}

function TimelineLanes({
  durationS,
  audioPeaks,
  currentTimeS,
}: Pick<DefaultTabBodyProps, "durationS" | "audioPeaks" | "currentTimeS">) {
  const safeDuration = durationS > 0 ? durationS : 1;
  const playheadPct = Math.max(0, Math.min(100, (currentTimeS / safeDuration) * 100));

  return (
    <div className="relative h-full overflow-auto">
      <div className="min-w-full">
        <Lane label="AUDIO" rowHeight={72} last>
          <AudioWaveform peaks={audioPeaks} />
        </Lane>
      </div>
      {/* Global playhead overlay */}
      <div
        aria-hidden
        className="pointer-events-none absolute inset-y-0 w-0.5 -translate-x-1/2 bg-[var(--color-viz-playhead)]"
        style={{
          left: `calc(96px + (100% - 96px) * ${playheadPct} / 100)`,
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
    <div className={cn("grid grid-cols-[96px_minmax(0,1fr)] items-stretch", !last && "border-b border-[var(--color-border-subtle)]")}>
      <div
        className="z-10 flex items-center justify-end pr-3 border-r border-[var(--color-border-subtle)] bg-[var(--color-surface-panel)] sticky left-0"
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

function AudioWaveform({ peaks }: { peaks: number[] }) {
  if (peaks.length === 0) {
    return (
      <div className="absolute inset-0 grid place-items-center px-3">
        <span className="text-[var(--text-caption)] text-[var(--color-text-muted)]">
          No waveform index yet
        </span>
      </div>
    );
  }
  const data = peaks;
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
