import { GitBranch, RotateCcw, SendHorizontal, Sparkles, Undo2 } from "lucide-react";
import { useState } from "react";
import {
  Button,
  Card,
  EvidenceChip,
  IconButton,
  Inline,
  Pill,
  Stack,
  cn,
  type PillStatus,
} from "../ui";

/**
 * ReviseSurface — the Revise stage.
 *
 * Two columns: revision history on the left, ask-for-a-revision composer
 * in the center. Wraps the existing vedit Tauri commands semantically as
 * "ask the agent to revise" rather than raw git-for-the-timeline.
 */

export type RevisionEntry = {
  id: string;
  ref: string;
  at: string;
  title: string;
  by: "agent" | "user";
  status: PillStatus;
  tags?: string[];
};

export type ReviseSurfaceProps = {
  revisions?: RevisionEntry[];
  activeRef?: string;
  suggestions?: string[];
  onSubmit?: (prompt: string) => void;
  onPickRevision?: (rev: RevisionEntry) => void;
  onRestore?: (rev: RevisionEntry) => void;
  className?: string;
};

export function ReviseSurface({
  revisions = [],
  activeRef,
  suggestions = [],
  onSubmit,
  onPickRevision,
  onRestore,
  className,
}: ReviseSurfaceProps) {
  const [draft, setDraft] = useState("");

  function submit() {
    const trimmed = draft.trim();
    if (!trimmed) return;
    onSubmit?.(trimmed);
    setDraft("");
  }

  return (
    <div
      className={cn(
        "grid h-full grid-cols-[320px_1fr] bg-[var(--color-surface-app)]",
        className,
      )}
    >
      <aside className="border-r border-[var(--color-border-subtle)] bg-[var(--color-surface-panel)] flex flex-col min-h-0">
        <div className="h-10 px-3 flex items-center justify-between border-b border-[var(--color-border-subtle)] shrink-0">
          <Inline gap="2" align="center">
            <GitBranch className="h-3.5 w-3.5 stroke-[1.75] text-[var(--color-text-muted)]" />
            <span className="text-[var(--text-label)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-secondary)]">
              Revision history
            </span>
          </Inline>
          <IconButton icon={<Undo2 />} label="Undo last revision" size="sm" />
        </div>
        <div className="flex-1 overflow-y-auto p-2">
          <Stack gap="1">
            {revisions.map((rev) => {
              const active = activeRef === rev.ref;
              return (
                <button
                  key={rev.id}
                  type="button"
                  onClick={() => onPickRevision?.(rev)}
                  className={cn(
                    "text-left rounded-[var(--radius-sm)] border px-2.5 py-2 transition-colors",
                    active
                      ? "border-[var(--color-border-active)] bg-[var(--color-surface-selected)] glow-active"
                      : "border-[var(--color-border-subtle)] bg-[var(--color-surface-card)] hover:border-[var(--color-border)] hover:bg-[var(--color-surface-card-hover)]",
                  )}
                >
                  <Stack gap="1">
                    <Inline justify="between" align="center">
                      <span className="font-mono text-[var(--text-caption)] text-[var(--color-text-muted)]">
                        {rev.ref.slice(0, 7)}
                      </span>
                      <Pill status={rev.status} dot={false}>
                        {rev.by}
                      </Pill>
                    </Inline>
                    <span className="text-[var(--text-body-sm)] text-[var(--color-text-primary)] leading-snug">
                      {rev.title}
                    </span>
                    <Inline justify="between" align="center">
                      <span className="font-mono text-[var(--text-caption)] text-[var(--color-text-muted)]">
                        {rev.at}
                      </span>
                      {!active ? (
                        <button
                          type="button"
                          onClick={(e) => {
                            e.stopPropagation();
                            onRestore?.(rev);
                          }}
                          className="inline-flex items-center gap-1 text-[var(--text-caption)] text-[var(--color-brand-secondary)] hover:underline"
                        >
                          <RotateCcw className="h-3 w-3 stroke-[1.75]" />
                          <span>Restore</span>
                        </button>
                      ) : (
                        <span className="text-[var(--text-caption)] text-[var(--color-text-muted)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold">
                          Active
                        </span>
                      )}
                    </Inline>
                    {rev.tags && rev.tags.length > 0 ? (
                      <Inline gap="1" wrap="wrap">
                        {rev.tags.map((t) => (
                          <EvidenceChip key={t} label={t} />
                        ))}
                      </Inline>
                    ) : null}
                  </Stack>
                </button>
              );
            })}
            {revisions.length === 0 ? (
              <span className="text-[var(--text-caption)] text-[var(--color-text-muted)] p-2">
                No revisions yet. Ask the agent for a change to start the loop.
              </span>
            ) : null}
          </Stack>
        </div>
      </aside>

      <main className="flex flex-col p-6 min-h-0 overflow-y-auto">
        <Stack gap="4" className="max-w-[720px] mx-auto w-full">
          <Stack gap="1">
            <span className="text-[var(--text-h2)] font-semibold text-[var(--color-text-primary)]">
              Revise the working timeline
            </span>
            <span className="text-[var(--text-body-sm)] text-[var(--color-text-secondary)]">
              Tell the agent what to change. It will propose a revision you
              can review and accept — never destructive without your sign-off.
            </span>
          </Stack>
          <Card padding="md">
            <Stack gap="2">
              <textarea
                value={draft}
                onChange={(e) => setDraft(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
                    e.preventDefault();
                    submit();
                  }
                }}
                rows={4}
                placeholder="e.g. Make the open tighter. Keep the punchline at 06:42 intact."
                className="w-full resize-none bg-[var(--color-surface-input)] border border-[var(--color-border-subtle)] rounded-[var(--radius-md)] px-3 py-2 text-[var(--text-body)] text-[var(--color-text-primary)] placeholder:text-[var(--color-text-muted)] outline-none focus:border-[var(--color-border-focus)]"
              />
              <Inline justify="between" align="center">
                <span className="text-[var(--text-micro)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-muted)]">
                  ⌘↩ to send
                </span>
                <Button
                  variant="primary"
                  size="sm"
                  disabled={draft.trim().length === 0}
                  onClick={submit}
                  trailingIcon={<SendHorizontal className="h-3.5 w-3.5 stroke-[1.75]" />}
                >
                  Ask agent
                </Button>
              </Inline>
            </Stack>
          </Card>

          {suggestions.length > 0 ? (
            <Stack gap="2">
              <span className="text-[var(--text-label)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-muted)]">
                Suggested revisions
              </span>
              <Stack gap="1">
                {suggestions.map((s, i) => (
                  <button
                    key={i}
                    type="button"
                    onClick={() => onSubmit?.(s)}
                    className="text-left rounded-[var(--radius-md)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-card)] hover:bg-[var(--color-surface-card-hover)] hover:border-[var(--color-border)] px-3 py-2 transition-colors"
                  >
                    <Inline gap="2" align="center">
                      <Sparkles className="h-3.5 w-3.5 stroke-[1.75] text-[var(--color-brand-purple)] shrink-0" />
                      <span className="text-[var(--text-body-sm)] text-[var(--color-text-primary)]">{s}</span>
                    </Inline>
                  </button>
                ))}
              </Stack>
            </Stack>
          ) : null}
        </Stack>
      </main>
    </div>
  );
}
