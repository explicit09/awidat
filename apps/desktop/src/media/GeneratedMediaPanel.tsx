import { RefreshCw, Sparkles } from "lucide-react";
import {
  Button,
  Card,
  Inline,
  Stack,
  StatusPill,
  type JobPillState,
  type ProposalPillState,
} from "../ui";
import type { GeneratedMediaEntry } from "./generatedMediaStore";

export function GeneratedMediaPanel({
  entries,
  loading,
  error,
  onRefresh,
  onUse,
}: {
  entries: GeneratedMediaEntry[];
  loading: boolean;
  error: string | null;
  onRefresh: () => void;
  onUse: (entry: GeneratedMediaEntry) => void;
}) {
  return (
    <Stack gap="2">
      <Inline justify="between" align="center" gap="2">
        <Inline align="center" gap="2" className="min-w-0">
          <Sparkles className="h-3.5 w-3.5 text-[var(--color-brand-secondary)]" aria-hidden />
          <span className="text-[var(--text-label)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-muted)]">
            Generated
          </span>
        </Inline>
        <Button variant="ghost" size="sm" onClick={onRefresh} aria-label="Refresh generated media">
          <RefreshCw className="h-3.5 w-3.5" aria-hidden />
        </Button>
      </Inline>
      {error ? (
        <Card padding="sm" tone="flat">
          <span className="text-[var(--text-caption)] text-[var(--color-danger)]">{error}</span>
        </Card>
      ) : null}
      {entries.length > 0 ? (
        <Stack gap="2">
          {entries.map((entry) => (
            <GeneratedMediaJobCard key={entry.job_id} entry={entry} onUse={onUse} />
          ))}
        </Stack>
      ) : (
        <Card padding="sm" tone="flat">
          <Stack gap="1">
            <span className="text-[var(--text-body-sm)] font-semibold text-[var(--color-text-primary)]">
              {loading ? "Checking generated media" : "No generated media yet"}
            </span>
            <span className="text-[var(--text-caption)] leading-relaxed text-[var(--color-text-muted)]">
              Generated B-roll jobs will appear here with prompt, provider, and disclosure metadata.
            </span>
          </Stack>
        </Card>
      )}
    </Stack>
  );
}

function GeneratedMediaJobCard({
  entry,
  onUse,
}: {
  entry: GeneratedMediaEntry;
  onUse: (entry: GeneratedMediaEntry) => void;
}) {
  const usable = entry.state === "succeeded" && Boolean(entry.video_path);
  return (
    <div
      className="rounded-[var(--radius-md)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-card)] px-3 py-2 text-left transition-colors hover:border-[var(--color-border)] hover:bg-[var(--color-surface-card-hover)]"
      title={entry.prompt_excerpt}
      draggable={usable}
      onDragStart={(event) => {
        if (!entry.video_path) return;
        event.dataTransfer.setData("application/x-awidat-media", entry.video_path);
        event.dataTransfer.setData("text/plain", entry.video_path);
        event.dataTransfer.effectAllowed = "copy";
      }}
    >
      <Inline justify="between" align="start" gap="2">
        <Stack gap="1" className="min-w-0">
          <span className="truncate text-[var(--text-body-sm)] font-semibold text-[var(--color-text-primary)]">
            {formatPurpose(entry.workflow_purpose)} · {entry.provider}
          </span>
          <span className="line-clamp-2 text-[var(--text-caption)] text-[var(--color-text-muted)]">
            {entry.prompt_excerpt}
          </span>
        </Stack>
        {(() => {
          const pill = statePill(entry.state);
          const label = stateLabel(entry.state);
          return pill.family === "job" ? (
            <StatusPill
              family="job"
              state={pill.state}
              label={label}
              className="shrink-0"
            />
          ) : (
            <StatusPill
              family="proposal"
              state={pill.state}
              label={label}
              className="shrink-0"
            />
          );
        })()}
      </Inline>
      <Inline justify="between" align="center" gap="2" className="mt-2">
        <span className="truncate text-[var(--text-caption)] text-[var(--color-text-muted)]">
          {entry.model}
          {entry.requires_disclosure ? " · AI disclosure" : ""}
          {entry.uses_likeness ? " · likeness" : ""}
        </span>
        {usable ? (
          <Button
            variant="ghost"
            size="sm"
            onClick={(event) => {
              event.stopPropagation();
              onUse(entry);
            }}
          >
            Add to timeline
          </Button>
        ) : null}
      </Inline>
      {entry.failure_message ? (
        <span className="mt-2 block text-[var(--text-caption)] text-[var(--color-danger)]">
          {entry.failure_message}
        </span>
      ) : null}
    </div>
  );
}

type GeneratedPill =
  | { family: "job"; state: JobPillState }
  | { family: "proposal"; state: ProposalPillState };

function statePill(state: string): GeneratedPill {
  switch (state) {
    case "succeeded":
      return { family: "job", state: "ready" };
    case "queued":
    case "running":
      return { family: "job", state: "running" };
    case "failed":
      return { family: "job", state: "failed" };
    case "cancelled":
      // Lossy: "warning" had no direct equivalent; cancelled is closer
      // to "failed" than to "ready" — render in the failed treatment.
      return { family: "job", state: "failed" };
    default:
      // "reviewing" → proposal/proposed (awaiting human action).
      return { family: "proposal", state: "proposed" };
  }
}

function stateLabel(state: string): string {
  return state.replace(/_/g, " ").replace(/\b\w/g, (letter) => letter.toUpperCase());
}

function formatPurpose(purpose: string): string {
  if (purpose === "broll") return "B-roll";
  return stateLabel(purpose);
}
