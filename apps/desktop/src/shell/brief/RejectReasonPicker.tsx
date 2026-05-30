/**
 * RejectReasonPicker — inline reason capture for Brief proposal cards
 * (Wave 5 C1).
 *
 * Why: today's reject flow on Brief proposal cards is silent. Clicking
 * Reject vanishes the row and the agent learns nothing. This picker is
 * the first piece of the C-path feedback loop: a tight per-medium chip
 * list + a custom-text fallback that captures WHY the user rejected,
 * stamped onto the persisted HistoryEntry so later C-path tasks (logs,
 * agent prompt injection, pattern surfaces) can chew on it.
 *
 * Behavior:
 *   - Preset chip click  → instant apply + reject (no debounce).
 *   - Custom input Enter → trim → reject with the text (empty falls
 *     through to "reject without reason").
 *   - Reject without reason → preserves the historical silent-reject
 *     path so users who just want it gone aren't punished.
 *   - Esc anywhere in the picker → cancels the reject and closes.
 *
 * Token-only styling. Mount/unmount drives the slide-down animation
 * (keyframe `reject-picker-slide-down` lives in App.css).
 */

import { useEffect, useRef } from "react";
import type React from "react";
import { cn } from "../../ui";

export interface RejectReasonPickerProps {
  proposalId: string;
  proposalTitle: string;
  presets: readonly string[];
  customReason: string;
  onCustomChange: (next: string) => void;
  /** Called with the selected preset / trimmed custom text, or `undefined`
   *  for the explicit "reject without reason" path. Caller dispatches
   *  the actual reject through `useBriefProposalsStore.reject`. */
  onPick: (reason?: string) => void;
  onCancel: () => void;
}

export function RejectReasonPicker({
  proposalId,
  proposalTitle,
  presets,
  customReason,
  onCustomChange,
  onPick,
  onCancel,
}: RejectReasonPickerProps) {
  const firstChipRef = useRef<HTMLButtonElement>(null);

  // Auto-focus the first preset chip so keyboard users land on the
  // most-likely target. Mouse users see no caret jump — focus on a
  // button doesn't paint a caret.
  useEffect(() => {
    firstChipRef.current?.focus();
  }, []);

  function handleKeyDown(e: React.KeyboardEvent) {
    if (e.key === "Escape") {
      e.preventDefault();
      onCancel();
    }
  }

  function submitCustom() {
    const trimmed = customReason.trim();
    onPick(trimmed.length > 0 ? trimmed : undefined);
  }

  return (
    <div
      id={`reject-picker-${proposalId}`}
      role="group"
      aria-label={`Reject reason for ${proposalTitle}`}
      onKeyDown={handleKeyDown}
      className={cn(
        "flex flex-col gap-2",
        "border-t border-[var(--color-border-subtle)]",
        "bg-[var(--color-surface-panel)]",
        "px-2.5 py-2",
        "rounded-b-[var(--radius-md)]",
        // Slide-down entrance — keyframe defined in App.css next to the
        // sibling `proposal-slide-in`. Mount/unmount drives the run.
        "reject-picker-enter",
      )}
    >
      <div className="text-[10px] uppercase tracking-wider text-[var(--color-text-muted)]">
        Why reject?
      </div>
      <div className="flex flex-wrap gap-1.5">
        {presets.map((reason, idx) => (
          <button
            key={reason}
            ref={idx === 0 ? firstChipRef : undefined}
            type="button"
            onClick={() => onPick(reason)}
            className={cn(
              "h-6 rounded-full border px-2 text-[11px]",
              "border-[var(--color-border-subtle)] bg-[var(--color-surface-card)]",
              "text-[var(--color-text-secondary)]",
              "hover:border-[var(--color-border-strong)] hover:text-[var(--color-text-primary)]",
              "focus:border-[var(--color-brand)] focus:outline-none focus:ring-1 focus:ring-[var(--color-brand)]",
            )}
          >
            {reason}
          </button>
        ))}
      </div>
      <div className="flex items-center gap-1.5">
        <input
          type="text"
          value={customReason}
          onChange={(e) => onCustomChange(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              submitCustom();
            }
          }}
          placeholder="Or type a custom reason…"
          aria-label="Custom reject reason"
          className={cn(
            "h-7 flex-1 rounded-[var(--radius-sm)] border border-[var(--color-border-subtle)]",
            "bg-[var(--color-surface-input)] px-2 text-[11px]",
            "text-[var(--color-text-primary)] placeholder:text-[var(--color-text-muted)]",
            "focus:border-[var(--color-brand)] focus:outline-none",
          )}
        />
        <button
          type="button"
          onClick={submitCustom}
          className={cn(
            "h-7 rounded-[var(--radius-sm)] px-2 text-[11px] font-medium",
            "border border-[var(--color-border-subtle)] bg-[var(--color-surface-card)]",
            "text-[var(--color-text-primary)]",
            "hover:border-[var(--color-border-strong)]",
          )}
          aria-label="Reject with this reason"
        >
          Reject
        </button>
      </div>
      <div className="flex items-center justify-between gap-2">
        <button
          type="button"
          onClick={() => onPick(undefined)}
          className={cn(
            "text-[11px] text-[var(--color-text-muted)]",
            "hover:text-[var(--color-text-secondary)] underline-offset-2 hover:underline",
          )}
        >
          Reject without reason
        </button>
        <button
          type="button"
          onClick={onCancel}
          className={cn(
            "text-[11px] text-[var(--color-text-muted)]",
            "hover:text-[var(--color-text-secondary)]",
          )}
          aria-label="Cancel reject"
        >
          Cancel
        </button>
      </div>
    </div>
  );
}
