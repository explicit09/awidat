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
} from "lucide-react";
import { useEffect, useState, type ReactNode } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { Button, Divider, Inline, Pill, Stack, cn } from "../ui";
import type { PermissionMode } from "../protocol";

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
  /** Current agent permission mode. Drives the composer-footer chip. */
  permissionMode?: PermissionMode;
  /** Called when the user picks a new permission mode from the chip. */
  onSetPermissionMode?: (mode: PermissionMode) => void;
  /** Called when the user renames a chat from the context menu. */
  onRenameChat?: (session: ChatSessionSummary, newTitle: string) => Promise<void> | void;
  /** Called when the user deletes a chat from the context menu. */
  onDeleteChat?: (session: ChatSessionSummary) => Promise<void> | void;
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
  permissionMode,
  onSetPermissionMode,
  onRenameChat,
  onDeleteChat,
}: CommandRailProps) {
  const [draft, setDraft] = useState(initialDraft);
  const [activityOpen, setActivityOpen] = useState(false);
  const [historyOpen, setHistoryOpen] = useState(false);
  // Log-path of the chat whose row-actions menu is open. Only one
  // menu is open at a time across both the history dropdown and the
  // focused sidebar.
  const [menuFor, setMenuFor] = useState<string | null>(null);
  // Log-path of the chat being inline-renamed; `null` when no rename
  // is in flight. The matching draft is held alongside so the input
  // is controlled.
  const [renameFor, setRenameFor] = useState<string | null>(null);
  const [renameDraft, setRenameDraft] = useState("");

  // Close the chat-row menu when the user clicks anywhere outside it.
  // The menu items stopPropagation, so this only fires for true
  // outside clicks. Escape also closes — pairs with Esc canceling
  // an inline rename so the user has one consistent dismissal key.
  useEffect(() => {
    if (menuFor === null) return;
    function onDoc(e: MouseEvent) {
      const target = e.target as HTMLElement | null;
      if (target?.closest(".awidat-chat-row-menu")) return;
      if (target?.closest(".awidat-chat-row-more")) return;
      setMenuFor(null);
    }
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") setMenuFor(null);
    }
    document.addEventListener("mousedown", onDoc);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDoc);
      document.removeEventListener("keydown", onKey);
    };
  }, [menuFor]);
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

  // Helper: render a chat row for a given session in either the
  // dropdown or the focused sidebar. Closes over the rename/menu
  // state declared at the top of the component so both call sites
  // stay in sync (only one menu open at a time, one rename in flight).
  const renderChatRow = (
    session: ChatSessionSummary,
    variant: "dropdown" | "sidebar",
    onClick: () => void,
  ) => (
    <ChatRow
      key={session.logPath}
      session={session}
      isActive={activeChatSession?.logPath === session.logPath}
      disabled={running || chatLoading}
      onClick={onClick}
      menuOpen={menuFor === session.logPath}
      onOpenMenu={() => setMenuFor(session.logPath)}
      onCloseMenu={() => setMenuFor(null)}
      isRenaming={renameFor === session.logPath}
      renameDraft={renameDraft}
      setRenameDraft={setRenameDraft}
      beginRename={() => {
        setRenameDraft(session.title);
        setRenameFor(session.logPath);
      }}
      commitRename={async () => {
        const next = renameDraft.trim();
        setRenameFor(null);
        if (!onRenameChat) return;
        if (next.length === 0 || next === session.title) return;
        try {
          await onRenameChat(session, next);
        } catch (e) {
          // eslint-disable-next-line no-console
          console.warn("rename chat failed", e);
        }
      }}
      cancelRename={() => setRenameFor(null)}
      onDelete={async () => {
        if (!onDeleteChat) return;
        const ok = window.confirm(`Delete "${session.title}"? This can't be undone.`);
        if (!ok) return;
        try {
          await onDeleteChat(session);
        } catch (e) {
          // eslint-disable-next-line no-console
          console.warn("delete chat failed", e);
        }
      }}
      variant={variant}
    />
  );

  const sessionChrome = (
    <div className={cn("shrink-0 px-3 py-2", focused ? "" : "border-b border-[var(--color-border-subtle)]")}>
      <Inline justify="between" align="center" gap="2">
        {/* Single tab: title + chevron. Click toggles history (which
            contains "+ New chat" at the top). Replaces the old
            title + History + New + Focus button cluster. */}
        <button
          type="button"
          className={cn(
            "group min-w-0 flex-1 rounded-[var(--radius-md)] px-2 py-1.5 text-left",
            "transition-[background-color,border-color] duration-[120ms]",
            "border border-transparent hover:bg-[var(--color-surface-card)]",
          )}
          onClick={() => setHistoryOpen((open) => !open)}
          disabled={chatLoading}
          aria-expanded={historyOpen}
          title="Chats"
        >
          <Inline gap="2" align="center" className="min-w-0">
            <span className="min-w-0 truncate text-[var(--text-body-sm)] font-medium text-[var(--color-text-primary)]">
              {chatLoading ? "Loading chats…" : activeChatSession?.title ?? "New chat"}
            </span>
            <ChevronDown
              className={cn(
                "h-3 w-3 shrink-0 stroke-[1.75] text-[var(--color-text-muted)] transition-transform duration-[120ms]",
                historyOpen ? "rotate-180" : "",
              )}
              aria-hidden
            />
          </Inline>
        </button>
        <Inline gap="0" align="center" className="shrink-0">
          <button
            type="button"
            onClick={onNewChat}
            disabled={running || chatLoading}
            className="inline-flex h-7 w-7 shrink-0 items-center justify-center rounded-[var(--radius-sm)] text-[var(--color-text-muted)] hover:bg-[var(--color-surface-card)] hover:text-[var(--color-text-primary)] transition-colors disabled:opacity-40 disabled:hover:bg-transparent"
            title="New chat"
            aria-label="New chat"
          >
            <Plus className="h-3.5 w-3.5 stroke-[1.75]" />
          </button>
          <button
            type="button"
            onClick={() => setHistoryOpen((open) => !open)}
            disabled={chatLoading}
            aria-expanded={historyOpen}
            className="inline-flex h-7 w-7 shrink-0 items-center justify-center rounded-[var(--radius-sm)] text-[var(--color-text-muted)] hover:bg-[var(--color-surface-card)] hover:text-[var(--color-text-primary)] transition-colors disabled:opacity-40 disabled:hover:bg-transparent"
            title="Chat history"
            aria-label="Chat history"
          >
            <History className="h-3.5 w-3.5 stroke-[1.75]" />
          </button>
          {onToggleFocus ? (
            <button
              type="button"
              onClick={onToggleFocus}
              className="inline-flex h-7 w-7 shrink-0 items-center justify-center rounded-[var(--radius-sm)] text-[var(--color-text-muted)] hover:bg-[var(--color-surface-card)] hover:text-[var(--color-text-primary)] transition-colors"
              title={focused ? "Restore workspace" : "Focus mode"}
              aria-label={focused ? "Restore workspace" : "Focus mode"}
            >
              {focused ? (
                <Minimize2 className="h-3.5 w-3.5 stroke-[1.75]" />
              ) : (
                <Maximize2 className="h-3.5 w-3.5 stroke-[1.75]" />
              )}
            </button>
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
          {chatSessions.map((session) =>
            renderChatRow(session, "dropdown", () => {
              onSelectChatSession?.(session);
              setHistoryOpen(false);
            }),
          )}
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
            "awidat-composer-card rounded-[var(--radius-md)] transition-colors",
            focused ? "shadow-[0_18px_70px_rgba(0,0,0,0.3)]" : "",
          )}
        >
          <textarea
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={(e) => {
              // Enter sends. Shift/Option/Cmd/Ctrl+Enter inserts a
              // newline (Cmd+Enter still sends for muscle-memory
              // power users). IME composition guards prevent
              // accidental sends mid-input on Asian keyboards.
              if (e.key !== "Enter" || e.nativeEvent.isComposing) return;
              if (e.shiftKey || e.altKey) return;
              e.preventDefault();
              submit();
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
              {permissionMode && onSetPermissionMode ? (
                <PermissionModeChip
                  mode={permissionMode}
                  onChange={onSetPermissionMode}
                />
              ) : null}
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
                variant="ghost"
                size="sm"
                disabled={Boolean(sendDisabledReason)}
                onClick={submit}
                trailingIcon={<SendHorizontal className="h-3.5 w-3.5 stroke-[1.75]" />}
                title={sendDisabledReason}
                className="awidat-send-button"
              >
                Send
              </Button>
            )}
          </Inline>
        </div>
        {(() => {
          // Filter out the Project chip — the chat is already scoped to
          // the open project (the rail header shows the title), so
          // restating it adds noise without information.
          const visibleChips = contextChips.filter((c) => c.kind !== "project");
          if (visibleChips.length === 0) return null;
          return (
            <div className="awidat-context-strip">
              {visibleChips.map((chip, i) => (
                <span
                  key={`${chip.label}-${i}`}
                  className="awidat-context-item"
                  title={chip.label}
                >
                  <Paperclip className="h-3 w-3 shrink-0 stroke-[1.75] opacity-70" />
                  <span className="truncate max-w-[200px]">{shortenChip(chip.label)}</span>
                </span>
              ))}
              {onRemoveChip ? (
                <button
                  type="button"
                  onClick={() => {
                    // Clear all visible chips in one shot. Walking
                    // backward keeps indices stable across the removes.
                    for (let i = visibleChips.length - 1; i >= 0; i--) {
                      const chip = visibleChips[i];
                      const originalIdx = contextChips.indexOf(chip);
                      onRemoveChip(chip, originalIdx);
                    }
                  }}
                  className="awidat-context-clear"
                  aria-label="Clear attached context"
                  title="Clear attached context"
                >
                  ×
                </button>
              ) : null}
            </div>
          );
        })()}
      </Stack>
    </div>
  );

  // Focused mode = full-window chat. We split into a 240px left sidebar
  // (chat list, always visible) + a centered conversation column capped
  // at ~760px so long messages don't sprawl. Matches Cursor's full-chat
  // surface: chats on the left, conversation centered, composer below.
  if (focused) {
    return (
      <div className="awidat-chat-rail flex h-full min-h-0 w-full bg-[var(--color-surface-page)]">
        <FocusedSidebar
          chatSessions={chatSessions}
          activeChatSession={activeChatSession}
          chatLoading={chatLoading}
          running={running}
          onNewChat={onNewChat}
          onToggleFocus={onToggleFocus}
          renderRow={(session) =>
            renderChatRow(session, "sidebar", () => onSelectChatSession?.(session))
          }
        />
        <div className="flex min-h-0 min-w-0 flex-1 flex-col">
          <div className="mx-auto flex w-full min-h-0 max-w-[760px] flex-1 flex-col">
            <div
              className={cn(
                "min-h-0 flex-1 overflow-y-auto p-3",
                !hasWork ? "flex items-center justify-center" : "",
              )}
            >
              {!hasWork ? (
                <div className="w-full max-w-[720px]">{composer}</div>
              ) : (
                <Stack gap="5">
                  {turns.length > 0 ? (
                    <Section label="Conversation">
                      <div className="flex flex-col gap-5">
                        {turns.map((turn) => (
                          <ConversationTurnBlock
                            key={turn.id}
                            turn={turn}
                            showSeparator={false}
                          />
                        ))}
                      </div>
                    </Section>
                  ) : null}
                </Stack>
              )}
            </div>
            {hasWork ? <div className="shrink-0">{composer}</div> : null}
          </div>
        </div>
      </div>
    );
  }

  return (
    <div className="awidat-chat-rail flex h-full min-h-0 flex-col">
      {sessionChrome}

      <div
        className={cn(
          "min-h-0 flex-1 overflow-y-auto p-3",
          isOnlyEmptyState ? "flex items-center justify-center" : "",
        )}
      >
        {isOnlyEmptyState ? (
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
              <div className="flex flex-col gap-5">
                {turns.map((turn) => (
                  <ConversationTurnBlock
                    key={turn.id}
                    turn={turn}
                    showSeparator={false}
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

          {/* The empty-state hint is rendered above the Stack (centered)
              when nothing else is present — no fallback render needed
              inside the Stack itself. */}
        </Stack>
        )}
      </div>
      {composer}
    </div>
  );
}

/** Left sidebar shown in focused/full-chat mode. Always-visible chat
 *  list with new-chat at the top + a back-out icon at the bottom to
 *  collapse back to the cockpit layout. */
function FocusedSidebar({
  chatSessions,
  activeChatSession,
  chatLoading,
  running,
  onNewChat,
  onToggleFocus,
  renderRow,
}: {
  chatSessions: ChatSessionSummary[];
  activeChatSession: ChatSessionSummary | null;
  chatLoading: boolean;
  running: boolean;
  onNewChat?: () => void;
  onToggleFocus?: () => void;
  renderRow: (session: ChatSessionSummary) => ReactNode;
}) {
  return (
    <aside className="awidat-focused-sidebar">
      <div className="awidat-focused-sidebar-header">
        <button
          type="button"
          onClick={onNewChat}
          disabled={running || chatLoading}
          className="awidat-focused-sidebar-new"
          title="New chat"
        >
          <Plus className="h-3.5 w-3.5 stroke-[1.75]" />
          <span>New chat</span>
        </button>
      </div>
      <div className="awidat-focused-sidebar-list">
        <button
          type="button"
          className={cn(
            "awidat-focused-sidebar-item",
            activeChatSession === null ? "is-active" : "",
          )}
          disabled={running || chatLoading}
          onClick={() => onNewChat?.()}
        >
          <span className="truncate">New chat</span>
          <span className="awidat-focused-sidebar-meta">fresh</span>
        </button>
        {chatSessions.map((session) => renderRow(session))}
        {!chatLoading && chatSessions.length === 0 ? (
          <p className="awidat-focused-sidebar-empty">No saved chats yet.</p>
        ) : null}
      </div>
      {onToggleFocus ? (
        <div className="awidat-focused-sidebar-footer">
          <button
            type="button"
            onClick={onToggleFocus}
            className="awidat-focused-sidebar-footer-button"
            title="Restore workspace"
          >
            <Minimize2 className="h-3.5 w-3.5 stroke-[1.75]" />
            <span>Exit focus</span>
          </button>
        </div>
      ) : null}
    </aside>
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
            data-chat-user
            className={cn(
              "max-w-[85%] whitespace-pre-wrap break-words leading-relaxed",
              "rounded-[14px] px-3.5 py-2",
              "text-[14px] text-[var(--color-text-primary)]",
              "bg-[var(--color-surface-selected)]",
            )}
          >
            {turn.userText}
          </p>
        </div>
      ) : null}
      {turn.parts.length > 0 ? (
        <div className="flex flex-col gap-3">
          {turn.parts.map((part) =>
            part.kind === "text" ? (
              <div
                key={part.id}
                className="markdown w-full break-words text-[14px] leading-[1.6] text-[var(--color-text-primary)]"
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
  const starters = [
    "Cut this into a 60-second highlight reel.",
    "Remove silences and filler words.",
    "Find the strongest opening hook.",
    "Show why each cut was made.",
  ];
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

/**
 * Compress a long chip label so the inline context strip never wraps.
 * Strategy:
 *   - "Clip: copy_F65206FA-9AEC-...F7FA" → "Clip … F7FA"
 *   - "Timeline: 2:49"                    → "Playhead 2:49"
 *   - everything else: pass through.
 *
 * The mapping is intentional, not algorithmic — these are the labels
 * the agent attaches today, and the goal is human-readable shorthand
 * the user can scan in 100ms instead of decoding a UUID.
 */
/** Composer-footer pill that exposes the agent's permission mode.
 *  Labels chosen for compactness — the tooltip carries the full
 *  meaning so the user gets a one-glance read of the chip and a
 *  full sentence on hover. */
function PermissionModeChip({
  mode,
  onChange,
}: {
  mode: PermissionMode;
  onChange: (next: PermissionMode) => void;
}) {
  const labels: Record<PermissionMode, string> = {
    manual: "Manual",
    copilot: "Copilot",
    autopilot: "Auto",
  };
  const titles: Record<PermissionMode, string> = {
    manual: "Manual — every proposal needs explicit Accept.",
    copilot: "Copilot — agent surfaces notes; you ask it to act.",
    autopilot: "Auto — agent applies edits without approval cards.",
  };
  return (
    <label className="awidat-mode-chip" title={titles[mode]}>
      <span className="awidat-mode-chip-dot" data-mode={mode} aria-hidden />
      <span className="awidat-mode-chip-label">{labels[mode]}</span>
      <select
        value={mode}
        onChange={(e) => onChange(e.target.value as PermissionMode)}
        aria-label="Agent permission mode"
      >
        <option value="manual">Manual</option>
        <option value="copilot">Copilot</option>
        <option value="autopilot">Auto</option>
      </select>
    </label>
  );
}

/** Row representing one saved chat. Click loads it; right-click opens
 *  a context menu with Rename / Delete. Used by both the in-cockpit
 *  history dropdown and the focused-mode sidebar. */
function ChatRow({
  session,
  isActive,
  disabled,
  onClick,
  onOpenMenu,
  onCloseMenu,
  menuOpen,
  isRenaming,
  renameDraft,
  setRenameDraft,
  commitRename,
  cancelRename,
  beginRename,
  onDelete,
  variant,
}: {
  session: ChatSessionSummary;
  isActive: boolean;
  disabled: boolean;
  onClick: () => void;
  onOpenMenu: () => void;
  onCloseMenu: () => void;
  menuOpen: boolean;
  isRenaming: boolean;
  renameDraft: string;
  setRenameDraft: (s: string) => void;
  commitRename: () => void;
  cancelRename: () => void;
  beginRename: () => void;
  onDelete: () => void;
  variant: "dropdown" | "sidebar";
}) {
  const baseClass =
    variant === "sidebar"
      ? cn("awidat-focused-sidebar-item awidat-chat-row", isActive ? "is-active" : "")
      : cn(
          "awidat-chat-row mt-1 flex w-full min-w-0 items-center justify-between gap-2 rounded-[var(--radius-sm)] px-2 py-1.5 text-left transition-colors hover:bg-[var(--color-surface-hover)]",
          isActive ? "bg-[var(--color-surface-selected)]" : "",
        );
  if (isRenaming) {
    return (
      <div className={baseClass}>
        <input
          autoFocus
          value={renameDraft}
          onChange={(e) => setRenameDraft(e.target.value)}
          onBlur={commitRename}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              commitRename();
            }
            if (e.key === "Escape") {
              e.preventDefault();
              cancelRename();
            }
            // Don't bubble Enter/Escape up to global handlers.
            e.stopPropagation();
          }}
          className="awidat-chat-row-rename"
          aria-label="Rename chat"
        />
      </div>
    );
  }
  return (
    <div className="awidat-chat-row-wrapper">
      <button
        type="button"
        className={baseClass}
        disabled={disabled}
        onClick={onClick}
        onContextMenu={(e) => {
          e.preventDefault();
          onOpenMenu();
        }}
      >
        <span className="truncate min-w-0 text-[var(--text-caption)] font-semibold text-[var(--color-text-primary)]">
          {session.title}
        </span>
        <span
          className={
            variant === "sidebar"
              ? "awidat-focused-sidebar-meta"
              : "shrink-0 font-mono text-[10px] text-[var(--color-text-muted)]"
          }
        >
          {formatChatDate(session.startedAt)}
        </span>
      </button>
      <button
        type="button"
        className="awidat-chat-row-more"
        onClick={(e) => {
          e.stopPropagation();
          onOpenMenu();
        }}
        aria-label="More actions"
        title="More actions"
      >
        …
      </button>
      {menuOpen ? (
        <div className="awidat-chat-row-menu" role="menu" onClick={(e) => e.stopPropagation()}>
          <button
            type="button"
            role="menuitem"
            onClick={() => {
              onCloseMenu();
              beginRename();
            }}
          >
            Rename
          </button>
          <button
            type="button"
            role="menuitem"
            className="awidat-chat-row-menu-danger"
            onClick={() => {
              onCloseMenu();
              onDelete();
            }}
          >
            Delete
          </button>
        </div>
      ) : null}
    </div>
  );
}

function shortenChip(raw: string): string {
  const trimmed = raw.trim();
  const tlMatch = trimmed.match(/^Timeline:\s*(.+)$/i);
  if (tlMatch) return `Playhead ${tlMatch[1]}`;
  const clipMatch = trimmed.match(/^Clip:\s*(.+)$/i);
  if (clipMatch) {
    const value = clipMatch[1];
    if (value.length > 14) {
      return `Clip … ${value.slice(-8)}`;
    }
    return `Clip ${value}`;
  }
  return trimmed;
}

function formatChatDate(value: string): string {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return "";
  return date.toLocaleDateString(undefined, { month: "short", day: "numeric" });
}

// Re-export for any consumers that want to assemble a row manually.
export { Section as CommandRailSection };
