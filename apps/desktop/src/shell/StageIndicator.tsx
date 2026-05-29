import {
  SlidersHorizontal,
  Upload,
  Sparkles,
  History,
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
  edit: SlidersHorizontal,
  deliver: Upload,
  skills: Sparkles,
  history: History,
};

export type StageIndicatorProps = {
  className?: string;
};

/**
 * The stage indicator in the top chrome. Click to set the current stage.
 * In production the agent advances stages too — this is the user-facing surface.
 */
export function StageIndicator({ className }: StageIndicatorProps) {
  const current = useStageStore((s) => s.current);
  const visited = useStageStore((s) => s.visited);
  const set = useStageStore((s) => s.set);

  return (
    <div role="tablist" aria-label="Stage" className={cn("flex items-center gap-0.5", className)}>
      {STAGES.map((stage) => {
        const progress = stageProgress(stage, current, visited);
        const Icon = STAGE_ICON[stage];
        const isCurrent = progress === "current";
        return (
          <button
            key={stage}
            type="button"
            role="tab"
            aria-selected={isCurrent}
            onClick={() => set(stage)}
            className={cn(
              "inline-flex items-center gap-1.5 h-7 px-2.5 rounded-[var(--radius-sm)]",
              "text-[var(--text-caption)] font-medium",
              "transition-[background-color,color] duration-[120ms] ease-[cubic-bezier(0.2,0,0,1)]",
              isCurrent
                ? "bg-[var(--color-surface-selected)] text-[var(--color-text-primary)]"
                : "text-[var(--color-text-muted)] hover:bg-[var(--color-surface-hover)] hover:text-[var(--color-text-primary)]",
            )}
          >
            <Icon className="h-3.5 w-3.5" strokeWidth={1.75} />
            <span>{STAGE_LABEL[stage]}</span>
          </button>
        );
      })}
    </div>
  );
}
