import { useState } from "react";
import { StatusPill } from "../ui";
import { IndexRailPro, type IndexRailProProps } from "./IndexRailPro";
import { buildRailModel, type RailModel } from "./indexRailModel";

/**
 * IndexRailCreator — Creator-mode rendering of the index readiness rail.
 *
 * Per redesign spec §7.2, the Creator surface is a calmer summary of the
 * same data IndexRailPro consumes. It shows:
 *   1. A summary card (gradient, title, StatusPill, 4px progress, subtext)
 *   2. A 2-stat block (Duration + Scenes detected — half of Pro's grid)
 *   3. One short body paragraph framing what indexing means for the user
 *   4. A disclosure button that expands the Pro view in place
 *
 * Same prop shape as IndexRailPro so the parent `IndexRail.tsx` selector
 * can forward props through unchanged.
 */

export type IndexRailCreatorProps = IndexRailProProps;

export function IndexRailCreator(props: IndexRailCreatorProps) {
  const [showDetails, setShowDetails] = useState(false);

  if (showDetails) {
    return (
      <div className="flex h-full min-h-0 flex-col overflow-y-auto bg-[var(--color-surface-panel)]">
        <button
          type="button"
          onClick={() => setShowDetails(false)}
          className="border-b border-[var(--color-border-subtle)] px-3 py-2 text-left text-[11px] font-semibold text-[var(--color-brand)] transition-colors hover:bg-[var(--color-surface-hover)]"
        >
          ▴ Hide signal details
        </button>
        <div className="min-h-0 flex-1">
          <IndexRailPro {...props} />
        </div>
      </div>
    );
  }

  const { tasks = [], structurePreview, indexerConfig, ready = false } = props;
  const model = buildRailModel(tasks, structurePreview, indexerConfig);
  const isReady = ready || model.percent >= 100;

  return (
    <div className="flex h-full min-h-0 flex-col overflow-y-auto bg-[var(--color-surface-panel)] text-[12px]">
      <div className="flex flex-col gap-3 p-3.5">
        <SummaryCard model={model} isReady={isReady} />
        <SummaryStats model={model} />
        <BodyParagraph />
        <DisclosureButton onClick={() => setShowDetails(true)} />
      </div>
    </div>
  );
}

function SummaryCard({ model, isReady }: { model: RailModel; isReady: boolean }) {
  const percent = Math.max(0, Math.min(100, model.percent));
  const title = isReady ? "Ready to edit" : "Indexing your media…";
  const subtext = [
    `${model.ready} of ${model.total} signals ready`,
    model.etaText ? `ETA ${model.etaText}` : null,
    "works offline",
  ]
    .filter(Boolean)
    .join(" · ");

  return (
    <div className="flex flex-col gap-2 rounded-lg border border-[var(--color-border-subtle)] bg-gradient-to-br from-[color-mix(in_srgb,var(--color-brand)_14%,var(--color-surface-card))] to-[var(--color-surface-card)] px-3 py-3">
      <div className="flex items-start justify-between gap-2">
        <h4 className="text-[13px] font-semibold text-[var(--color-text-primary)]">
          {title}
        </h4>
        {isReady ? (
          <StatusPill family="job" state="ready" />
        ) : (
          <StatusPill family="job" state="running" percent={model.percent} label="Indexing" />
        )}
      </div>
      <div className="h-1 overflow-hidden rounded-full bg-[var(--color-surface-input)]">
        <div
          className="h-full bg-gradient-to-r from-[var(--color-brand)] to-[#FCA67A]"
          style={{ width: `${percent}%` }}
        />
      </div>
      <div className="font-mono text-[11px] text-[var(--color-text-muted)]">{subtext}</div>
    </div>
  );
}

function SummaryStats({ model }: { model: RailModel }) {
  const cells: Array<{ val: string; lab: string }> = [
    { val: model.durationLabel ?? "—", lab: "Duration" },
    {
      val: typeof model.scenes === "number" ? model.scenes.toLocaleString() : "—",
      lab: "Scenes detected",
    },
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

function BodyParagraph() {
  return (
    <p className="text-[12px] leading-relaxed text-[var(--color-text-secondary)]">
      Awidat is reading your media. As soon as it's done, the agent can propose cleanup
      edits.
    </p>
  );
}

function DisclosureButton({ onClick }: { onClick: () => void }) {
  return (
    <button
      type="button"
      onClick={onClick}
      className="self-start rounded text-left text-[11px] font-semibold text-[var(--color-brand)] transition-colors hover:text-[var(--color-text-primary)]"
    >
      ▾ Show signal details <span className="text-[var(--color-text-muted)]">· advanced</span>
    </button>
  );
}
