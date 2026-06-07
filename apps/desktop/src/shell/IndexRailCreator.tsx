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
      <div className="index-rail flex h-full min-h-0 flex-col overflow-y-auto">
        <button
          type="button"
          onClick={() => setShowDetails(false)}
          className="border-b border-[var(--glass-border)] px-3 py-2 text-left text-[11px] font-semibold text-[#FF9A45] transition-colors hover:bg-[rgba(255,255,255,0.04)]"
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
    <div className="index-rail flex h-full min-h-0 flex-col overflow-y-auto text-[12px]">
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
    <div className="glass-content flex flex-col gap-2 bg-gradient-to-br from-[color-mix(in_srgb,#FF7A18_14%,var(--glass-content))] to-[var(--glass-content)] px-3 py-3">
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
      <div className="h-1 overflow-hidden rounded-full bg-[rgba(255,255,255,0.06)]">
        <div
          className="h-full rounded-full transition-[width] duration-300"
          style={{
            width: `${percent}%`,
            background: isReady ? "#5EEAD4" : "#FF7A18",
            boxShadow: isReady
              ? "0 0 8px rgba(94,234,212,0.55)"
              : percent >= 100
                ? "0 0 8px rgba(255,122,24,0.55)"
                : "none",
          }}
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
        <div key={c.lab} className="glass-content px-2.5 py-2">
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
      Montage is reading your media. As soon as it's done, the agent can propose cleanup
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
