import { cn } from "../../ui/cn";
import {
  WORKSPACE_SHORTCUTS,
  useStageStore,
  type Stage,
} from "../../state";
import { useMediaStore } from "../../media/store";

/**
 * WorkspaceRow — row 2 of the redesigned top chrome.
 *
 * Layout:
 *   [Edit] [Deliver] [Schedule] [Skills] [History]             00:00:00:00 / 00:00:38:17
 *
 * Tabs drive the global stage (`useStageStore`). All four tabs are
 * routable destinations: Edit and Deliver are workflow stages, Skills
 * and History are non-linear surfaces the user jumps to and back from.
 * The right side mirrors the preview clock — current playhead over
 * total duration — using whichever clock the media store currently
 * exposes (timeline-time when a timeline is loaded, source-time otherwise).
 *
 * Both timecodes fall back to `00:00:00:00` when nothing is loaded so the
 * row always renders cleanly.
 */

type TabId = Stage;

type TabDef = {
  id: TabId;
  label: string;
  kbd: string;
  disabled?: boolean;
};

const TABS: readonly TabDef[] = WORKSPACE_SHORTCUTS.map((shortcut) => ({
  id: shortcut.stage,
  label: shortcut.label,
  kbd: shortcut.keys,
}));

export function WorkspaceRow() {
  const active = useStageStore((s) => s.current);
  const setActive = useStageStore((s) => s.set);
  const { current, total } = usePreviewClock();

  return (
    <div
      className="flex items-center justify-between h-[30px] px-3 border-t border-[var(--color-border-subtle)]"
      role="tablist"
      aria-label="Workspace"
    >
      <div className="flex items-center gap-4">
        {TABS.map((t) => {
          const isActive = !t.disabled && active === t.id;
          return (
            <button
              key={t.id}
              type="button"
              role="tab"
              aria-selected={isActive}
              aria-disabled={t.disabled || undefined}
              disabled={t.disabled}
              title={t.kbd ? `${t.label} (${t.kbd})` : t.label}
              onClick={() => {
                if (t.disabled) return;
                setActive(t.id);
              }}
              className={cn(
                "relative h-[30px] -mb-px text-[12px] font-semibold",
                t.disabled
                  ? "text-[var(--color-text-disabled)] cursor-not-allowed"
                  : isActive
                  ? "text-[var(--color-text-primary)]"
                  : "text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)]",
              )}
            >
              {t.label}
              {isActive && (
                <span
                  aria-hidden
                  className="absolute -bottom-px left-0 right-0 h-[2px] bg-[var(--color-brand)]"
                />
              )}
            </button>
          );
        })}
      </div>
      <div className="font-mono text-[12px] text-[var(--color-text-muted)] tabular-nums">
        {current} <span className="text-[var(--color-text-disabled)]">/</span> {total}
      </div>
    </div>
  );
}

/**
 * Reads the live preview clock from the media store and formats it as
 * `HH:MM:SS:FF`. Prefers timeline-time (the clock the user thinks in when
 * they look at the scrub bar) and falls back to source-time when no
 * timeline is loaded.
 */
function usePreviewClock(): { current: string; total: string } {
  const sourceCurrent = useMediaStore((s) => s.currentTime);
  const sourceDuration = useMediaStore((s) => s.durationS);
  const timelineCurrent = useMediaStore((s) => s.timelineTime);
  const timelineDuration = useMediaStore((s) => s.timelineDurationS);

  const useTimeline = timelineDuration > 0;
  const current = useTimeline ? timelineCurrent : sourceCurrent;
  const total = useTimeline ? timelineDuration : sourceDuration;

  return {
    current: formatTimecode(current),
    total: formatTimecode(total),
  };
}

function pad(n: number) {
  return n.toString().padStart(2, "0");
}

/** `HH:MM:SS:FF` at 24 fps — matches PreviewSurface's transport readout. */
function formatTimecode(totalSeconds: number) {
  if (!isFinite(totalSeconds) || totalSeconds < 0) return "00:00:00:00";
  const total = Math.max(0, totalSeconds);
  const hours = Math.floor(total / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  const seconds = Math.floor(total % 60);
  const frames = Math.floor((total - Math.floor(total)) * 24);
  return `${pad(hours)}:${pad(minutes)}:${pad(seconds)}:${pad(frames)}`;
}
