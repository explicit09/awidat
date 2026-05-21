import {
  AudioWaveform,
  BadgeCheck,
  Blocks,
  Captions,
  Database,
  Eye,
  Import,
  Palette,
  Upload,
  type LucideIcon,
} from "lucide-react";
import { LENSES, LENS_LABEL, useLensStore, type Lens } from "../state";
import { cn } from "../ui";

/**
 * Lens icons mapped from the canonical design spec §7.
 *
 * Lenses are the *working surface* the user is in — they're navigation, not state.
 * Selects/Assembly use Lucide icons that match the spec's recommendations.
 */
const LENS_ICON: Record<Lens, LucideIcon> = {
  import: Import,
  index: Database,
  selects: BadgeCheck,
  assembly: Blocks,
  review: Eye,
  captions: Captions,
  audio: AudioWaveform,
  color: Palette,
  delivery: Upload,
};

export type LensNavProps = {
  className?: string;
};

export function LensNav({ className }: LensNavProps) {
  const current = useLensStore((s) => s.current);
  const set = useLensStore((s) => s.set);

  return (
    <div role="tablist" aria-label="Workflow lens" className={cn("flex items-center gap-1 h-full", className)}>
      {LENSES.map((lens) => {
        const Icon = LENS_ICON[lens];
        const isCurrent = lens === current;
        return (
          <button
            key={lens}
            type="button"
            role="tab"
            aria-selected={isCurrent}
            onClick={() => set(lens)}
            className={cn(
              "relative inline-flex items-center gap-2 h-9 px-3",
              "text-[var(--text-body-sm)] font-medium",
              "transition-[color,background-color] duration-[120ms] ease-[cubic-bezier(0.2,0,0,1)]",
              "focus-visible:outline-2 focus-visible:outline-[var(--color-border-focus)] focus-visible:outline-offset-[-2px]",
              isCurrent
                ? "text-[var(--color-text-primary)]"
                : "text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)]",
            )}
          >
            <Icon
              className="h-4 w-4"
              strokeWidth={1.75}
              style={{
                color: isCurrent ? "var(--color-brand)" : "currentColor",
              }}
            />
            <span>{LENS_LABEL[lens]}</span>
            {isCurrent ? (
              <span
                aria-hidden
                className="absolute inset-x-2 bottom-0 h-0.5 rounded-full bg-[var(--color-brand)]"
              />
            ) : null}
          </button>
        );
      })}
    </div>
  );
}
