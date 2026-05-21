import type { ReactNode } from "react";
import wordmark from "../brand/awidat-wordmark.svg";
import { AgentStatusBadge, Inline } from "../ui";

/**
 * AppShell — the v2 application shell.
 *
 * Layout from the canonical design spec (~/Downloads/Awidat UI Design Concept.md §4):
 *
 *   ┌───────────────────────────────────────────────────────── 44 ─┐   top chrome
 *   ├───────────────────────────────────────────────────────── 44 ─┤   lens row
 *   │┌──────────┐┌─────────────────────────────────┐┌────────────┐│
 *   ││          ││         preview / review        ││            ││
 *   ││  agent   │└─────────────────────────────────┘│  proposal  ││
 *   ││  command ││         timeline / transcript    ││  inspector ││
 *   ││  rail    │└─────────────────────────────────┘│            ││
 *   ││  320     ││                                  ││  320       ││
 *   │└──────────┘└────────────────────────────────-─┘└────────────┘│
 *   ├───────────────────────────────────────────────────────── 32 ─┤   footer
 *   └────────────────────────────────────────────────────────────-─┘
 *
 * This component is the *frame*. Each region is a slot filled by a follow-up
 * Phase 2 task. Until those land, each region renders a placeholder.
 */
export type AppShellProps = {
  topChromeStart?: ReactNode;
  topChromeCenter?: ReactNode;
  topChromeEnd?: ReactNode;
  lensRow?: ReactNode;
  commandRail?: ReactNode;
  preview?: ReactNode;
  timeline?: ReactNode;
  inspector?: ReactNode;
  footer?: ReactNode;
};

export function AppShell({
  topChromeStart,
  topChromeCenter,
  topChromeEnd,
  lensRow,
  commandRail,
  preview,
  timeline,
  inspector,
  footer,
}: AppShellProps) {
  return (
    <div className="grid h-screen w-screen grid-rows-[var(--layout-chrome-h)_var(--layout-lens-h)_1fr_var(--layout-footer-h)] bg-[var(--color-surface-app)] text-[var(--color-text-primary)] font-sans">
      {/* Top chrome */}
      <header className="grid grid-cols-[1fr_auto_1fr] items-center gap-4 border-b border-[var(--color-border-subtle)] bg-[var(--color-surface-panel)] px-4">
        <div className="flex items-center justify-start">
          {topChromeStart ?? (
            <Inline gap="2" align="center">
              <img src={wordmark} alt="Awidat" className="h-7" />
            </Inline>
          )}
        </div>
        <div className="flex items-center justify-center">{topChromeCenter}</div>
        <div className="flex items-center justify-end">
          {topChromeEnd ?? <AgentStatusBadge status="idle" detail="No project" />}
        </div>
      </header>

      {/* Workflow lens row */}
      <nav className="border-b border-[var(--color-border-subtle)] bg-[var(--color-surface-panel)] px-4 flex items-center">
        {lensRow ?? (
          <span className="text-[var(--text-caption)] text-[var(--color-text-muted)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold">
            Lens nav — pending Phase 2.3
          </span>
        )}
      </nav>

      {/* Workspace */}
      <div className="grid grid-cols-[var(--layout-rail-w)_1fr_var(--layout-inspector-w)] min-h-0 overflow-hidden">
        <aside className="border-r border-[var(--color-border-subtle)] bg-[var(--color-surface-panel)] min-h-0 overflow-y-auto">
          {commandRail ?? <RegionPlaceholder label="Agent / Command Rail · Phase 2.4" />}
        </aside>
        <main className="grid grid-rows-[1fr_280px] min-h-0 min-w-0">
          <section className="bg-[var(--color-surface-app)] min-h-0 min-w-0 overflow-hidden">
            {preview ?? <RegionPlaceholder label="Preview / Review surface · Phase 2.5" />}
          </section>
          <section className="border-t border-[var(--color-border-subtle)] bg-[var(--color-surface-panel)] min-h-0 min-w-0 overflow-hidden">
            {timeline ?? <RegionPlaceholder label="Timeline / Transcript hybrid · Phase 2.6" />}
          </section>
        </main>
        <aside className="border-l border-[var(--color-border-subtle)] bg-[var(--color-surface-panel)] min-h-0 overflow-y-auto">
          {inspector ?? <RegionPlaceholder label="Proposal Inspector · Phase 2.7" />}
        </aside>
      </div>

      {/* Status footer */}
      <footer className="border-t border-[var(--color-border-subtle)] bg-[var(--color-surface-panel)] px-4 flex items-center justify-between">
        {footer ?? (
          <>
            <span className="text-[var(--text-caption)] text-[var(--color-text-muted)] font-mono">
              local · idle · disk 412 GB free
            </span>
            <span className="text-[var(--text-caption)] text-[var(--color-text-muted)] font-mono">
              Awidat v2 · ui-v2
            </span>
          </>
        )}
      </footer>
    </div>
  );
}

function RegionPlaceholder({ label }: { label: string }) {
  return (
    <div className="flex h-full w-full items-center justify-center p-4">
      <span className="text-[var(--text-caption)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-muted)]">
        {label}
      </span>
    </div>
  );
}
