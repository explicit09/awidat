/**
 * CenterModeTabs — the Brief / Source / Timeline tab strip that lives
 * above the Edit-tab center pane (Wave 3 B1).
 *
 * Visual contract: same chrome pattern as `WorkspaceRow` — mono small-
 * caps labels, orange underline on the active tab. The strip is short
 * (28px) so it doesn't compete with the surface beneath it.
 */

import { cn } from "../../ui";
import {
  CENTER_MODE_LABEL,
  CENTER_MODES,
  type CenterMode,
} from "../../state/centerMode";

export interface CenterModeTabsProps {
  active: CenterMode;
  onChange: (mode: CenterMode) => void;
  /** Per-tab badge (e.g. pending count next to "Brief"). */
  badges?: Partial<Record<CenterMode, number>>;
}

export function CenterModeTabs({ active, onChange, badges }: CenterModeTabsProps) {
  return (
    <div
      className="flex h-7 shrink-0 items-center gap-4 border-b border-[var(--color-border-subtle)] bg-[var(--color-surface-panel)] px-3"
      role="tablist"
      aria-label="Center view mode"
    >
      {CENTER_MODES.map((mode) => {
        const isActive = mode === active;
        const badge = badges?.[mode];
        return (
          <button
            key={mode}
            type="button"
            role="tab"
            aria-selected={isActive}
            onClick={() => onChange(mode)}
            className={cn(
              "relative inline-flex h-7 items-center gap-1.5 text-[10px] font-semibold uppercase tracking-[0.1em]",
              "transition-[color] duration-[120ms]",
              isActive
                ? "text-[var(--color-text-primary)]"
                : "text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)]",
            )}
          >
            {CENTER_MODE_LABEL[mode]}
            {typeof badge === "number" && badge > 0 && (
              <span className="rounded-[var(--radius-sm)] bg-[var(--color-brand)] px-1 text-[9px] font-bold text-[var(--color-text-inverse)] leading-[1.4]">
                {badge}
              </span>
            )}
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
  );
}
