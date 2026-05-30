import { Settings as SettingsIcon } from "lucide-react";
import {
  Button,
  Card,
  Inline,
  Stack,
  StatusPill,
} from "../../ui";
import type { PreflightFinding } from "./types";

/**
 * Right column top — the currently-selected preflight issue with
 * full context (title, status pill, asset/time, body, suggested fix,
 * and a repair action when applicable).
 *
 * Status pill mapping:
 *   pass    → job/ready  ("Pass")
 *   warn    → job/running ("Warn") — no percent, just the pill state
 *   error   → job/failed ("Error")
 *
 * Using the StatusPill primitive means a future tone tweak in
 * tokens.css applies here automatically.
 */
export function IssueInspector({
  selectedIssue,
  onAgentRepair,
}: {
  selectedIssue?: PreflightFinding;
  onAgentRepair?: (finding: PreflightFinding) => void;
}) {
  if (!selectedIssue) return null;
  const isPass = selectedIssue.severity === "pass";
  const isWarn = selectedIssue.severity === "warning" || selectedIssue.severity === "info";
  const isError = selectedIssue.severity === "error" || selectedIssue.severity === "failure";
  return (
    <Card padding="md" tone={isPass ? "default" : isWarn ? "warning" : "danger"}>
      <Stack gap="3">
        <Inline justify="between" align="center" gap="2">
          <span className="min-w-0 truncate text-[var(--text-h3)] font-semibold text-[var(--color-text-primary)]">
            {selectedIssue.message}
          </span>
          {isPass ? (
            <StatusPill family="job" state="ready" label="Pass" size="sm" />
          ) : isError ? (
            <StatusPill family="job" state="failed" label="Error" size="sm" />
          ) : (
            <StatusPill family="job" state="running" label="Warn" size="sm" />
          )}
        </Inline>
        {/* Asset row — single line if it fits. */}
        {selectedIssue.asset || selectedIssue.time ? (
          <Inline justify="between" align="baseline" gap="2" className="min-w-0">
            <span className="min-w-0 truncate text-[var(--text-caption)] text-[var(--color-text-secondary)]">
              {selectedIssue.asset ?? "Timeline"}
            </span>
            {selectedIssue.time ? (
              <span className="shrink-0 font-mono text-[var(--text-caption)] text-[var(--color-text-muted)]">
                {selectedIssue.time}
              </span>
            ) : null}
          </Inline>
        ) : null}
        <p className="text-[var(--text-body-sm)] leading-relaxed text-[var(--color-text-secondary)]">
          {isPass
            ? "All preflight checks passed. Ready to export."
            : "This may affect platform quality. Use the agent repair flow to apply the safest automatic fix before export."}
        </p>
        {selectedIssue.suggestedFix ? (
          <div className="rounded-[var(--radius-sm)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-input)] px-3 py-2 text-[var(--text-caption)] text-[var(--color-text-secondary)]">
            {selectedIssue.suggestedFix}
          </div>
        ) : null}
        {!isPass ? (
          <Button
            variant="repair"
            size="sm"
            onClick={() => onAgentRepair?.(selectedIssue)}
            leadingIcon={<SettingsIcon className="h-3.5 w-3.5 stroke-[1.75]" />}
          >
            Agent Repair
          </Button>
        ) : null}
      </Stack>
    </Card>
  );
}
