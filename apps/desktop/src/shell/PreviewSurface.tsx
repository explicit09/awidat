import {
  Check,
  ChevronDown,
  Edit3,
  Maximize2,
  MoreHorizontal,
  Pause,
  Play,
  Search,
  SkipBack,
  SkipForward,
  Volume2,
  X,
} from "lucide-react";
import { useState, type ReactNode } from "react";
import {
  Inline,
  IconButton,
  Stack,
  cn,
  type TimelineMarkerKind,
} from "../ui";
import type { PreviewQualityMode } from "../media/previewSource";

/**
 * PreviewSurface — the center "Preview / Review" surface from the design spec
 * (~/Downloads/Awidat UI Design Concept.md §8 Screen 2).
 *
 * What this owns:
 *
 *   ┌─ header ─────────────────────────────────────────────────────────────────┐
 *   │ PROPOSAL: Podcast Tightening v1 ▾    12 pending changes    [Before/After] │
 *   ├─ stage ─────────────────────────────────────────────────────────────────-┤
 *   │                                                                          │
 *   │                            <video stage>                                 │
 *   │                                                                          │
 *   ├─ jump chips ────────────────────────────────────────────────────────────-┤
 *   │ Jump to change  01 02 03 04 05 06 07 08 09 10 11 12                      │
 *   ├─ transport ────────────────────────────────────────────────────────────-─┤
 *   │ 00:06:42:11 / 00:08:12:03   ⏮ ⏯ ⏭   1.0x   ◐━━━━━━━ ⚙                  │
 *   ├─ scrubber ─────────────────────────────────────────────────────────────-─┤
 *   │ ●━━━━●━━━━━●━━━━━●━━━━●━━━━●━━━●━━━━━●━━━━━●━━━●━━━━●━━━●               │
 *   └────────────────────────────────────────────────────────────────────────-─┘
 *
 * The actual <SegmentedVideoView> will be slotted into `<stage>` at Phase 2.11
 * cutover. Until then, `videoSlot` defaults to a placeholder.
 */

export type PreviewChange = {
  id: string;
  index: number;
  kind: TimelineMarkerKind;
  timeS: number;
  label?: string;
};

export type PreviewViewMode = "before-after" | "side-by-side";

export type PreviewSurfaceProps = {
  /** Proposal name shown in the header (e.g. "Podcast Tightening v1"). */
  proposalName?: string;
  /** Count of pending changes for the pill. */
  pendingCount?: number;
  /** All changes for the jump-chip row + scrubber markers. */
  changes?: PreviewChange[];
  /** Currently-active change index (highlights the chip). */
  activeChangeId?: string;
  /** Current playhead in source seconds. */
  currentTimeS?: number;
  /** Total duration in source seconds. */
  durationS?: number;
  /** Whether the player is playing. */
  isPlaying?: boolean;
  /** Volume 0..1. */
  volume?: number;
  /** Playback rate. */
  rate?: number;
  /** Viewer media quality preference for source review. */
  qualityMode?: PreviewQualityMode;
  /** View mode for the dual-cam preview. */
  viewMode?: PreviewViewMode;

  /** Slot for the actual video element (SegmentedVideoView wraps in Phase 2.11). */
  videoSlot?: ReactNode;

  /** Callbacks — wired at cutover. */
  onSelectChange?: (change: PreviewChange) => void;
  onPlayPause?: () => void;
  onPrevCut?: () => void;
  onNextCut?: () => void;
  onSeek?: (timeS: number) => void;
  onSetVolume?: (v: number) => void;
  onSetRate?: (r: number) => void;
  onSetQualityMode?: (m: PreviewQualityMode) => void;
  onSetViewMode?: (m: PreviewViewMode) => void;
  onOpenProposalMenu?: () => void;
  onInspectProposal?: () => void;
  onReviseProposal?: () => void;
  onAcceptProposal?: () => void;
  onRejectProposal?: () => void;
  onFullscreen?: () => void;
};

const RATES = [0.5, 1, 1.25, 1.5, 2];

export function PreviewSurface({
  proposalName = "Podcast Tightening v1",
  pendingCount = 12,
  changes = [],
  activeChangeId,
  currentTimeS = 0,
  durationS = 0,
  isPlaying = false,
  volume = 0.7,
  rate = 1,
  qualityMode = "auto",
  viewMode = "before-after",
  videoSlot,
  onSelectChange,
  onPlayPause,
  onPrevCut,
  onNextCut,
  onSeek,
  onSetVolume,
  onSetRate,
  onSetQualityMode,
  onSetViewMode,
  onOpenProposalMenu,
  onInspectProposal,
  onReviseProposal,
  onAcceptProposal,
  onRejectProposal,
  onFullscreen,
}: PreviewSurfaceProps) {
  const [rateMenuOpen, setRateMenuOpen] = useState(false);
  const [moreMenuOpen, setMoreMenuOpen] = useState(false);

  return (
    <Stack gap="0" className="h-full w-full bg-[var(--color-surface-app)]">
      {/*
        Header — visually merged into the video stage so the viewer
        reads as the center of gravity. Pure black background, no
        bottom border; chrome dissolves into the stage. Only the
        proposal selector and mode toggle stay visible. Everything
        else moved to the overflow menu.
      */}
      <div className="grid grid-cols-[minmax(0,1fr)_auto_auto] items-center gap-2 px-3 h-10 bg-black shrink-0">
        <Inline gap="2" align="center" className="min-w-0 overflow-hidden">
          <button
            type="button"
            onClick={onOpenProposalMenu}
            className="inline-flex min-w-0 items-center gap-1.5 h-7 px-2 rounded-[var(--radius-sm)] text-[var(--color-text-secondary)] hover:bg-[rgba(255,255,255,0.04)] hover:text-[var(--color-text-primary)] transition-colors"
            title={proposalName}
          >
            <span className="min-w-0 truncate text-[var(--text-body-sm)]">
              {proposalName}
            </span>
            <ChevronDown className="h-3.5 w-3.5 shrink-0 stroke-[1.75] opacity-60" />
          </button>
          {pendingCount > 0 ? (
            <span className="shrink-0 inline-flex items-center gap-1 text-[var(--text-caption)] text-[var(--color-text-muted)]">
              <span
                className="h-1.5 w-1.5 rounded-full"
                style={{ backgroundColor: "rgb(217, 165, 75)" }}
                aria-hidden
              />
              {pendingCount} pending
            </span>
          ) : null}
        </Inline>
        <Inline gap="0" align="center" className="shrink-0">
          <ViewModeToggle value={viewMode} onChange={onSetViewMode} />
        </Inline>
        <Inline gap="0" align="center">
          <div className="relative">
            <IconButton
              icon={<MoreHorizontal />}
              label="More"
              size="sm"
              onClick={() => setMoreMenuOpen((x) => !x)}
              aria-haspopup="menu"
              aria-expanded={moreMenuOpen}
            />
            {moreMenuOpen ? (
              <PreviewMoreMenu
                onClose={() => setMoreMenuOpen(false)}
                onInspect={onInspectProposal}
                onRevise={onReviseProposal}
                onAccept={onAcceptProposal}
                onReject={onRejectProposal}
              />
            ) : null}
          </div>
          <IconButton icon={<Maximize2 />} label="Fullscreen" size="sm" onClick={onFullscreen} />
        </Inline>
      </div>

      {/* Video stage — pure black, no top border. The header above
          shares the same background so the viewer extends visually
          to the very top of the panel. */}
      <div className="relative flex-1 min-h-0 min-w-0 bg-black overflow-hidden">
        {videoSlot ?? <VideoPlaceholder viewMode={viewMode} />}
      </div>

      {/* Jump chips */}
      {changes.length > 0 ? (
        <div className="px-4 py-2 border-t border-[var(--color-border-subtle)] bg-[var(--color-surface-panel)] shrink-0">
          <Inline gap="2" align="center" className="overflow-x-auto">
            <span className="text-[var(--text-caption)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-muted)] shrink-0">
              Jump to change
            </span>
            <Inline gap="1" align="center">
              {changes.map((c) => (
                <button
                  key={c.id}
                  type="button"
                  onClick={() => onSelectChange?.(c)}
                  className={cn(
                    "inline-flex items-center justify-center h-6 min-w-7 px-1.5 rounded-[var(--radius-xs)]",
                    "text-[var(--text-caption)] font-mono font-semibold border",
                    "transition-[background-color,border-color,color] duration-[120ms]",
                    c.id === activeChangeId
                      ? "bg-[var(--color-pill-warning-fill)] border-[var(--color-pill-warning-border)] text-[var(--color-pill-warning-text)]"
                      : "bg-[var(--color-surface-card)] border-[var(--color-border-subtle)] text-[var(--color-text-secondary)] hover:border-[var(--color-border)] hover:text-[var(--color-text-primary)]",
                  )}
                  title={c.label}
                >
                  {String(c.index).padStart(2, "0")}
                </button>
              ))}
            </Inline>
          </Inline>
        </div>
      ) : null}

      {/* Transport */}
      <div className="px-4 h-12 border-t border-[var(--color-border-subtle)] bg-[var(--color-surface-panel)] grid grid-cols-[auto_1fr_auto] items-center gap-4 shrink-0">
        <span className="font-mono text-[var(--text-timecode)] text-[var(--color-text-primary)]">
          <span className="text-[var(--color-text-primary)]">{formatTimecode(currentTimeS)}</span>
          <span className="text-[var(--color-text-muted)]"> / {formatTimecode(durationS)}</span>
        </span>
        <Inline gap="1" align="center" justify="center">
          <IconButton icon={<SkipBack />} label="Previous cut" size="md" onClick={onPrevCut} />
          <IconButton
            icon={isPlaying ? <Pause /> : <Play />}
            label={isPlaying ? "Pause" : "Play"}
            size="lg"
            onClick={onPlayPause}
            tone="accent"
            className="!h-9 !w-9 !rounded-full bg-[var(--color-surface-card)] border border-[var(--color-border)] hover:bg-[var(--color-surface-card-hover)]"
          />
          <IconButton icon={<SkipForward />} label="Next cut" size="md" onClick={onNextCut} />
        </Inline>
        <Inline gap="3" align="center">
          <div className="relative">
            <button
              type="button"
              onClick={() => setRateMenuOpen((x) => !x)}
              className="inline-flex items-center h-7 px-2.5 rounded-[var(--radius-sm)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-card)] hover:border-[var(--color-border)] transition-colors font-mono text-[var(--text-body-sm)] text-[var(--color-text-primary)]"
              aria-haspopup="listbox"
              aria-expanded={rateMenuOpen}
            >
              {rate.toFixed(2).replace(/\.?0+$/, "")}x
            </button>
            {rateMenuOpen ? (
              <div
                role="listbox"
                className="absolute bottom-full right-0 mb-1 z-10 min-w-[60px] rounded-[var(--radius-md)] border border-[var(--color-border)] bg-[var(--color-surface-modal)] elev-2 p-1"
              >
                {RATES.map((r) => (
                  <button
                    key={r}
                    type="button"
                    role="option"
                    aria-selected={r === rate}
                    onClick={() => {
                      onSetRate?.(r);
                      setRateMenuOpen(false);
                    }}
                    className={cn(
                      "flex w-full items-center justify-between px-2 py-1 rounded-[var(--radius-sm)] text-[var(--text-body-sm)] font-mono",
                      r === rate
                        ? "bg-[var(--color-surface-selected)] text-[var(--color-text-primary)]"
                        : "text-[var(--color-text-secondary)] hover:bg-[var(--color-surface-hover)] hover:text-[var(--color-text-primary)]",
                    )}
                  >
                    {r.toFixed(2).replace(/\.?0+$/, "")}x
                  </button>
                ))}
              </div>
            ) : null}
          </div>
          <QualityModeSelect value={qualityMode} onChange={onSetQualityMode} />
          <Inline gap="2" align="center" className="w-32">
            <Volume2 className="h-4 w-4 stroke-[1.75] text-[var(--color-text-muted)] shrink-0" />
            <input
              type="range"
              min={0}
              max={1}
              step={0.01}
              value={volume}
              onChange={(e) => onSetVolume?.(Number(e.target.value))}
              aria-label="Volume"
              // brand-sweep: transport/playback controls use cyan, not brand orange.
              className="flex-1 accent-[var(--color-brand-secondary)]"
            />
          </Inline>
        </Inline>
      </div>

      {/* Scrubber + marker rail */}
      <div className="px-4 py-2 border-t border-[var(--color-border-subtle)] bg-[var(--color-surface-panel)] shrink-0">
        <Scrubber
          changes={changes}
          currentTimeS={currentTimeS}
          durationS={durationS}
          onSeek={onSeek}
          onSelectChange={onSelectChange}
          activeChangeId={activeChangeId}
        />
        <Inline justify="between" align="center" className="mt-1">
          <Timecodes durationS={durationS} />
        </Inline>
      </div>
    </Stack>
  );
}

function PreviewMoreMenu({
  onClose,
  onInspect,
  onRevise,
  onAccept,
  onReject,
}: {
  onClose: () => void;
  onInspect?: () => void;
  onRevise?: () => void;
  onAccept?: () => void;
  onReject?: () => void;
}) {
  const actions = [
    { label: "Inspect deeper", icon: Search, action: onInspect },
    { label: "Revise", icon: Edit3, action: onRevise },
    { label: "Accept", icon: Check, action: onAccept },
    { label: "Reject", icon: X, action: onReject },
  ];
  return (
    <div
      role="menu"
      className="absolute right-0 top-full z-20 mt-1 w-40 rounded-[var(--radius-md)] border border-[var(--color-border)] bg-[var(--color-surface-modal)] p-1 elev-2"
    >
      {actions.map(({ label, icon: Icon, action }) => (
        <button
          key={label}
          type="button"
          role="menuitem"
          disabled={!action}
          onClick={() => {
            action?.();
            onClose();
          }}
          className={cn(
            "flex h-7 w-full items-center gap-2 rounded-[var(--radius-sm)] px-2 text-left text-[var(--text-body-sm)]",
            "text-[var(--color-text-secondary)] transition-colors hover:bg-[var(--color-surface-hover)] hover:text-[var(--color-text-primary)]",
            "disabled:cursor-not-allowed disabled:opacity-40 disabled:hover:bg-transparent disabled:hover:text-[var(--color-text-secondary)]",
          )}
        >
          <Icon className="h-3.5 w-3.5 shrink-0 stroke-[1.75]" />
          <span>{label}</span>
        </button>
      ))}
    </div>
  );
}

function VideoPlaceholder({ viewMode }: { viewMode: PreviewViewMode }) {
  const Cell = ({ label }: { label: string }) => (
    <div className="flex-1 flex items-center justify-center bg-gradient-to-br from-[#0E1623] to-[#070A0F] border border-[var(--color-border-subtle)] rounded-[var(--radius-md)] m-2">
      <Stack gap="2" align="center">
        <div className="h-12 w-12 rounded-full border-2 border-[var(--color-border)] flex items-center justify-center">
          <Play className="h-5 w-5 stroke-[1.75] text-[var(--color-text-muted)] ml-1" />
        </div>
        <span className="text-[var(--text-caption)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-muted)]">
          {label}
        </span>
      </Stack>
    </div>
  );
  if (viewMode === "side-by-side") {
    return (
      <div className="flex h-full w-full">
        <Cell label="Speaker A" />
        <Cell label="Speaker B" />
      </div>
    );
  }
  return (
    <div className="relative h-full w-full">
      <Cell label="Preview" />
      <div className="absolute top-1/2 left-1/2 -translate-x-1/2 -translate-y-1/2 h-10 w-10 rounded-full bg-[var(--color-surface-modal)] border border-[var(--color-border-strong)] flex items-center justify-center text-[var(--text-caption)] text-[var(--color-text-secondary)] font-mono">
        ◀▶
      </div>
    </div>
  );
}

function ViewModeToggle({
  value,
  onChange,
}: {
  value: PreviewViewMode;
  onChange?: (m: PreviewViewMode) => void;
}) {
  // Borderless on the viewer's black background — the underline on
  // the active option is the only ink. Mirrors how Resolve / FCP
  // signal a viewer-mode toggle without a bordered pill box.
  return (
    <div className="inline-flex items-center">
      {(
        [
          { key: "before-after", label: "Before / After" },
          { key: "side-by-side", label: "Side by Side" },
        ] as const
      ).map((opt) => (
        <button
          key={opt.key}
          type="button"
          onClick={() => onChange?.(opt.key)}
          aria-pressed={value === opt.key}
          className={cn(
            "h-7 px-2.5 text-[var(--text-caption)] transition-colors",
            value === opt.key
              ? "text-[var(--color-text-primary)] border-b border-[rgba(255,255,255,0.6)]"
              : "text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)]",
          )}
        >
          {opt.label}
        </button>
      ))}
    </div>
  );
}

function QualityModeSelect({
  value,
  onChange,
}: {
  value: PreviewQualityMode;
  onChange?: (m: PreviewQualityMode) => void;
}) {
  return (
    <label className="inline-flex h-7 items-center gap-1 rounded-[var(--radius-sm)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-card)] px-2 text-[var(--text-caption)] text-[var(--color-text-secondary)]">
      <span className="uppercase tracking-[var(--text-label--letter-spacing)] text-[var(--color-text-muted)]">
        Quality
      </span>
      <select
        value={value}
        onChange={(event) => onChange?.(event.target.value as PreviewQualityMode)}
        className="bg-transparent font-mono text-[var(--text-caption)] text-[var(--color-text-primary)] outline-none"
        aria-label="Preview quality"
      >
        <option value="auto">Auto</option>
        <option value="full">Full</option>
        <option value="proxy">Proxy</option>
      </select>
    </label>
  );
}

function Scrubber({
  changes,
  currentTimeS,
  durationS,
  onSeek,
  onSelectChange,
  activeChangeId,
}: {
  changes: PreviewChange[];
  currentTimeS: number;
  durationS: number;
  onSeek?: (timeS: number) => void;
  onSelectChange?: (c: PreviewChange) => void;
  activeChangeId?: string;
}) {
  const safeDuration = durationS > 0 ? durationS : 1;
  function pct(t: number) {
    return Math.max(0, Math.min(100, (t / safeDuration) * 100));
  }
  function onTrackClick(e: React.MouseEvent<HTMLDivElement>) {
    const rect = e.currentTarget.getBoundingClientRect();
    const ratio = (e.clientX - rect.left) / rect.width;
    onSeek?.(ratio * safeDuration);
  }
  return (
    <div className="relative h-6">
      <div
        className="absolute inset-x-0 top-1/2 -translate-y-1/2 h-1.5 rounded-full bg-[var(--color-surface-input)] cursor-pointer"
        onClick={onTrackClick}
      >
        <div
          className="absolute inset-y-0 left-0 rounded-full bg-[var(--accent-selection)]/40"
          style={{ width: `${pct(currentTimeS)}%` }}
        />
      </div>
      {/* Change markers — clickable dots driven by the parent track click */}
      {changes.map((c) => (
        <MarkerDot
          key={c.id}
          kind={c.kind}
          left={pct(c.timeS)}
          active={c.id === activeChangeId}
          label={c.label ?? `Change ${c.index}`}
          onClick={(e) => {
            e.stopPropagation();
            onSelectChange?.(c);
          }}
        />
      ))}
      {/* Playhead */}
      <div
        aria-hidden
        className="absolute top-0 bottom-0 w-0.5 -translate-x-1/2 bg-[var(--color-viz-playhead)]"
        style={{
          left: `${pct(currentTimeS)}%`,
          boxShadow: "0 0 8px var(--color-viz-playhead)",
        }}
      />
    </div>
  );
}

const MARKER_COLOR: Record<TimelineMarkerKind, string> = {
  accepted: "var(--color-success)",
  rejected: "var(--color-danger)",
  pending: "var(--color-warning)",
  warning: "var(--color-warning)",
  info: "var(--color-info)",
  skipped: "var(--color-text-muted)",
  selected: "var(--color-brand-purple)",
};

function MarkerDot({
  kind,
  left,
  active,
  label,
  onClick,
}: {
  kind: TimelineMarkerKind;
  left: number;
  active: boolean;
  label: string;
  onClick: (e: React.MouseEvent) => void;
}) {
  const color = MARKER_COLOR[kind];
  return (
    <button
      type="button"
      onClick={onClick}
      aria-label={label}
      title={label}
      className={cn(
        "absolute top-1/2 -translate-y-1/2 -translate-x-1/2 rounded-full",
        "transition-transform duration-[120ms] hover:scale-125",
        "focus-visible:outline-2 focus-visible:outline-[var(--color-border-focus)]",
        active ? "h-3 w-3 z-10 ring-2 ring-offset-2 ring-offset-[var(--color-surface-panel)]" : "h-2 w-2 z-0",
      )}
      style={{
        left: `${left}%`,
        backgroundColor: color,
        ["--tw-ring-color" as never]: color,
      }}
    />
  );
}

function Timecodes({ durationS }: { durationS: number }) {
  const ticks = 6;
  const step = durationS / ticks;
  return (
    <div className="flex w-full justify-between font-mono text-[var(--text-micro)] text-[var(--color-text-muted)]">
      {Array.from({ length: ticks + 1 }).map((_, i) => (
        <span key={i}>{formatTimecodeShort(i * step)}</span>
      ))}
    </div>
  );
}

function pad(n: number) {
  return n.toString().padStart(2, "0");
}

function formatTimecode(totalSeconds: number) {
  if (!isFinite(totalSeconds) || totalSeconds < 0) return "00:00:00:00";
  const total = Math.max(0, totalSeconds);
  const hours = Math.floor(total / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  const seconds = Math.floor(total % 60);
  const frames = Math.floor((total - Math.floor(total)) * 24);
  return `${pad(hours)}:${pad(minutes)}:${pad(seconds)}:${pad(frames)}`;
}

function formatTimecodeShort(totalSeconds: number) {
  if (!isFinite(totalSeconds) || totalSeconds < 0) return "00:00";
  const minutes = Math.floor(totalSeconds / 60);
  const seconds = Math.floor(totalSeconds % 60);
  return `${pad(minutes)}:${pad(seconds)}`;
}
