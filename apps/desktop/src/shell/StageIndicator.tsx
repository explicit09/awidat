import {
  Database,
  Eye,
  MessageSquareText,
  SlidersHorizontal,
  Sparkles,
  Upload,
  type LucideIcon,
} from "lucide-react";
import {
  STAGES,
  STAGE_LABEL,
  stageProgress,
  useStageStore,
  type Stage,
} from "../state";
import { cn } from "../ui";

/**
 * Stage glyphs from the canonical design spec §7. Each stage gets a single Lucide
 * icon — these are *not* purely decorative; they're how the user identifies the
 * stage in tiny chrome. Color comes from the active/complete state, not the icon.
 */
const STAGE_ICON: Record<Stage, LucideIcon> = {
  intent: MessageSquareText,
  indexing: Database,
  proposal: Sparkles,
  review: Eye,
  revise: SlidersHorizontal,
  deliver: Upload,
};

export type StageIndicatorProps = {
  className?: string;
};

/**
 * The six-stage indicator in the top chrome. Click to set the current stage.
 * In production the agent advances stages too — this is the user-facing surface.
 */
export function StageIndicator({ className }: StageIndicatorProps) {
  const current = useStageStore((s) => s.current);
  const visited = useStageStore((s) => s.visited);
  const set = useStageStore((s) => s.set);

  return (
    <div role="tablist" aria-label="Stage" className={cn("flex items-center gap-0.5", className)}>
      {STAGES.map((stage, i) => {
        const progress = stageProgress(stage, current, visited);
        const Icon = STAGE_ICON[stage];
        const isCurrent = progress === "current";
        const isComplete = progress === "complete";
        return (
          <div key={stage} className="flex items-center">
            <button
              type="button"
              role="tab"
              aria-selected={isCurrent}
              onClick={() => set(stage)}
              className={cn(
                "group inline-flex items-center gap-1 h-7 px-2 rounded-[var(--radius-sm)]",
                "border text-[var(--text-caption)] font-semibold uppercase tracking-[var(--text-label--letter-spacing)]",
                "transition-[background-color,border-color,color] duration-[120ms] ease-[cubic-bezier(0.2,0,0,1)]",
                "focus-visible:outline-2 focus-visible:outline-[var(--color-border-focus)] focus-visible:outline-offset-1",
                isCurrent &&
                  "bg-[var(--color-surface-selected)] border-[var(--color-border-active)] text-[var(--color-text-primary)] glow-active",
                isComplete && !isCurrent &&
                  "bg-transparent border-[var(--color-border-subtle)] text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] hover:bg-[var(--color-surface-hover)]",
                !isCurrent && !isComplete &&
                  "bg-transparent border-[var(--color-border-subtle)] text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)]",
              )}
            >
              <Icon
                className="h-3.5 w-3.5"
                strokeWidth={1.75}
                style={{
                  color: isCurrent
                    ? "var(--color-brand)"
                    : isComplete
                      ? "var(--color-success)"
                      : "currentColor",
                }}
              />
              <span>{STAGE_LABEL[stage]}</span>
            </button>
            {i < STAGES.length - 1 ? (
              <span
                aria-hidden
                className={cn(
                  "h-px w-1.5 mx-px",
                  isComplete ? "bg-[var(--color-success)] opacity-60" : "bg-[var(--color-border-subtle)]",
                )}
              />
            ) : null}
          </div>
        );
      })}
    </div>
  );
}
