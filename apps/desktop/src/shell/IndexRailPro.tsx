import { useMemo, useState, type ReactNode } from "react";
import { Button, StatusPill, cn } from "../ui";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "../components/ui/Popover";
import { useProjectStore } from "../app/state";
import { useIndexerOverlay } from "../state";
import type {
  IndexerConfigSnapshot,
  IndexingStructurePreview,
  IndexingTask,
} from "./IndexingDashboard";
import { buildRailModel, type RailModel, type RailSignal } from "./indexRailModel";

/**
 * IndexRailPro — Pro-mode rendering of the index readiness rail.
 *
 * Per redesign spec §7.1, replaces the 776-line 4-column IndexingDashboard
 * concept layout with a dense, single-column rail consumed by AppShell's
 * indexing surface. Sections:
 *   1. Header — title + job pill + 1-line meta + 4px progress bar
 *   2. 4-stat grid — Duration / Scenes / Segments / Transcript (— placeholders)
 *   3. Three signal groups — Speech / Visuals / Audio (3-col tile grid each)
 *   4. Indexers strip — "N indexers active · first 3 names · +overflow"
 *
 * Data shape: re-uses the existing IndexingTask / IndexerConfigSnapshot /
 * IndexingStructurePreview types from IndexingDashboard so callers don't
 * need to migrate state. The view-model is built in `indexRailModel.ts`.
 */

export type IndexRailProProps = {
  tasks?: IndexingTask[];
  structurePreview?: IndexingStructurePreview;
  indexerConfig?: IndexerConfigSnapshot;
  ready?: boolean;
  /** Re-run / refresh all indexers. */
  onRefreshIndexers?: () => void;
  onOpenConfigPath?: (path: string) => void;
};

export function IndexRailPro({
  tasks = [],
  structurePreview,
  indexerConfig,
  ready = false,
  onRefreshIndexers,
  onOpenConfigPath,
}: IndexRailProProps) {
  const model = buildRailModel(tasks, structurePreview, indexerConfig);
  return (
    <div className="flex h-full min-h-0 flex-col overflow-y-auto bg-[var(--color-surface-panel)] text-[12px]">
      <div className="flex flex-col gap-3 p-3.5">
        <Header model={model} ready={ready} />
        <StatGrid model={model} />
        <SignalGroup title="Speech" signals={model.bySection.speech} />
        <SignalGroup title="Visuals" signals={model.bySection.visuals} />
        <SignalGroup title="Audio" signals={model.bySection.audio} />
        <IndexersStrip model={model} onRefreshIndexers={onRefreshIndexers} />
        <Footer
          model={model}
          indexerConfig={indexerConfig}
          onRefreshIndexers={onRefreshIndexers}
          onOpenConfigPath={onOpenConfigPath}
        />
      </div>
    </div>
  );
}

function Header({ model, ready }: { model: RailModel; ready: boolean }) {
  const showReady = ready || model.percent >= 100;
  return (
    <div className="flex flex-col gap-1">
      <div className="flex items-center justify-between">
        <h4 className="text-[13px] font-semibold text-[var(--color-text-primary)]">
          Index readiness
        </h4>
        {showReady ? (
          <StatusPill family="job" state="ready" />
        ) : (
          <StatusPill family="job" state="running" percent={model.percent} label="Indexing" />
        )}
      </div>
      <div className="font-mono text-[11px] text-[var(--color-text-muted)]">
        {model.ready} of {model.total} ready · {model.queued} queued
        {model.etaText ? ` · ETA ${model.etaText}` : ""}
      </div>
      <div className="h-1 overflow-hidden rounded-full bg-[var(--color-surface-input)]">
        <div
          className="h-full bg-gradient-to-r from-[var(--color-brand)] to-[#FCA67A]"
          style={{ width: `${Math.max(0, Math.min(100, model.percent))}%` }}
        />
      </div>
    </div>
  );
}

function StatGrid({ model }: { model: RailModel }) {
  const cells: Array<{ val: ReactNode; lab: string }> = [
    { val: model.durationLabel ?? "—", lab: "Duration" },
    { val: typeof model.scenes === "number" ? model.scenes.toLocaleString() : "—", lab: "Scenes" },
    {
      val: typeof model.segments === "number" ? model.segments.toLocaleString() : "—",
      lab: "Segments",
    },
    { val: model.transcriptLabel ?? "—", lab: "Transcript" },
  ];
  return (
    <div className="grid grid-cols-2 gap-1.5">
      {cells.map((c) => (
        <div
          key={c.lab}
          className="rounded-md border border-[var(--color-border-subtle)] bg-[var(--color-surface-card)] px-2.5 py-2"
        >
          <div className="font-mono text-[13px] font-semibold text-[var(--color-text-primary)]">
            {c.val}
          </div>
          <div className="mt-0.5 text-[10px] uppercase tracking-[0.06em] text-[var(--color-text-muted)]">
            {c.lab}
          </div>
        </div>
      ))}
    </div>
  );
}

function SignalGroup({ title, signals }: { title: string; signals: RailSignal[] }) {
  const readyCount = signals.filter((s) => s.state === "ready").length;
  const padded: (RailSignal | null)[] = [...signals];
  while (padded.length % 3 !== 0) padded.push(null);

  return (
    <div className="flex flex-col gap-1.5">
      <div className="flex items-baseline justify-between">
        <span className="text-[10px] font-bold uppercase tracking-[0.08em] text-[var(--color-text-muted)]">
          {title}
        </span>
        <span className="font-mono text-[11px] text-[var(--color-text-secondary)]">
          {readyCount} / {signals.length} ready
        </span>
      </div>
      <div className="grid grid-cols-3 gap-px overflow-hidden rounded-md border border-[var(--color-border-subtle)] bg-[var(--color-border-subtle)]">
        {padded.map((s, i) =>
          s ? <SignalTile key={s.name} signal={s} /> : <PadTile key={`pad-${i}`} />,
        )}
      </div>
    </div>
  );
}

function SignalTile({ signal }: { signal: RailSignal }) {
  return (
    <div className="flex min-h-[42px] flex-col justify-between bg-[var(--color-surface-card)] px-2 py-1.5">
      <div className="text-[11px] font-semibold text-[var(--color-text-primary)]">
        {signal.name}
      </div>
      <div className="flex items-center gap-1 font-mono text-[10px] text-[var(--color-text-muted)]">
        {signal.state === "running" ? (
          <StatusPill family="job" state="running" percent={signal.percent} dotOnly />
        ) : (
          <StatusPill family="job" state={signal.state} dotOnly />
        )}
        <span>
          {signal.state}
          {signal.state === "running" && signal.percent !== undefined
            ? ` ${signal.percent}%`
            : ""}
        </span>
      </div>
    </div>
  );
}

function PadTile() {
  return (
    <div className="min-h-[42px] bg-[var(--color-surface-card)] px-2 py-1.5 text-[var(--color-text-disabled)] opacity-40">
      <span className="text-[11px]">—</span>
    </div>
  );
}

/**
 * Per-indexer enable/disable popover (Wave 4 T3) — restores the
 * affordance the legacy IndexReadinessPanel exposed inline. Mirrors
 * the W4.1 skills pattern: store hydrated from `.awidat/indexers.json`
 * on project open; every toggle persists back through
 * `write_disabled_indexers`; the dispatcher reads the same file at
 * run time, so any view that omits the popover (CLI, auto-chain)
 * still honors the user's choices.
 *
 * Active count in the strip reflects "configured minus disabled" so
 * the chip count matches what the next Re-run will actually launch.
 */
function IndexersStrip({
  model,
  onRefreshIndexers,
}: {
  model: RailModel;
  onRefreshIndexers?: () => void;
}) {
  const projectRoot = useProjectStore((s) => s.current);
  // Subscribe to the disabled map (not isDisabled) so the strip
  // re-renders when toggles happen elsewhere; isDisabled is a getter
  // that closes over the current map snapshot.
  const disabledByProject = useIndexerOverlay((s) => s.disabledByProject);
  const toggle = useIndexerOverlay((s) => s.toggle);
  const [open, setOpen] = useState(false);

  const disabledSet = useMemo(
    () => disabledByProject.get(projectRoot ?? "__global__") ?? new Set<string>(),
    [disabledByProject, projectRoot],
  );

  const configured = model.indexers;
  const activeNames = configured
    .filter((i) => !disabledSet.has(i.name))
    .map((i) => i.name);
  const previewNames = configured.map((i) => i.name).slice(0, 3);
  const overflow = Math.max(0, configured.length - 3);

  if (configured.length === 0) {
    return (
      <div className="flex items-center justify-between border-t border-[var(--color-border-subtle)] pt-2 text-[11px] text-[var(--color-text-muted)]">
        <span>0 indexers active</span>
      </div>
    );
  }

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <button
          type="button"
          className="flex w-full items-center justify-between gap-2 rounded border-t border-[var(--color-border-subtle)] pt-2 text-left text-[11px] text-[var(--color-text-muted)] transition-colors hover:text-[var(--color-text-primary)] focus:outline-none focus-visible:text-[var(--color-text-primary)]"
          aria-label="Configure active indexers"
        >
          <span>
            {activeNames.length} of {configured.length}{" "}
            {configured.length === 1 ? "indexer" : "indexers"} active
          </span>
          <span className="flex flex-wrap justify-end gap-1">
            {previewNames.map((n) => (
              <IndexerChip
                key={n}
                label={n}
                disabled={disabledSet.has(n)}
              />
            ))}
            {overflow > 0 ? <IndexerChip label={`+${overflow}`} /> : null}
          </span>
        </button>
      </PopoverTrigger>
      <PopoverContent align="end" sideOffset={6}>
        <IndexerTogglePanel
          configured={configured}
          disabledSet={disabledSet}
          onToggle={(name) => toggle(projectRoot, name)}
          onRerun={
            onRefreshIndexers
              ? () => {
                  setOpen(false);
                  onRefreshIndexers();
                }
              : undefined
          }
        />
      </PopoverContent>
    </Popover>
  );
}

function IndexerChip({ label, disabled }: { label: string; disabled?: boolean }) {
  return (
    <span
      className={cn(
        "rounded border border-[var(--color-border-subtle)] bg-[var(--color-surface-card)] px-1.5 py-px font-mono text-[10px]",
        disabled
          ? "text-[var(--color-text-disabled)] line-through opacity-60"
          : "text-[var(--color-text-secondary)]",
      )}
    >
      {label}
    </span>
  );
}

function IndexerTogglePanel({
  configured,
  disabledSet,
  onToggle,
  onRerun,
}: {
  configured: RailModel["indexers"];
  disabledSet: Set<string>;
  onToggle: (name: string) => void;
  onRerun?: () => void;
}) {
  return (
    <div className="flex w-[260px] flex-col gap-2 p-2">
      <div className="px-1 text-[10px] font-bold uppercase tracking-[0.08em] text-[var(--color-text-muted)]">
        Active indexers
      </div>
      <ul className="flex max-h-[240px] flex-col gap-px overflow-y-auto">
        {configured.map((indexer) => {
          const isDisabled = disabledSet.has(indexer.name);
          return (
            <li key={indexer.name}>
              <label
                className={cn(
                  "flex cursor-pointer items-center gap-2 rounded px-1.5 py-1 text-[12px] transition-colors hover:bg-[var(--color-surface-card)]",
                  isDisabled
                    ? "text-[var(--color-text-muted)]"
                    : "text-[var(--color-text-primary)]",
                )}
              >
                <input
                  type="checkbox"
                  checked={!isDisabled}
                  onChange={() => onToggle(indexer.name)}
                  className="h-3 w-3 cursor-pointer accent-[var(--color-brand)]"
                  aria-label={`${isDisabled ? "Enable" : "Disable"} ${indexer.name}`}
                />
                <span
                  className={cn(
                    "flex-1 truncate font-mono text-[11px]",
                    isDisabled ? "line-through opacity-70" : undefined,
                  )}
                >
                  {indexer.name}
                </span>
              </label>
            </li>
          );
        })}
      </ul>
      {onRerun ? (
        <div className="flex justify-end border-t border-[var(--color-border-subtle)] pt-2">
          <Button variant="secondary" size="xs" onClick={onRerun}>
            Re-run indexers
          </Button>
        </div>
      ) : null}
    </div>
  );
}

function Footer({
  model,
  indexerConfig,
  onRefreshIndexers,
  onOpenConfigPath,
}: {
  model: RailModel;
  indexerConfig?: IndexerConfigSnapshot;
  onRefreshIndexers?: () => void;
  onOpenConfigPath?: (path: string) => void;
}) {
  const hasConfigPaths = Boolean(indexerConfig?.projectPath || indexerConfig?.globalPath);
  if (!onRefreshIndexers && !hasConfigPaths) return null;
  return (
    <div
      className={cn(
        "flex flex-col gap-1.5 border-t border-[var(--color-border-subtle)] pt-2",
        model.indexers.length === 0 ? "border-t-0 pt-0" : null,
      )}
    >
      {onRefreshIndexers ? (
        <Button variant="secondary" size="xs" onClick={onRefreshIndexers}>
          Re-run indexers
        </Button>
      ) : null}
      {indexerConfig?.projectPath ? (
        <ConfigLine
          label="Project config"
          path={indexerConfig.projectPath}
          onOpen={onOpenConfigPath}
        />
      ) : null}
      {indexerConfig?.globalPath ? (
        <ConfigLine
          label="Global config"
          path={indexerConfig.globalPath}
          onOpen={onOpenConfigPath}
        />
      ) : null}
    </div>
  );
}

function ConfigLine({
  label,
  path,
  onOpen,
}: {
  label: string;
  path: string;
  onOpen?: (path: string) => void;
}) {
  return (
    <button
      type="button"
      onClick={onOpen ? () => onOpen(path) : undefined}
      disabled={!onOpen}
      className="flex items-center justify-between gap-2 rounded text-left text-[10px] uppercase tracking-[0.06em] text-[var(--color-text-muted)] transition-colors hover:text-[var(--color-text-primary)] disabled:cursor-default disabled:hover:text-[var(--color-text-muted)]"
    >
      <span className="font-semibold">{label}</span>
      <span className="min-w-0 flex-1 truncate text-right font-mono normal-case tracking-normal text-[var(--color-text-secondary)]">
        {path}
      </span>
    </button>
  );
}
