import {
  ChevronDown,
  ChevronRight,
  CircleStop,
  History,
  ListChecks,
  Maximize2,
  Minimize2,
  Paperclip,
  Plus,
  SendHorizontal,
  Sparkles,
  Terminal,
  X,
} from "lucide-react";
import { useState, type ReactNode } from "react";
import { Button, Divider, Inline, Pill, Stack, cn } from "../ui";

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
  detail?: string;
  kind?: "tool" | "user" | "assistant" | "result";
};

export type SuggestedAction = {
  id: string;
  label: string;
  prompt: string;
};

export type ChatSessionSummary = {
  id: string;
  title: string;
  projectRoot: string;
  logPath: string;
  startedAt: string;
  messageCount: number;
};

export type CommandRailProps = {
  /** Used to disable Send when no project is open. */
  hasProject?: boolean;
  /** Active context the agent is grounded on. */
  contextChips?: ContextChip[];
  /** What the agent intends to do for the current turn. */
  plan?: PlanItem[];
  /** Current task progress label, e.g. "Reading transcript… 67%". */
  taskProgress?: { label: string; progress?: number; eta?: string };
  /** Activity log entries — collapsed by default. */
  activity?: ActivityEntry[];
  /** Human conversation only. System jobs and tool calls do not belong here. */
  conversation?: ActivityEntry[];
  /** Suggestions the agent surfaces for the user's next move. */
  suggestions?: SuggestedAction[];
  /** Optional prefilled command for static/demo review surfaces. */
  initialDraft?: string;
  /** Saved chats for the active project. */
  chatSessions?: ChatSessionSummary[];
  /** Currently loaded chat, or null for a fresh chat. */
  activeChatSession?: ChatSessionSummary | null;
  /** True while chat history is loading. */
  chatLoading?: boolean;
  /** True when the command rail is promoted to the main workspace. */
  focused?: boolean;
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
  /** Called when the user loads a saved chat. */
  onSelectChatSession?: (session: ChatSessionSummary) => void;
  /** Called when the user starts a fresh chat. */
  onNewChat?: () => void;
  /** Called when the user enters/leaves focus mode. */
  onToggleFocus?: () => void;
};

export function CommandRail({
  hasProject = true,
  contextChips = [],
  plan = [],
  taskProgress,
  activity = [],
  conversation = [],
  suggestions = [],
  initialDraft = "",
  chatSessions = [],
  activeChatSession = null,
  chatLoading = false,
  focused = false,
  running = false,
  onSubmit,
  onCancel,
  onSuggestion,
  onRemoveChip,
  onSelectChatSession,
  onNewChat,
  onToggleFocus,
}: CommandRailProps) {
  const [draft, setDraft] = useState(initialDraft);
  const [activityOpen, setActivityOpen] = useState(false);
  const [historyOpen, setHistoryOpen] = useState(false);
  const trimmedDraft = draft.trim();
  const sendDisabledReason = !hasProject
    ? "Open a project before sending commands."
    : trimmedDraft.length === 0
      ? "Type a command to enable Send."
      : undefined;

  function submit() {
    if (!trimmedDraft || !hasProject) return;
    onSubmit?.(trimmedDraft);
    setDraft("");
  }

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="shrink-0 border-b border-[var(--color-border-subtle)] p-2">
        <Inline justify="between" align="center" gap="2">
          <button
            type="button"
            className="min-w-0 flex-1 rounded-[var(--radius-sm)] px-2 py-1 text-left transition-colors hover:bg-[var(--color-surface-hover)]"
            onClick={() => setHistoryOpen((open) => !open)}
            disabled={chatLoading}
            aria-expanded={historyOpen}
            title="Chat history"
          >
            <span className="block truncate text-[var(--text-caption)] font-semibold text-[var(--color-text-primary)]">
              {chatLoading ? "Loading chats..." : activeChatSession?.title ?? "New chat"}
            </span>
            <span className="block truncate font-mono text-[10px] text-[var(--color-text-muted)]">
              {activeChatSession
                ? `${activeChatSession.messageCount} messages`
                : "Fresh context"}
            </span>
          </button>
          <Inline gap="1" align="center" className="shrink-0">
            <Button
              variant="ghost"
              size="sm"
              disabled={chatLoading}
              onClick={() => setHistoryOpen((open) => !open)}
              leadingIcon={<History className="h-3.5 w-3.5 stroke-[1.75]" />}
              title="Past chats"
            >
              History
            </Button>
            <Button
              variant="ghost"
              size="sm"
              disabled={running || chatLoading}
              onClick={onNewChat}
              leadingIcon={<Plus className="h-3.5 w-3.5 stroke-[1.75]" />}
              title="New chat"
            >
              New
            </Button>
            {onToggleFocus ? (
              <Button
                variant="ghost"
                size="sm"
                onClick={onToggleFocus}
                leadingIcon={
                  focused ? (
                    <Minimize2 className="h-3.5 w-3.5 stroke-[1.75]" />
                  ) : (
                    <Maximize2 className="h-3.5 w-3.5 stroke-[1.75]" />
                  )
                }
                title={focused ? "Restore workspace" : "Focus mode"}
              >
                {focused ? "Restore" : "Focus"}
              </Button>
            ) : null}
          </Inline>
        </Inline>
        {historyOpen ? (
          <div className="mt-2 max-h-64 overflow-y-auto rounded-[var(--radius-md)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-card)] p-1 shadow-[var(--shadow-md)]">
            <button
              type="button"
              className={cn(
                "flex w-full min-w-0 items-center justify-between gap-2 rounded-[var(--radius-sm)] px-2 py-1.5 text-left transition-colors hover:bg-[var(--color-surface-hover)]",
                activeChatSession === null ? "bg-[var(--color-surface-selected)]" : "",
              )}
              disabled={running || chatLoading}
              onClick={() => {
                onNewChat?.();
                setHistoryOpen(false);
              }}
            >
              <span className="min-w-0 truncate text-[var(--text-caption)] font-semibold text-[var(--color-text-primary)]">
                New chat
              </span>
              <span className="shrink-0 font-mono text-[10px] text-[var(--color-text-muted)]">
                ready
              </span>
            </button>
            {chatSessions.map((session) => (
              <button
                key={session.logPath}
                type="button"
                className={cn(
                  "mt-1 flex w-full min-w-0 items-center justify-between gap-2 rounded-[var(--radius-sm)] px-2 py-1.5 text-left transition-colors hover:bg-[var(--color-surface-hover)]",
                  activeChatSession?.logPath === session.logPath
                    ? "bg-[var(--color-surface-selected)]"
                    : "",
                )}
                disabled={running || chatLoading}
                onClick={() => {
                  onSelectChatSession?.(session);
                  setHistoryOpen(false);
                }}
              >
                <span className="min-w-0 truncate text-[var(--text-caption)] font-semibold text-[var(--color-text-primary)]">
                  {session.title}
                </span>
                <span className="shrink-0 font-mono text-[10px] text-[var(--color-text-muted)]">
                  {formatChatDate(session.startedAt)}
                </span>
              </button>
            ))}
            {!chatLoading && chatSessions.length === 0 ? (
              <p className="px-2 py-2 text-[var(--text-caption)] text-[var(--color-text-muted)]">
                No saved chats for this project yet.
              </p>
            ) : null}
          </div>
        ) : null}
      </div>

      {/* Composer */}
      <div className="shrink-0 p-3 border-b border-[var(--color-border-subtle)]">
        <Stack gap="3">
          <Inline gap="2" align="center">
            <Terminal className="h-3.5 w-3.5 stroke-[1.75] text-[var(--color-brand-secondary)]" />
            <span className="text-[var(--text-label)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-secondary)]">
              Agent Command
            </span>
          </Inline>
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
            <Inline justify="between" align="center" gap="2" className="px-2 py-1.5 border-t border-[var(--color-border-subtle)]">
              <span
                className={cn(
                  "min-w-0 truncate text-[var(--text-micro)] font-semibold",
                  sendDisabledReason ? "text-[var(--color-text-muted)]" : "text-[var(--color-text-secondary)]",
                )}
                title={sendDisabledReason ?? "Command-Enter sends the command."}
              >
                {sendDisabledReason ?? "⌘↩ sends"}
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
                  disabled={Boolean(sendDisabledReason)}
                  onClick={submit}
                  trailingIcon={<SendHorizontal className="h-3.5 w-3.5 stroke-[1.75]" />}
                  title={sendDisabledReason}
                >
                  Send
                </Button>
              )}
            </Inline>
          </div>
          {contextChips.length > 0 ? (
            <div className="flex min-w-0 flex-col gap-1">
              {contextChips.map((chip, i) => (
                <button
                  key={`${chip.label}-${i}`}
                  type="button"
                  onClick={() => onRemoveChip?.(chip, i)}
                  className={cn(
                    "flex min-h-5 w-full min-w-0 items-center gap-1 rounded-[var(--radius-xs)] border px-1.5 py-0.5 text-left text-[var(--text-caption)] hover:text-[var(--color-text-primary)] transition-colors",
                    contextChipClass(chip.kind),
                  )}
                  aria-label={`Remove ${chip.label}`}
                  title={`Remove ${chip.label}`}
                >
                  <Paperclip className="h-3 w-3 shrink-0 stroke-[1.75]" />
                  <span className="min-w-0 flex-1 truncate">{chip.label}</span>
                  {onRemoveChip ? <X className="h-3 w-3 shrink-0 stroke-[1.75] opacity-70" /> : null}
                </button>
              ))}
            </div>
          ) : null}
        </Stack>
      </div>

      {/* Scroll region: plan, progress, activity, suggestions */}
      <div className="min-h-0 flex-1 overflow-y-auto p-3">
        <Stack gap="3">
          {/* Task progress */}
          {taskProgress ? (
            <Section
              icon={<Sparkles className="h-3.5 w-3.5 stroke-[1.75] text-[var(--color-brand-secondary)]" />}
              label="Agent plan"
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
                      className="h-full rounded-full bg-[var(--color-brand-secondary)] transition-[width] duration-[400ms] ease-[cubic-bezier(0.16,1,0.3,1)]"
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
                {taskProgress.eta ? (
                  <Inline justify="between" align="center" className="mt-2">
                    <span className="text-[var(--text-caption)] text-[var(--color-text-muted)]">
                      Est. time remaining
                    </span>
                    <span className="font-mono text-[var(--text-caption)] text-[var(--color-text-secondary)]">
                      {taskProgress.eta}
                    </span>
                  </Inline>
                ) : null}
              </div>
            </Section>
          ) : null}

          {/* Agent plan */}
          {plan.length > 0 ? (
            <Section icon={<ListChecks className="h-3.5 w-3.5 stroke-[1.75]" />} label={taskProgress ? "Steps" : "Plan"}>
              <Stack gap="1" className="!gap-[6px]">
                {plan.map((step) => (
                  <PlanRow key={step.id} step={step} />
                ))}
              </Stack>
            </Section>
          ) : null}

          {/* Conversation */}
          {conversation.length > 0 ? (
            <Section label="Conversation">
              <Stack gap="1" className="max-h-[320px] overflow-y-auto pr-1">
                {conversation.map((message) => (
                  <div
                    key={message.id}
                    className={cn(
                      "rounded-[var(--radius-sm)] border px-2.5 py-2 text-[var(--text-caption)]",
                      message.kind === "user"
                        ? "border-[rgba(32,201,151,0.28)] bg-[rgba(32,201,151,0.07)]"
                        : "border-[var(--color-border-subtle)] bg-[var(--color-surface-card)]",
                    )}
                  >
                    <div className="flex min-w-0 items-baseline gap-2">
                      <span className="shrink-0 font-mono text-[var(--color-text-muted)]">
                        {message.timestamp}
                      </span>
                      <span className="shrink-0 text-[10px] font-semibold uppercase tracking-[0.04em] text-[var(--color-text-muted)]">
                        {message.kind === "user" ? "You" : "Agent"}
                      </span>
                    </div>
                    <p className="mt-1 whitespace-pre-wrap break-words leading-snug text-[var(--color-text-secondary)]">
                      {message.text}
                    </p>
                  </div>
                ))}
              </Stack>
            </Section>
          ) : null}

          {/* Activity log */}
          {activity.length > 0 ? (
            <Section label="System activity">
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
                <Stack gap="1" className="mt-2 max-h-[360px] overflow-y-auto pr-1">
                  {activity.map((a) => (
                    <div
                      key={a.id}
                      className={cn(
                        "rounded-[var(--radius-sm)] border px-2 py-1.5 text-[var(--text-caption)]",
                        a.kind === "result"
                          ? "border-[var(--color-border-subtle)] bg-[var(--color-surface-input)]"
                          : "border-transparent bg-transparent",
                      )}
                    >
                      <div className="flex min-w-0 items-baseline gap-2">
                        <span className="shrink-0 font-mono text-[var(--color-text-muted)]">{a.timestamp}</span>
                        <span className="min-w-0 truncate text-[var(--color-text-secondary)] leading-snug">{a.text}</span>
                      </div>
                      {a.detail ? (
                        <p className="mt-1 line-clamp-3 pl-[3.25rem] leading-snug text-[var(--color-text-muted)]">
                          {a.detail}
                        </p>
                      ) : null}
                    </div>
                  ))}
                </Stack>
              ) : null}
            </Section>
          ) : null}

          {/* Suggested next actions */}
          {suggestions.length > 0 ? (
            <Section label="Suggested next actions">
              <Stack gap="1">
                {suggestions.map((s) => (
                  <button
                    key={s.id}
                    type="button"
                    onClick={() => onSuggestion?.(s)}
                    className="text-left rounded-[var(--radius-md)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-card)] hover:bg-[var(--color-surface-card-hover)] hover:border-[var(--color-border)] px-2.5 py-2 transition-colors"
                  >
                    <span className="text-[var(--text-body-sm)] text-[var(--color-text-primary)]">{s.label}</span>
                    <span className="mt-0.5 block text-[var(--text-caption)] text-[var(--color-text-muted)]">
                      {s.prompt}
                    </span>
                  </button>
                ))}
              </Stack>
            </Section>
          ) : null}

          {plan.length === 0 && !taskProgress && suggestions.length === 0 && conversation.length === 0 && activity.length === 0 ? (
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
    <Stack gap="2" className="mt-2 rounded-[var(--radius-md)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-card)] p-2.5 text-[var(--color-text-muted)]">
      <span className="text-[var(--text-caption)] leading-relaxed">
        Type an editing goal and Awidat will use the attached project, clip, and timeline context.
      </span>
      <Divider />
      <div className="grid gap-1">
        {[
          "Show why this cut was made.",
          "Make this section slower.",
          "Replace this b-roll.",
        ].map((s) => (
          <span key={s} className="text-[var(--text-caption)] text-[var(--color-text-secondary)] leading-snug">
            · {s}
          </span>
        ))}
      </div>
    </Stack>
  );
}

function contextChipClass(kind: ContextChip["kind"]) {
  switch (kind) {
    case "media":
    case "selection":
      return "border-[rgba(59,130,246,0.36)] bg-[rgba(59,130,246,0.08)] text-[var(--color-pill-proposed-text)]";
    case "project":
      return "border-[rgba(32,201,151,0.34)] bg-[rgba(32,201,151,0.08)] text-[var(--color-pill-ready-text)]";
    case "lens":
      return "border-[rgba(168,85,247,0.34)] bg-[rgba(168,85,247,0.08)] text-[var(--color-pill-reviewing-text)]";
    default:
      return "border-[var(--color-border-subtle)] bg-[var(--color-surface-card)] text-[var(--color-text-secondary)]";
  }
}

function formatChatDate(value: string): string {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return "";
  return date.toLocaleDateString(undefined, { month: "short", day: "numeric" });
}

// Re-export for any consumers that want to assemble a row manually.
export { Section as CommandRailSection };
