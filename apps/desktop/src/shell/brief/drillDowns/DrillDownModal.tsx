/**
 * DrillDownModal — shared shell for every evidence drill-down panel.
 *
 * Wave 3 C1. Renders the chrome (header + body + close button) and
 * routes Escape / backdrop-click to `onClose`. Per-kind drill-downs
 * (`ScenesDrillDown`, `SilencesDrillDown`, …) plug into `children`.
 *
 * Surface contract:
 *   - Reuses the global `.modal-*` CSS scaffolding from `tokens.css`
 *     so spacing, borders, and motion stay consistent with the rest
 *     of the chrome (Settings, NewProjectForm, etc).
 *   - Header carries title + count + close. Counts come from the
 *     accessor that owns the data so we don't refetch here.
 *   - Body is scroll-managed by `.modal-body`; per-kind drill-downs
 *     own their own grid / list layout.
 *   - Footer is optional; per-kind components pass `footer` only
 *     when they need an action row.
 */

import { useEffect } from "react";
import type { ReactNode } from "react";

export { formatTimecode, jumpTimelineTo } from "./utils";

interface DrillDownModalProps {
  title: string;
  count: number;
  onClose: () => void;
  children: ReactNode;
  footer?: ReactNode;
}

export function DrillDownModal({
  title,
  count,
  onClose,
  children,
  footer,
}: DrillDownModalProps) {
  useEffect(() => {
    function onKey(event: KeyboardEvent) {
      if (event.key === "Escape") {
        event.preventDefault();
        onClose();
      }
    }
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [onClose]);

  return (
    <div
      className="modal-backdrop"
      role="presentation"
      onClick={onClose}
    >
      <div
        className="modal"
        role="dialog"
        aria-modal="true"
        aria-label={`${title} drill-down`}
        onClick={(event) => event.stopPropagation()}
        style={{
          width: "min(720px, calc(100vw - 48px))",
          maxHeight: "min(560px, calc(100vh - 48px))",
        }}
      >
        <header className="modal-header">
          <div className="flex items-baseline gap-2">
            <h2>{title}</h2>
            <span
              className="font-mono tabular-nums text-[var(--text-caption)] text-[var(--color-text-muted)]"
              aria-label={`${count} ${title.toLowerCase()}`}
            >
              {count}
            </span>
          </div>
          <button
            type="button"
            className="modal-close"
            onClick={onClose}
            aria-label={`Close ${title} drill-down`}
          >
            ×
          </button>
        </header>
        <div className="modal-body">{children}</div>
        {footer ? <footer className="modal-footer">{footer}</footer> : null}
      </div>
    </div>
  );
}

/**
 * Reusable empty state for "indexer hasn't produced this kind yet"
 * or "indexer ran and found nothing".
 *
 * `onRunIndexers` flips the chrome over to the re-run path; we just
 * call it. Wiring lives in the caller (`EvidenceChipRow`) because
 * `index_project` is a Tauri-only invoke and the modal stays
 * environment-agnostic for tests.
 */
export function DrillDownEmpty({
  message,
  onRunIndexers,
}: {
  message: string;
  onRunIndexers?: () => void;
}) {
  return (
    <div className="flex flex-col items-center justify-center gap-3 py-10 text-center">
      <p className="max-w-[36ch] text-[var(--text-body-sm)] text-[var(--color-text-muted)]">
        {message}
      </p>
      {onRunIndexers ? (
        <button
          type="button"
          onClick={onRunIndexers}
          className="h-7 px-3 rounded-[var(--radius-sm)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-card)] text-[var(--text-caption)] font-medium text-[var(--color-text-secondary)] hover:bg-[var(--color-surface-card-hover)] hover:text-[var(--color-text-primary)]"
        >
          Re-run indexers
        </button>
      ) : null}
    </div>
  );
}

