import { ShieldCheck } from "lucide-react";
import {
  Button,
  Inline,
  PreflightFindingRow,
  Stack,
  cn,
  type PreflightSeverity,
} from "../../ui";
import type { PreflightFinding } from "./types";

/**
 * Center column top — Preflight header + filter chips + finding list.
 *
 * Filter chips show their count tinted to the severity color
 * (green / amber / red, with neutral for "All"). The active chip
 * picks up the new `--color-surface-selected` background.
 *
 * The selected finding (passed in from the parent) gets a stronger
 * border + tinted fill so the Issue inspector on the right reads as
 * "this is the row I'm looking at."
 */
export function PreflightPanel({
  findings,
  filtered,
  counts,
  severityFilter,
  setSeverityFilter,
  selectedIssue,
  onAgentRepair,
}: {
  findings: PreflightFinding[];
  filtered: PreflightFinding[];
  counts: Record<PreflightSeverity, number>;
  severityFilter: "all" | PreflightSeverity;
  setSeverityFilter: (s: "all" | PreflightSeverity) => void;
  selectedIssue?: PreflightFinding;
  onAgentRepair?: (finding: PreflightFinding) => void;
}) {
  const errorCount = counts.error + counts.failure;
  return (
    <section className="flex min-h-0 flex-col">
      <div className="h-8 px-3 flex items-center justify-between border-b border-[var(--color-border-subtle)] shrink-0">
        <Inline gap="2" align="center">
          <ShieldCheck className="h-3.5 w-3.5 stroke-[1.75] text-[var(--color-text-muted)]" />
          <span className="text-[var(--text-label)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-secondary)]">
            Preflight
          </span>
          <span className="text-[var(--text-caption)] text-[var(--color-text-muted)]">
            {findings.length} {findings.length === 1 ? "finding" : "findings"}
          </span>
        </Inline>
        <Inline gap="1" align="center">
          <SeverityChip
            label="All"
            count={findings.length}
            active={severityFilter === "all"}
            onClick={() => setSeverityFilter("all")}
          />
          <SeverityChip
            label="Pass"
            count={counts.pass}
            color="var(--color-success)"
            active={severityFilter === "pass"}
            onClick={() => setSeverityFilter("pass")}
          />
          <SeverityChip
            label="Warn"
            count={counts.warning}
            color="var(--color-warning)"
            active={severityFilter === "warning"}
            onClick={() => setSeverityFilter("warning")}
          />
          <SeverityChip
            label="Error"
            count={errorCount}
            color="var(--color-danger)"
            active={severityFilter === "error"}
            onClick={() => setSeverityFilter("error")}
          />
        </Inline>
      </div>
      <div className="flex-1 overflow-y-auto p-2">
        {filtered.length === 0 ? (
          <EmptyPreflight />
        ) : (
          <Stack gap="1">
            {filtered.map((f) => {
              const isSelected = selectedIssue && f.id === selectedIssue.id;
              const isWarn = f.severity === "warning";
              const isError = f.severity === "failure" || f.severity === "error";
              return (
                <PreflightFindingRow
                  key={f.id}
                  severity={f.severity}
                  time={f.time}
                  message={f.message}
                  asset={f.asset}
                  suggestedFix={f.suggestedFix}
                  compact
                  className={cn(
                    isSelected && isWarn &&
                      "border-[rgba(245,158,11,0.75)] bg-[rgba(245,158,11,0.07)]",
                    isSelected && isError &&
                      "border-[rgba(239,68,68,0.65)] bg-[rgba(239,68,68,0.06)]",
                  )}
                  actions={
                    onAgentRepair && (isWarn || isError) ? (
                      <Button
                        variant="repair"
                        size="xs"
                        onClick={(e) => {
                          e.stopPropagation();
                          onAgentRepair(f);
                        }}
                      >
                        Repair
                      </Button>
                    ) : undefined
                  }
                />
              );
            })}
            {errorCount + counts.warning > 0 ? (
              <div
                className={cn(
                  "mt-0.5 rounded-[var(--radius-md)] border px-2.5 py-1.5",
                  errorCount > 0
                    ? "border-[rgba(239,68,68,0.45)] bg-[rgba(239,68,68,0.07)]"
                    : "border-[rgba(245,158,11,0.45)] bg-[rgba(245,158,11,0.08)]",
                )}
              >
                <span className="text-[var(--text-body-sm)] text-[var(--color-text-primary)]">
                  Preflight complete. {errorCount} {errorCount === 1 ? "failure" : "failures"}, {counts.warning} {counts.warning === 1 ? "warning" : "warnings"} found.
                </span>
                <p className="mt-1 text-[var(--text-caption)] text-[var(--color-text-secondary)]">
                  Address these issues to ensure the best quality and platform performance.
                </p>
              </div>
            ) : null}
          </Stack>
        )}
      </div>
    </section>
  );
}

/** SeverityChip — color-coded count for the preflight filter row.
 *  Active state uses --color-surface-selected; the count digit is
 *  tinted to its severity color so "1 warning" reads at a glance. */
function SeverityChip({
  label,
  count,
  color,
  active,
  onClick,
}: {
  label: string;
  count: number;
  color?: string;
  active: boolean;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      aria-pressed={active}
      className={cn(
        "inline-flex items-center gap-1.5 h-6 px-2 rounded-[var(--radius-xs)]",
        "text-[var(--text-caption)] font-medium border transition-colors",
        active
          ? "bg-[var(--color-surface-selected)] border-[var(--color-border-active)] text-[var(--color-text-primary)]"
          : "bg-[var(--color-surface-card)] border-[var(--color-border-subtle)] text-[var(--color-text-secondary)] hover:border-[var(--color-border)] hover:text-[var(--color-text-primary)]",
      )}
    >
      {color ? <span className="h-1.5 w-1.5 rounded-full" style={{ backgroundColor: color }} /> : null}
      <span>{label}</span>
      <span
        className="font-mono font-semibold"
        style={color ? { color } : { color: "var(--color-text-muted)" }}
      >
        {count}
      </span>
    </button>
  );
}

function EmptyPreflight() {
  return (
    <div className="flex h-full items-center justify-center p-8">
      <Stack gap="2" align="center" className="text-center">
        <ShieldCheck className="h-8 w-8 stroke-[1.5] text-[var(--color-success)]" />
        <span className="text-[var(--text-h3)] font-semibold text-[var(--color-text-primary)]">
          Nothing to flag
        </span>
        <span className="text-[var(--text-body-sm)] text-[var(--color-text-secondary)] max-w-sm">
          Preflight didn't find any issues for the selected targets.
          You can export when ready.
        </span>
      </Stack>
    </div>
  );
}
