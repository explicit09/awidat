import type { ReactNode } from "react";
import wordmark from "../brand/awidat-wordmark.svg";
import { AgentStatusBadge, Inline, cn } from "../ui";

/**
 * AppShell — the v2 application shell.
 *
 * Layout from the canonical design spec (~/Downloads/Awidat UI Design Concept.md §4):
 *
 *   ┌───────────────────────────────────────────────────────── 44 ─┐   top chrome
 *   │┌──────────┐┌─────────────────────────────────┐┌────────────┐│
 *   ││          ││         preview / review        ││            ││
 *   ││  agent   │└─────────────────────────────────┘│  proposal  ││
 *   ││  command ││         timeline / transcript    ││  inspector ││
 *   ││  rail    │└─────────────────────────────────┘│            ││
 *   ││  360     ││                                  ││  336       ││
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
  commandRail?: ReactNode;
  preview?: ReactNode;
  timeline?: ReactNode;
  timelineCollapsed?: boolean;
  inspector?: ReactNode;
  inspectorCollapsed?: boolean;
  /** Optional whole-workspace override for screens whose spec layout is not the three-pane cockpit. */
  workspace?: ReactNode;
  footer?: ReactNode;
};

export function AppShell({
  topChromeStart,
  topChromeCenter,
  topChromeEnd,
  commandRail,
  preview,
  timeline,
  timelineCollapsed = false,
  inspector,
  inspectorCollapsed = false,
  workspace,
  footer,
}: AppShellProps) {
  return (
    <div className="grid h-screen w-screen min-w-0 overflow-hidden grid-rows-[var(--layout-chrome-h)_1fr_var(--layout-footer-h)] bg-[var(--color-surface-page)] text-[var(--color-text-primary)] font-sans">
      {/* Top chrome */}
      <header className="grid min-w-0 grid-cols-[auto_minmax(160px,1fr)_auto] items-center gap-3 overflow-hidden border-b border-[var(--color-border-subtle)] bg-[var(--color-surface-app)] px-3">
        <div className="flex min-w-0 items-center justify-start">
          {topChromeStart ?? (
            <Inline gap="2" align="center">
              <img src={wordmark} alt="Awidat" className="h-7" />
            </Inline>
          )}
        </div>
        <div className="flex min-w-0 items-center justify-center overflow-hidden">{topChromeCenter}</div>
        <div className="flex min-w-0 items-center justify-end overflow-hidden">
          {topChromeEnd ?? <AgentStatusBadge status="idle" detail="No project" />}
        </div>
      </header>

      {/* Workspace */}
      {workspace ? (
        <div className="min-h-0 min-w-0 overflow-hidden p-2">
          <section className="panel h-full min-h-0 overflow-hidden">{workspace}</section>
        </div>
      ) : (
        <div
          className={cn(
            "grid min-h-0 min-w-0 overflow-hidden gap-2 p-2",
            inspectorCollapsed
              ? "grid-cols-[var(--layout-rail-w)_minmax(0,1fr)_44px]"
              : "grid-cols-[var(--layout-rail-w)_minmax(0,1fr)_var(--layout-inspector-w)]",
          )}
        >
          <aside className="panel min-h-0 overflow-y-auto">
            {commandRail ?? <RegionPlaceholder label="Agent / Command Rail · Phase 2.4" />}
          </aside>
          <main
            className={cn(
              "grid min-h-0 min-w-0 gap-2",
              timelineCollapsed
                ? "grid-rows-[minmax(260px,1fr)_44px]"
                : "grid-rows-[minmax(260px,48vh)_minmax(220px,1fr)]",
            )}
          >
            <section className="panel min-h-0 min-w-0 overflow-hidden">
              {preview ?? <RegionPlaceholder label="Preview / Review surface · Phase 2.5" />}
            </section>
            <section className="panel min-h-0 min-w-0 overflow-hidden">
              {timeline ?? <RegionPlaceholder label="Timeline / Transcript hybrid · Phase 2.6" />}
            </section>
          </main>
          <aside className={cn("panel min-h-0", inspectorCollapsed ? "overflow-hidden" : "overflow-y-auto")}>
            {inspector ?? <RegionPlaceholder label="Proposal Inspector · Phase 2.7" />}
          </aside>
        </div>
      )}

      {/* Status footer */}
      <footer className="min-w-0 overflow-hidden border-t border-[var(--color-border-subtle)] bg-[var(--color-surface-app)] px-3 flex items-center justify-between">
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
