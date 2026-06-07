import type { ReactNode } from "react";
import { Panel, PanelGroup, PanelResizeHandle } from "react-resizable-panels";
import { cn } from "../ui";
import { TopChrome } from "./chrome/TopChrome";

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
  commandRail?: ReactNode;
  preview?: ReactNode;
  timeline?: ReactNode;
  timelineCollapsed?: boolean;
  /**
   * When true, the bottom timeline dock + its resize handle are
   * fully removed from the layout (preview swells to fill the whole
   * center). Used by Wave 4 W4.8 when the center pane is itself in
   * Timeline mode and the dock would be a duplicate.
   */
  timelineHidden?: boolean;
  inspector?: ReactNode;
  inspectorCollapsed?: boolean;
  /** Optional whole-workspace override for screens whose spec layout is not the three-pane cockpit. */
  workspace?: ReactNode;
  footer?: ReactNode;
};

export function AppShell({
  commandRail,
  preview,
  timeline,
  timelineCollapsed = false,
  timelineHidden = false,
  inspector,
  inspectorCollapsed = false,
  workspace,
  footer,
}: AppShellProps) {
  return (
    <div className="relative z-10 grid h-full w-full min-w-0 overflow-hidden grid-rows-[auto_1fr_var(--layout-footer-h)] bg-transparent text-[var(--color-text-primary)] font-sans">
      {/* Top chrome — redesigned in Task 8 (IdentityRow) + Task 9 (WorkspaceRow). */}
      <TopChrome />

      {/* Workspace */}
      {workspace ? (
        <div className="min-h-0 min-w-0 overflow-hidden p-2">
          <section className="panel h-full min-h-0 overflow-hidden">{workspace}</section>
        </div>
      ) : (
        <div className="min-h-0 min-w-0 overflow-hidden p-2">
          <PanelGroup
            direction="horizontal"
            autoSaveId={
              // Avoid persisting under the same id when the inspector
              // is in its collapsed/expanded variant — they have
              // different default proportions.
              inspectorCollapsed ? "awidat.shell.h.inspector-collapsed" : "awidat.shell.h"
            }
            className="h-full min-h-0"
          >
            <Panel
              id="command-rail"
              order={1}
              defaultSize={21}
              minSize={12}
              maxSize={40}
              className="panel min-h-0 overflow-hidden"
            >
              {commandRail ?? <RegionPlaceholder label="Agent / Command Rail · Phase 2.4" />}
            </Panel>
            <ResizeHandle orientation="horizontal" />
            <Panel
              id="center"
              order={2}
              // Default split mirrors what the design settled at in
              // dogfood: ~21 / 58 / 21 (rail / center / inspector).
              // When the inspector is collapsed to its 44px stub the
              // center swells to fill the freed space.
              defaultSize={inspectorCollapsed ? 77 : 58}
              minSize={30}
              className="min-h-0 min-w-0"
            >
              {timelineHidden ? (
                // Dock is hidden — preview fills the whole center
                // column. Skipping the PanelGroup entirely avoids
                // react-resizable-panels re-balancing into a stale
                // ratio when the dock comes back.
                <div className="panel h-full min-h-0 min-w-0 overflow-hidden">
                  {preview ?? <RegionPlaceholder label="Preview / Review surface · Phase 2.5" />}
                </div>
              ) : (
                <PanelGroup
                  direction="vertical"
                  autoSaveId={
                    timelineCollapsed
                      ? "awidat.shell.v.timeline-collapsed"
                      : "awidat.shell.v"
                  }
                  className="h-full min-h-0"
                >
                  <Panel
                    id="preview"
                    order={1}
                    // ~79/21 preview/timeline mirrors the settled
                    // dogfood layout — preview-heavy, timeline as a
                    // strip the user opens up only when they're
                    // actively trimming.
                    defaultSize={timelineCollapsed ? 92 : 79}
                    minSize={20}
                    className="panel min-h-0 min-w-0 overflow-hidden"
                  >
                    {preview ?? <RegionPlaceholder label="Preview / Review surface · Phase 2.5" />}
                  </Panel>
                  <ResizeHandle orientation="vertical" />
                  <Panel
                    id="timeline"
                    order={2}
                    defaultSize={timelineCollapsed ? 8 : 21}
                    minSize={timelineCollapsed ? 4 : 15}
                    className="panel min-h-0 min-w-0 overflow-hidden"
                  >
                    {timeline ?? <RegionPlaceholder label="Timeline / Transcript hybrid · Phase 2.6" />}
                  </Panel>
                </PanelGroup>
              )}
            </Panel>
            <ResizeHandle orientation="horizontal" />
            <Panel
              id="inspector"
              order={3}
              defaultSize={inspectorCollapsed ? 2 : 21}
              minSize={inspectorCollapsed ? 2 : 10}
              maxSize={40}
              className={cn("panel min-h-0 overflow-hidden")}
            >
              {inspector ?? <RegionPlaceholder label="Proposal Inspector · Phase 2.7" />}
            </Panel>
          </PanelGroup>
        </div>
      )}

      {/* Status footer */}
      <footer className="glass-soft min-w-0 overflow-hidden border-t border-[var(--glass-border)] px-3 flex items-center justify-between">
        {footer ?? (
          <>
            <span className="text-[var(--text-caption)] text-[var(--color-text-muted)] font-mono">
              local · idle · disk 412 GB free
            </span>
            <span className="text-[var(--text-caption)] text-[var(--color-text-muted)] font-mono">
              Montage v2 · ui-v2
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

/** Draggable splitter between two adjacent panels. Hairline thickness
 *  expands to a wider hit zone on hover via a `::before` pseudo so the
 *  visual line stays minimal but the click target is comfortable. */
function ResizeHandle({ orientation }: { orientation: "horizontal" | "vertical" }) {
  const horizontal = orientation === "horizontal";
  return (
    <PanelResizeHandle
      className={cn(
        "group relative shrink-0 transition-colors",
        horizontal
          ? "mx-1 w-px cursor-col-resize hover:bg-[var(--color-brand-secondary)]"
          : "my-1 h-px cursor-row-resize hover:bg-[var(--color-brand-secondary)]",
        "bg-[var(--color-border-subtle)] data-[resize-handle-state=drag]:bg-[var(--color-brand-secondary)]",
      )}
    >
      {/* Wider invisible hit-zone so the user doesn't have to land on
          the 1px hairline. */}
      <span
        className={cn(
          "absolute",
          horizontal ? "-left-1 -right-1 inset-y-0" : "-top-1 -bottom-1 inset-x-0",
        )}
        aria-hidden
      />
    </PanelResizeHandle>
  );
}
