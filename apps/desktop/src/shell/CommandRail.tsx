import {
  ChevronDown,
  ChevronRight,
  CircleStop,
  ListChecks,
  Paperclip,
  SendHorizontal,
  Sparkles,
  Terminal,
} from "lucide-react";
import { useState, type ReactNode } from "react";
import { Button, Divider, IconButton, Inline, Pill, Stack, cn } from "../ui";

/**
 * Command Rail — the left rail of the cockpit.
 *
 * Hard rule from the design spec §10: "should not look like a generic chatbot.
 * It should behave like a production command interface." Required sections per spec:
 *
 *   1. Natural-language command field        — the input
 *   2. Context chips                          — what the agent is grounded on
 *   3. Agent plan                             — what it intends to do
 *   4. Task progress                          — what it's doing now
 *   5. Collapsed activity log                 — what it's done
 *   6. Suggested next actions                 — what to do next
 *
 * This component renders that *shape*. Wiring to useAgentStore lifecycle
 * (start_turn / cancel_turn / respond_user_input) happens at App.tsx cutover.
 */

export type ContextChip = { label: string; kind?: "media" | "selection" | "project" | "lens" };

export type PlanItem = {
  id: string;
  text: string;
  status: "pending" | "in_progress" | "complete" | "failed";
};

export type ActivityEntry = {
  id: string;
  timestamp: string;
  text: string;
  kind?: "tool" | "thought" | "result";
};

export type SuggestedAction = {
  id: string;
  label: string;
  prompt: string;
};

export type CommandRailProps = {
  /** Used to disable Send when no project is open. */
  hasProject?: boolean;
  /** Active context the agent is grounded on. */
  contextChips?: ContextChip[];
  /** What the agent intends to do for the current turn. */
  plan?: PlanItem[];
  /** Current task progress label, e.g. "Reading transcript… 67%". */
  taskProgress?: { label: string; progress?: number };
  /** Activity log entries — collapsed by default. */
  activity?: ActivityEntry[];
  /** Suggestions the agent surfaces for the user's next move. */
  suggestions?: SuggestedAction[];
  /** True while a turn is running — toggles Send → Stop. */
  running?: boolean;
  /** Called when the user submits the command field. */
  onSubmit?: (command: string) => void;
  /** Called when the user clicks Stop. */
  onCancel?: () => void;
  /** Called when the user picks a suggested action. */
  onSuggestion?: (action: SuggestedAction) => void;
  /** Called when a context chip is removed. */
  onRemoveChip?: (chip: ContextChip, index: number) => void;
};

export function CommandRail({
  hasProject = true,
  contextChips = [],
  plan = [],
  taskProgress,
  activity = [],
  suggestions = [],
  running = false,
  onSubmit,
  onCancel,
  onSuggestion,
  onRemoveChip,
}: CommandRailProps) {
  const [draft, setDraft] = useState("");
  const [activityOpen, setActivityOpen] = useState(false);

  function submit() {
    const trimmed = draft.trim();
    if (!trimmed || !hasProject) return;
    onSubmit?.(trimmed);
    setDraft("");
  }

  return (
    <div className="flex h-full flex-col">
      {/* Composer */}
      <div className="p-3 border-b border-[var(--color-border-subtle)]">
        <Stack gap="2">
          <Inline gap="2" align="center">
            <Terminal className="h-3.5 w-3.5 stroke-[1.75] text-[var(--color-brand)]" />
            <span className="text-[var(--text-label)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-secondary)]">
              Command
            </span>
          </Inline>
          {contextChips.length > 0 ? (
            <Inline gap="1" wrap="wrap">
              {contextChips.map((chip, i) => (
                <button
                  key={`${chip.label}-${i}`}
                  type="button"
                  onClick={() => onRemoveChip?.(chip, i)}
                  className="inline-flex items-center gap-1 h-5 px-1.5 rounded-[var(--radius-xs)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-card)] text-[var(--text-caption)] text-[var(--color-text-secondary)] hover:border-[var(--color-border)] hover:text-[var(--color-text-primary)] transition-colors"
                  aria-label={`Remove ${chip.label}`}
                >
                  <Paperclip className="h-3 w-3 stroke-[1.75]" />
                  <span>{chip.label}</span>
                </button>
              ))}
            </Inline>
          ) : null}
          <div className="rounded-[var(--radius-md)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-input)] focus-within:border-[var(--color-border-focus)] transition-colors">
            <textarea
              value={draft}
              onChange={(e) => setDraft(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
                  e.preventDefault();
                  submit();
                }
              }}
              placeholder={
                hasProject
                  ? "Cut this into a tight 8-minute episode."
                  : "Open a project to begin."
              }
              rows={3}
              disabled={!hasProject}
              className={cn(
                "w-full resize-none bg-transparent px-3 py-2",
                "text-[var(--text-body)] text-[var(--color-text-primary)] placeholder:text-[var(--color-text-muted)]",
                "outline-none disabled:cursor-not-allowed disabled:opacity-50",
              )}
            />
            <Inline justify="between" align="center" className="px-2 py-1.5 border-t border-[var(--color-border-subtle)]">
              <span className="text-[var(--text-micro)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-muted)]">
                ⌘↩ to send
              </span>
              {running ? (
                <Button
                  variant="secondary"
                  size="sm"
                  onClick={onCancel}
                  leadingIcon={<CircleStop className="h-3.5 w-3.5 stroke-[1.75]" />}
                >
                  Stop
                </Button>
              ) : (
                <Button
                  variant="primary"
                  size="sm"
                  disabled={!hasProject || draft.trim().length === 0}
                  onClick={submit}
                  trailingIcon={<SendHorizontal className="h-3.5 w-3.5 stroke-[1.75]" />}
                >
                  Send
                </Button>
              )}
            </Inline>
          </div>
        </Stack>
      </div>

      {/* Scroll region: plan, progress, activity, suggestions */}
      <div className="flex-1 overflow-y-auto p-3">
        <Stack gap="4">
          {/* Agent plan */}
          {plan.length > 0 ? (
            <Section icon={<ListChecks className="h-3.5 w-3.5 stroke-[1.75]" />} label="Plan">
              <Stack gap="1" className="!gap-[6px]">
                {plan.map((step) => (
                  <PlanRow key={step.id} step={step} />
                ))}
              </Stack>
            </Section>
          ) : null}

          {/* Task progress */}
          {taskProgress ? (
            <Section
              icon={<Sparkles className="h-3.5 w-3.5 stroke-[1.75] text-[var(--color-brand-purple)]" />}
              label="In flight"
            >
              <div className="rounded-[var(--radius-md)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-card)] p-2.5">
                <Inline justify="between" align="center" className="mb-1.5">
                  <span className="text-[var(--text-body-sm)] text-[var(--color-text-primary)]">
                    {taskProgress.label}
                  </span>
                  {typeof taskProgress.progress === "number" ? (
                    <span className="font-mono text-[var(--text-caption)] text-[var(--color-text-muted)]">
                      {Math.round(taskProgress.progress)}%
                    </span>
                  ) : null}
                </Inline>
                {typeof taskProgress.progress === "number" ? (
                  <div className="h-1 w-full overflow-hidden rounded-full bg-[var(--color-surface-input)]">
                    <div
                      className="h-full rounded-full bg-[var(--color-processing)] transition-[width] duration-[400ms] ease-[cubic-bezier(0.16,1,0.3,1)]"
                      style={{ width: `${Math.max(0, Math.min(100, taskProgress.progress))}%` }}
                    />
                  </div>
                ) : (
                  <div className="flex gap-1 mt-1">
                    {[0, 1, 2].map((i) => (
                      <span
                        key={i}
                        className="h-1 w-1 rounded-full bg-[var(--color-processing)] animate-pulse"
                        style={{ animationDelay: `${i * 200}ms` }}
                      />
                    ))}
                  </div>
                )}
              </div>
            </Section>
          ) : null}

          {/* Suggested next actions */}
          {suggestions.length > 0 ? (
            <Section label="Suggested">
              <Stack gap="1">
                {suggestions.map((s) => (
                  <button
                    key={s.id}
                    type="button"
                    onClick={() => onSuggestion?.(s)}
                    className="text-left rounded-[var(--radius-md)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-card)] hover:bg-[var(--color-surface-card-hover)] hover:border-[var(--color-border)] px-2.5 py-2 transition-colors"
                  >
                    <span className="text-[var(--text-body-sm)] text-[var(--color-text-primary)]">{s.label}</span>
                  </button>
                ))}
              </Stack>
            </Section>
          ) : null}

          {/* Activity log */}
          {activity.length > 0 ? (
            <Section label="Activity">
              <button
                type="button"
                onClick={() => setActivityOpen((x) => !x)}
                className="flex w-full items-center gap-1 text-[var(--text-caption)] text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)]"
              >
                {activityOpen ? (
                  <ChevronDown className="h-3 w-3 stroke-[1.75]" />
                ) : (
                  <ChevronRight className="h-3 w-3 stroke-[1.75]" />
                )}
                <span className="font-mono">
                  {activity.length} {activity.length === 1 ? "entry" : "entries"}
                </span>
              </button>
              {activityOpen ? (
                <Stack gap="1" className="mt-2">
                  {activity.map((a) => (
                    <div key={a.id} className="flex items-baseline gap-2 text-[var(--text-caption)]">
                      <span className="font-mono text-[var(--color-text-muted)] shrink-0">{a.timestamp}</span>
                      <span className="text-[var(--color-text-secondary)] leading-snug">{a.text}</span>
                    </div>
                  ))}
                </Stack>
              ) : null}
            </Section>
          ) : null}

          {plan.length === 0 && !taskProgress && suggestions.length === 0 && activity.length === 0 ? (
            <EmptyState />
          ) : null}
        </Stack>
      </div>
    </div>
  );
}

function Section({ icon, label, children }: { icon?: ReactNode; label: string; children: ReactNode }) {
  return (
    <Stack gap="2">
      <Inline gap="2" align="center">
        {icon ? <span className="text-[var(--color-text-muted)]">{icon}</span> : null}
        <span className="text-[var(--text-label)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-muted)]">
          {label}
        </span>
      </Inline>
      <div>{children}</div>
    </Stack>
  );
}

function PlanRow({ step }: { step: PlanItem }) {
  const pillStatus =
    step.status === "complete"
      ? "accepted"
      : step.status === "in_progress"
        ? "processing"
        : step.status === "failed"
          ? "failed"
          : "pending";
  return (
    <Inline gap="2" align="start" className="rounded-[var(--radius-sm)] px-1.5 py-1 hover:bg-[var(--color-surface-hover)]">
      <span className="mt-1.5 h-1.5 w-1.5 shrink-0 rounded-full" style={{
        backgroundColor:
          step.status === "complete"
            ? "var(--color-success)"
            : step.status === "in_progress"
              ? "var(--color-processing)"
              : step.status === "failed"
                ? "var(--color-danger)"
                : "var(--color-text-muted)",
      }} aria-hidden />
      <span className={cn(
        "flex-1 text-[var(--text-body-sm)] leading-snug",
        step.status === "complete"
          ? "text-[var(--color-text-muted)] line-through decoration-[var(--color-text-muted)]"
          : "text-[var(--color-text-primary)]",
      )}>
        {step.text}
      </span>
      {step.status === "in_progress" ? <Pill status={pillStatus} dot={false}>Running</Pill> : null}
      {step.status === "failed" ? <Pill status={pillStatus} dot={false}>Failed</Pill> : null}
    </Inline>
  );
}

function EmptyState() {
  return (
    <Stack gap="3" className="text-[var(--color-text-muted)] mt-2">
      <span className="text-[var(--text-caption)] leading-relaxed">
        Awidat works from your intent. Type a goal — “Cut this into a tight 8-minute podcast”, “Find 5 short clips for TikTok”, “Remove dead air but keep pacing” — and the agent will index, propose, and explain.
      </span>
      <Divider />
      <span className="text-[var(--text-micro)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold">
        Examples
      </span>
      <Stack gap="1">
        {[
          "Show me why you made this cut.",
          "Make this section slower.",
          "Replace this b-roll.",
          "Keep that pause.",
        ].map((s) => (
          <span key={s} className="text-[var(--text-caption)] text-[var(--color-text-secondary)] leading-snug">
            · {s}
          </span>
        ))}
      </Stack>
    </Stack>
  );
}

// Re-export for any consumers that want to assemble a row manually.
export { Section as CommandRailSection };

// Suppress unused import warnings (IconButton may be used by composing components later).
void IconButton;
