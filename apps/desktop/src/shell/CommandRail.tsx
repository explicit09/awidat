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
  X,
} from "lucide-react";
import { useState, useEffect, useRef, type ReactNode } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { Button, Divider, Inline, Pill, Stack, cn } from "../ui";
import { AgentRailCompact } from "./AgentRailCompact";
import { STARTER_PROMPTS } from "./agentPrompts";

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

/** One agent-side output inside a conversation turn. */
export type TurnPart =
  | { kind: "text"; id: string; text: string }
  | {
      kind: "tool_call";
      id: string;
      /** Stable tool name (e.g. `view_timeline`). */
      name: string;
      status: "running" | "done" | "failed";
      /** Short, human one-liner ("Read timeline", "Found 3 beats"). */
      summary: string;
      args: unknown;
      /** `null` while running; `{ Ok: string } | { Err: string }` when done. */
      result: { Ok: string } | { Err: string } | null;
    };

/** A single user turn + the agent's interleaved tool calls and text
 *  replies. Conversation rail renders one block per turn. */
export type ConversationTurn = {
  id: string;
  userText: string;
  parts: TurnPart[];
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
  /** Activity log entries — collapsed by default. Retained for jobs
   *  that have no place inside a turn (project-load background work). */
  activity?: ActivityEntry[];
  /** One block per user turn, with the agent's tool calls and text
   *  interleaved inside it. Replaces the legacy `conversation` flat list. */
  turns?: ConversationTurn[];
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
  turns = [],
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

  // Rail-mode state: compact (default idle) vs expanded (active/recent).
  const [manuallyExpanded, setManuallyExpanded] = useState(false);
  const lastTurnAtMsRef = useRef<number | null>(null);
  useEffect(() => {
    if (turns.length > 0) lastTurnAtMsRef.current = Date.now();
  }, [turns.length]);
  // Force re-evaluation every 5s while not running so the 30s decay fires.
  const [tickN, setTickN] = useState(0);
  useEffect(() => {
    if (running) return;
    const id = setInterval(() => setTickN((n) => n + 1), 5000);
    return () => clearInterval(id);
  }, [running]);
  // When running starts, clear manual expansion so next idle goes compact.
  useEffect(() => {
    if (running) setManuallyExpanded(false);
  }, [running]);

  const railMode: "compact" | "expanded" = (() => {
    if (running) return "expanded";
    if (manuallyExpanded) return "expanded";
    if (turns.length === 0) return "compact";
    const ageMs = lastTurnAtMsRef.current ? Date.now() - lastTurnAtMsRef.current : Infinity;
    return ageMs < 30_000 ? "expanded" : "compact";
  })();
  // Suppress unused-var warning — tickN exists to force re-renders.
  void tickN;
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

  const hasWork =
    turns.length > 0 ||
    activity.length > 0 ||
    plan.length > 0 ||
    suggestions.length > 0 ||
    taskProgress !== undefined;

  // True when the rail has literally nothing to show beyond the hint
  // card. We center the hint vertically in that case instead of
  // pinning it to the top — an empty rail with a top-aligned card
  // reads as "half-loaded UI" instead of "ready, waiting for input."
  const isOnlyEmptyState = !hasWork && !focused;

  const sessionChrome = (
    <div className={cn("shrink-0 px-3 py-2", focused ? "" : "border-b border-[var(--color-border-subtle)]")}>
      <Inline justify="between" align="center" gap="2">
        <button
          type="button"
          className={cn(
            "group min-w-0 flex-1 rounded-[var(--radius-md)] px-2 py-1.5 text-left",
            "transition-[background-color,border-color] duration-[120ms]",
            "border border-transparent hover:border-[var(--color-border-subtle)] hover:bg-[var(--color-surface-card)]",
          )}
          onClick={() => setHistoryOpen((open) => !open)}
          disabled={chatLoading}
          aria-expanded={historyOpen}
          title="Chat history"
        >
          <Inline gap="2" align="center" className="min-w-0">
            <span
              className={cn(
                "h-1.5 w-1.5 shrink-0 rounded-full",
                activeChatSession
                  ? "bg-[var(--accent-selection)]"
                  : "bg-[var(--color-text-muted)] opacity-50",
              )}
              aria-hidden
            />
            <span className="min-w-0 truncate text-[var(--text-body-sm)] font-semibold text-[var(--color-text-primary)]">
              {chatLoading ? "Loading chats…" : activeChatSession?.title ?? "New chat"}
            </span>
            <span className="shrink-0 font-mono text-[10px] uppercase tracking-[0.08em] text-[var(--color-text-muted)]">
              {activeChatSession ? `${activeChatSession.messageCount} msgs` : "fresh"}
            </span>
          </Inline>
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
  );

  const composer = (
    <div
      className={cn(
        "shrink-0 p-3",
        focused ? "pb-6" : "",
      )}
    >
      <Stack gap={focused ? "2" : "2"}>
        {focused ? (
          <Inline gap="2" align="center" className="px-1">
            <span className="min-w-0 truncate text-[var(--text-caption)] text-[var(--color-text-muted)]">
              {activeChatSession?.title ?? "New chat"}
            </span>
            <span className="font-mono text-[10px] text-[var(--color-text-muted)] opacity-60">Local</span>
          </Inline>
        ) : null}
        {/* Borderless input with a focus-only hairline — reads as a
            command bar, not a form. The submit cluster below sits
            flush with the input edge so the whole thing feels like
            one continuous control. */}
        <div
          className={cn(
            "rounded-[var(--radius-md)] bg-[var(--color-surface-input)] transition-colors",
            "border border-transparent focus-within:border-[var(--color-border-focus)]",
            focused ? "shadow-[0_18px_70px_rgba(0,0,0,0.3)]" : "",
          )}
        >
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
                ? "Plan, Build, / for commands, @ for context"
                : "Open a project to begin."
            }
            rows={focused ? 4 : 3}
            disabled={!hasProject}
            className={cn(
              "w-full resize-none bg-transparent px-3 py-2.5",
              "text-[var(--text-body)] text-[var(--color-text-primary)] placeholder:text-[var(--color-text-muted)]",
              "outline-none disabled:cursor-not-allowed disabled:opacity-50",
            )}
          />
          <Inline justify="between" align="center" gap="2" className="px-2 py-1.5">
            <Inline gap="2" align="center" className="min-w-0">
              <span
                className={cn(
                  "min-w-0 truncate text-[var(--text-micro)]",
                  sendDisabledReason ? "text-[var(--color-text-muted)]" : "text-[var(--color-text-muted)] opacity-70",
                )}
                title={sendDisabledReason ?? "Command-Enter sends the command."}
              >
                {sendDisabledReason ?? "⌘↩ to send"}
              </span>
            </Inline>
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
          <div className={cn("flex min-w-0 gap-1.5", focused ? "flex-row flex-wrap" : "flex-col")}>
            {contextChips.map((chip, i) => (
              <div
                key={`${chip.label}-${i}`}
                className={cn(
                  "group flex min-h-7 min-w-0 items-center gap-1.5 rounded-[var(--radius-sm)] border pl-2 pr-1 py-1 text-[var(--text-caption)]",
                  "transition-[background-color,border-color] duration-[120ms]",
                  focused ? "max-w-[260px]" : "w-full",
                  contextChipClass(chip.kind),
                )}
              >
                <span
                  className={cn(
                    "h-1.5 w-1.5 shrink-0 rounded-full",
                    contextChipDotClass(chip.kind),
                  )}
                  aria-hidden
                />
                <Paperclip className="h-3 w-3 shrink-0 stroke-[1.75] opacity-70" />
                <span
                  className="min-w-0 flex-1 truncate font-mono text-[11px]"
                  title={chip.label}
                >
                  {chip.label}
                </span>
                {onRemoveChip ? (
                  <button
                    type="button"
                    onClick={() => onRemoveChip(chip, i)}
                    className={cn(
                      "ml-0.5 inline-flex h-5 w-5 shrink-0 items-center justify-center rounded-[var(--radius-xs)]",
                      "text-[var(--color-text-muted)] opacity-0 transition-opacity duration-[100ms]",
                      "group-hover:opacity-100 focus-visible:opacity-100",
                      "hover:bg-[rgba(248,81,73,0.18)] hover:text-[#ffb4ae]",
                    )}
                    aria-label={`Remove ${chip.label}`}
                    title={`Remove ${chip.label}`}
                  >
                    <X className="h-3 w-3 stroke-[2]" />
                  </button>
                ) : null}
              </div>
            ))}
          </div>
        ) : null}
      </Stack>
    </div>
  );

  return (
    <div
      className={cn(
        "flex h-full min-h-0 flex-col",
        focused ? "bg-transparent" : "",
      )}
    >
      {sessionChrome}

      <div
        className={cn(
          "min-h-0 flex-1 overflow-y-auto",
          railMode === "compact"
            ? ""
            : (focused && !hasWork) || isOnlyEmptyState
              ? "flex items-center justify-center p-3"
              : "p-3",
        )}
      >
        {railMode === "compact" ? (
          /* ── Compact mode: summary + starters + composer + expand link ── */
          <AgentRailCompact
            composer={composer}
            messageCount={turns.length}
            onExpand={() => setManuallyExpanded(true)}
            onUseSuggestion={(prompt) => setDraft(prompt)}
          />
        ) : focused && !hasWork ? (
          <div className="w-full max-w-[720px]">{composer}</div>
        ) : isOnlyEmptyState ? (
          // Nothing to do yet — vertically + horizontally centered hint
          // card so the empty rail doesn't read as a pinned-to-top
          // half-rendered panel.
          <div className="w-full max-w-[320px]">
            <EmptyState onUseSuggestion={(prompt) => setDraft(prompt)} />
          </div>
        ) : (
        <Stack gap="3">
          {/* Task progress */}
          {taskProgress ? (
            <Section
              icon={<Sparkles className="h-3.5 w-3.5 stroke-[1.75] text-[var(--accent-selection)]" />}
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
                      className="h-full rounded-full bg-[var(--accent-selection)] transition-[width] duration-[400ms] ease-[cubic-bezier(0.16,1,0.3,1)]"
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

          {/* Conversation — Cursor-style flow: speaker + timestamp
              as quiet metadata, message body as the hero. No
              bordered cards. User turns get a subtle accent in
              their label; agent turns stay neutral. Borders gone
              so the rail reads as a continuous conversation, not
              a stack of widgets. */}
          {turns.length > 0 ? (
            <Section label="Conversation">
              <div className="flex flex-col gap-4">
                {turns.map((turn, i) => (
                  <ConversationTurnBlock
                    key={turn.id}
                    turn={turn}
                    showSeparator={i > 0}
                  />
                ))}
              </div>
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

          {/* Collapse to summary — only shown when idle and has turns */}
          {!running && turns.length > 0 ? (
            <button
              type="button"
              onClick={() => {
                setManuallyExpanded(false);
                // Push timestamp into the past so decay logic keeps it compact.
                lastTurnAtMsRef.current = 0;
              }}
              className="self-start text-[var(--text-caption)] text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)] transition-colors"
            >
              Collapse to summary
            </button>
          ) : null}

          {/* The empty-state hint is rendered above the Stack (centered)
              when nothing else is present — no fallback render needed
              inside the Stack itself. */}
        </Stack>
        )}
      </div>
      {/* Composer is slotted into AgentRailCompact in compact mode;
          render it here only in expanded mode. */}
      {railMode === "compact" ? null : focused && !hasWork ? null : composer}
    </div>
  );
}

/** One user turn rendered ChatGPT-style: user pill on the right, then
 *  the agent's interleaved tool calls (collapsed cards) and text
 *  blocks stacked in the order they fired. */
function ConversationTurnBlock({
  turn,
  showSeparator,
}: {
  turn: ConversationTurn;
  showSeparator: boolean;
}) {
  return (
    <div className="flex flex-col gap-3">
      {showSeparator ? (
        <hr className="border-0 border-t border-[var(--color-border-subtle)] opacity-60" />
      ) : null}
      {turn.userText ? (
        <div className="flex min-w-0 justify-end">
          <p
            className={cn(
              "max-w-[85%] whitespace-pre-wrap break-words leading-relaxed",
              "rounded-[18px] px-3.5 py-2",
              "text-[var(--text-body-sm)] text-[var(--color-text-primary)]",
              "bg-[color-mix(in_oklab,#4c5b86_38%,var(--color-surface-card))]",
            )}
          >
            {turn.userText}
          </p>
        </div>
      ) : null}
      {turn.parts.length > 0 ? (
        <div className="flex flex-col gap-2">
          {turn.parts.map((part) =>
            part.kind === "text" ? (
              <div
                key={part.id}
                className="markdown w-full break-words leading-relaxed text-[var(--text-body-sm)] text-[var(--color-text-secondary)]"
              >
                <ReactMarkdown remarkPlugins={[remarkGfm]}>
                  {part.text}
                </ReactMarkdown>
              </div>
            ) : (
              <ToolCallRow key={part.id} part={part} />
            ),
          )}
        </div>
      ) : null}
    </div>
  );
}

/** Compact, collapsed-by-default representation of one tool call.
 *  Status dot (running/done/failed) + tool name + one-line summary.
 *  Click to expand args + result. */
function ToolCallRow({
  part,
}: {
  part: Extract<TurnPart, { kind: "tool_call" }>;
}) {
  const dotColor =
    part.status === "running"
      ? "bg-[var(--color-text-muted)] animate-pulse"
      : part.status === "failed"
        ? "bg-[#ef7168]"
        : "bg-[var(--accent-audio)]";
  return (
    <details className="group min-w-0">
      <summary className="flex min-w-0 cursor-pointer list-none items-center gap-2 py-0.5 text-[var(--text-caption)] text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)]">
        <span className={cn("h-1.5 w-1.5 shrink-0 rounded-full", dotColor)} aria-hidden />
        <span className="shrink-0 font-mono text-[10px] uppercase tracking-[0.04em] text-[var(--color-text-muted)]">
          {part.name.replace(/_/g, " ")}
        </span>
        <span className="min-w-0 truncate leading-snug">{part.summary}</span>
      </summary>
      <div className="mt-1 ml-3.5 border-l border-[var(--color-border-subtle)] pl-2.5 text-[var(--text-caption)] text-[var(--color-text-muted)]">
        {part.args !== null && part.args !== undefined ? (
          <pre className="overflow-x-auto whitespace-pre-wrap break-words font-mono text-[11px] leading-snug text-[var(--color-text-muted)]">
            {JSON.stringify(part.args, null, 2)}
          </pre>
        ) : null}
        {part.result && "Ok" in part.result ? (
          <pre className="mt-1 overflow-x-auto whitespace-pre-wrap break-words font-mono text-[11px] leading-snug text-[var(--color-text-secondary)]">
            {part.result.Ok}
          </pre>
        ) : null}
        {part.result && "Err" in part.result ? (
          <pre className="mt-1 overflow-x-auto whitespace-pre-wrap break-words font-mono text-[11px] leading-snug text-[#ef7168]">
            {part.result.Err}
          </pre>
        ) : null}
      </div>
    </details>
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

function EmptyState({
  onUseSuggestion,
}: {
  onUseSuggestion?: (prompt: string) => void;
}) {
  // Starter prompts the agent can actually act on. Click to drop the
  // text into the composer (the user can tweak before sending). These
  // replace the now-removed "Ask agent for first cut" button — moving
  // the agent-launch surface here lets the user pick *what kind* of
  // cut they want instead of getting a generic one.
  const starters = STARTER_PROMPTS;
  return (
    <Stack gap="2" className="rounded-[var(--radius-md)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-card)] p-3 text-[var(--color-text-muted)]">
      <span className="text-[var(--text-caption)] leading-relaxed">
        Type an editing goal — Awidat uses the attached project, clip, and timeline context.
      </span>
      <Divider />
      <div className="grid gap-1">
        {starters.map((s) =>
          onUseSuggestion ? (
            <button
              key={s}
              type="button"
              onClick={() => onUseSuggestion(s)}
              className="group flex items-start gap-1.5 rounded-[var(--radius-xs)] px-1 py-0.5 text-left transition-colors hover:bg-[var(--color-surface-hover)]"
            >
              <span className="mt-0.5 text-[var(--color-text-muted)] group-hover:text-[var(--accent-selection)]">
                ›
              </span>
              <span className="text-[var(--text-caption)] text-[var(--color-text-secondary)] leading-snug group-hover:text-[var(--color-text-primary)]">
                {s}
              </span>
            </button>
          ) : (
            <span
              key={s}
              className="text-[var(--text-caption)] text-[var(--color-text-secondary)] leading-snug"
            >
              · {s}
            </span>
          ),
        )}
      </div>
    </Stack>
  );
}

function contextChipClass(kind: ContextChip["kind"]) {
  switch (kind) {
    case "media":
    case "selection":
      return "border-[rgba(59,130,246,0.36)] bg-[rgba(59,130,246,0.08)] text-[var(--color-pill-proposed-text)] hover:bg-[rgba(59,130,246,0.14)]";
    case "project":
      return "border-[rgba(32,201,151,0.34)] bg-[rgba(32,201,151,0.08)] text-[var(--color-pill-ready-text)] hover:bg-[rgba(32,201,151,0.14)]";
    case "lens":
      return "border-[rgba(168,85,247,0.34)] bg-[rgba(168,85,247,0.08)] text-[var(--color-pill-reviewing-text)] hover:bg-[rgba(168,85,247,0.14)]";
    default:
      return "border-[var(--color-border-subtle)] bg-[var(--color-surface-card)] text-[var(--color-text-secondary)] hover:bg-[var(--color-surface-card-hover)]";
  }
}

/**
 * Solid-color dot prefix per chip kind so the eye can scan a stack of
 * chips by color even before reading the label. Matches the
 * border/background tints used by [`contextChipClass`].
 */
function contextChipDotClass(kind: ContextChip["kind"]) {
  switch (kind) {
    case "media":
    case "selection":
      return "bg-[rgb(96,165,250)]";
    case "project":
      return "bg-[rgb(74,222,128)]";
    case "lens":
      return "bg-[rgb(192,132,252)]";
    default:
      return "bg-[var(--color-text-muted)]";
  }
}

function formatChatDate(value: string): string {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return "";
  return date.toLocaleDateString(undefined, { month: "short", day: "numeric" });
}

// Re-export for any consumers that want to assemble a row manually.
export { Section as CommandRailSection };
