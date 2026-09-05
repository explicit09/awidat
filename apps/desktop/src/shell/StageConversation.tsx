import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { History, Plus, Sparkles } from "lucide-react";
import { ChatStream } from "../agent/ChatStream";
import { AGENT_PROFILE_OPTIONS } from "../agent/agentProfile";
import type { AgentProfile } from "../protocol/generated/AgentProfile";
import type { PermissionMode } from "../protocol/generated/PermissionMode";
import type { ChatSessionSummary, MediaSuggestion } from "./CommandRail";

type ConversationPanelProps = {
  agentRead?: string;
  draft: string;
  running: boolean;
  onDraft: (value: string) => void;
  onSubmit: () => void;
  onCancel: () => void;
  mediaSuggestions?: MediaSuggestion[];
  onPickMedia?: (suggestion: MediaSuggestion) => void;
  chatSessions?: ChatSessionSummary[];
  activeChatSession?: ChatSessionSummary | null;
  chatLoading?: boolean;
  onOpenHistory?: () => void;
  onSelectChatSession?: (session: ChatSessionSummary) => void;
  onNewChat?: () => void;
  permissionMode?: PermissionMode;
  onSetPermissionMode?: (mode: PermissionMode) => void;
  agentProfile?: AgentProfile;
  onSetAgentProfile?: (profile: AgentProfile) => void;
};

export function ConversationPanel({
  agentRead,
  draft,
  running,
  onDraft,
  onSubmit,
  onCancel,
  mediaSuggestions = [],
  onPickMedia,
  chatSessions = [],
  activeChatSession = null,
  chatLoading = false,
  onOpenHistory,
  onSelectChatSession,
  onNewChat,
  permissionMode = "manual",
  onSetPermissionMode,
  agentProfile = "balanced",
  onSetAgentProfile,
}: ConversationPanelProps) {
  const inputRef = useRef<HTMLTextAreaElement | null>(null);
  const [mention, setMention] = useState<{ start: number; query: string } | null>(null);
  const [mentionIdx, setMentionIdx] = useState(0);
  const [historyOpen, setHistoryOpen] = useState(false);

  const filteredMedia = useMemo(() => {
    if (mention === null) return [];
    const query = mention.query.trim().toLowerCase();
    return mediaSuggestions
      .filter((suggestion) => {
        if (!query) return true;
        return (
          suggestion.label.toLowerCase().includes(query) ||
          suggestion.token.toLowerCase().includes(query)
        );
      })
      .slice(0, 8);
  }, [mediaSuggestions, mention]);

  useEffect(() => {
    setMentionIdx(0);
  }, [mention?.query, mention?.start]);

  useLayoutEffect(() => {
    const input = inputRef.current;
    if (!input) return;
    input.style.height = "0px";
    const nextHeight = Math.min(132, Math.max(38, input.scrollHeight));
    input.style.height = `${nextHeight}px`;
    input.style.overflowY = input.scrollHeight > 132 ? "auto" : "hidden";
  }, [draft]);

  function toggleHistory() {
    setHistoryOpen((open) => {
      const next = !open;
      if (next) onOpenHistory?.();
      return next;
    });
  }

  function syncMentionFromCaret() {
    const input = inputRef.current;
    if (!input) return;
    const caret = input.selectionStart;
    const text = input.value;
    const at = text.lastIndexOf("@", Math.max(0, caret - 1));
    if (at < 0) {
      setMention(null);
      return;
    }
    const previous = at === 0 ? " " : text[at - 1];
    if (previous !== undefined && !/\s/.test(previous)) {
      setMention(null);
      return;
    }
    const query = text.slice(at + 1, caret);
    if (/\s/.test(query)) {
      setMention(null);
      return;
    }
    setMention({ start: at, query });
  }

  function commitMention(suggestion: MediaSuggestion | undefined) {
    const input = inputRef.current;
    if (!input || mention === null || !suggestion) return;
    const caret = input.selectionStart;
    const before = draft.slice(0, mention.start);
    const insert = `@${suggestion.token} `;
    const next = before + insert + draft.slice(caret);
    onDraft(next);
    setMention(null);
    onPickMedia?.(suggestion);
    requestAnimationFrame(() => {
      const pos = (before + insert).length;
      input.setSelectionRange(pos, pos);
      input.focus();
    });
  }

  return (
    <div
      data-stage-chat-panel
      className="stage-convo flex min-h-0 flex-1 flex-col overflow-hidden"
    >
      <div className="stage-chat-session-header border-b border-[var(--glass-border)] px-3 py-2">
        <div className="flex items-center gap-2">
          <button
            type="button"
            className="stage-chat-session-trigger glass-ghost min-w-0 flex-1 rounded-lg px-2.5 py-1.5 text-left text-[12px] font-semibold"
            onClick={toggleHistory}
            disabled={chatLoading}
            aria-expanded={historyOpen}
            title="Chat history"
          >
            <span className="block truncate">
              {chatLoading ? "Loading chats..." : activeChatSession?.title ?? "New chat"}
            </span>
          </button>
          <button
            type="button"
            className="stage-chat-icon-button stage-chat-icon"
            onClick={toggleHistory}
            disabled={chatLoading}
            aria-label="Chat history"
            title="Chat history"
            aria-expanded={historyOpen}
            data-active={historyOpen ? "true" : "false"}
          >
            <History className="h-3.5 w-3.5 stroke-[1.75]" />
          </button>
          <button
            type="button"
            className="stage-chat-icon-button stage-chat-icon"
            onClick={() => {
              onNewChat?.();
              setHistoryOpen(false);
            }}
            disabled={running || chatLoading}
            aria-label="New chat"
            title="New chat"
          >
            <Plus className="h-3.5 w-3.5 stroke-[1.75]" />
          </button>
          <label className="stage-permission-control" title="Agent permission mode">
            <span className="stage-permission-icon" aria-hidden>
              ∞
            </span>
            <select
              className="stage-permission-select"
              value={permissionMode}
              disabled={!onSetPermissionMode}
              onChange={(event) => onSetPermissionMode?.(event.target.value as PermissionMode)}
              aria-label="Agent permission mode"
            >
              <option value="manual">Manual</option>
              <option value="copilot">Copilot</option>
              <option value="autopilot">Auto</option>
            </select>
          </label>
          <label className="stage-permission-control" title="GPT-5.6 Codex capability profile">
            <Sparkles className="stage-permission-icon h-3 w-3 stroke-[1.75]" aria-hidden />
            <select
              className="stage-permission-select"
              value={agentProfile}
              disabled={!onSetAgentProfile}
              onChange={(event) => onSetAgentProfile?.(event.target.value as AgentProfile)}
              aria-label="GPT-5.6 Codex capability profile"
            >
              {AGENT_PROFILE_OPTIONS.map((option) => (
                <option key={option.value} value={option.value} title={option.description}>
                  {option.label}
                </option>
              ))}
            </select>
          </label>
        </div>
        {historyOpen ? (
          <div className="stage-chat-history mt-2 max-h-48 overflow-auto rounded-lg border border-[var(--glass-border)] bg-[rgba(10,10,18,0.86)] p-1">
            <button
              type="button"
              className="stage-chat-history-row"
              onClick={() => {
                onNewChat?.();
                setHistoryOpen(false);
              }}
              disabled={running || chatLoading}
            >
              <span className="truncate">New chat</span>
              <span className="font-mono text-[10px] text-[var(--color-text-muted)]">fresh</span>
            </button>
            {chatSessions.map((session) => (
              <button
                key={session.logPath}
                type="button"
                className="stage-chat-history-row"
                data-active={activeChatSession?.logPath === session.logPath ? "true" : "false"}
                onClick={() => {
                  onSelectChatSession?.(session);
                  setHistoryOpen(false);
                }}
                disabled={running || chatLoading}
              >
                <span className="min-w-0 truncate">{session.title}</span>
                <span className="shrink-0 font-mono text-[10px] text-[var(--color-text-muted)]">
                  {session.messageCount}
                </span>
              </button>
            ))}
            {!chatLoading && chatSessions.length === 0 ? (
              <p className="m-0 px-2 py-2 text-[11px] text-[var(--color-text-muted)]">
                No saved chats yet.
              </p>
            ) : null}
          </div>
        ) : null}
      </div>
      {agentRead ? (
        <div className="border-b border-[var(--glass-border)] px-3 py-2">
          <span className="text-[10px] text-[var(--color-text-muted)]">{agentRead}</span>
        </div>
      ) : null}
      <div className="stage-chat-scroll min-h-0 flex-1 overflow-auto">
        <ChatStream />
      </div>
      <div className="stage-chat-composer border-t border-[var(--glass-border)] p-2">
        <div className="stage-chat-card glass glass-reactive relative grid grid-cols-[minmax(0,1fr)_32px] items-center gap-2 rounded-lg px-3 py-2">
          <textarea
            ref={inputRef}
            value={draft}
            rows={1}
            onChange={(event) => {
              onDraft(event.target.value);
              requestAnimationFrame(syncMentionFromCaret);
            }}
            onSelect={syncMentionFromCaret}
            onKeyDown={(event) => {
              if (mention !== null && filteredMedia.length > 0) {
                if (event.key === "ArrowDown") {
                  event.preventDefault();
                  setMentionIdx((index) => (index + 1) % filteredMedia.length);
                  return;
                }
                if (event.key === "ArrowUp") {
                  event.preventDefault();
                  setMentionIdx((index) => (index - 1 + filteredMedia.length) % filteredMedia.length);
                  return;
                }
                if (event.key === "Enter" || event.key === "Tab") {
                  event.preventDefault();
                  commitMention(filteredMedia[mentionIdx]);
                  return;
                }
                if (event.key === "Escape") {
                  event.preventDefault();
                  setMention(null);
                  return;
                }
              }
              if (event.key === "Enter" && !event.shiftKey && !event.nativeEvent.isComposing) {
                event.preventDefault();
                onSubmit();
              }
            }}
            placeholder="ask, trim, propose... @ to attach media"
            autoComplete="off"
            autoCorrect="off"
            autoCapitalize="off"
            spellCheck={false}
            className="stage-chat-input min-w-0 flex-1 resize-none bg-transparent text-[13px] leading-5 text-[var(--color-text-primary)] placeholder:text-[var(--color-text-muted)] outline-none"
          />
          {mention !== null && filteredMedia.length > 0 ? (
            <div className="awidat-mention-picker" role="listbox" aria-label="Attach a clip">
              {filteredMedia.map((suggestion, index) => (
                <button
                  key={suggestion.id}
                  type="button"
                  role="option"
                  aria-selected={index === mentionIdx}
                  className={`awidat-mention-item ${index === mentionIdx ? "is-active" : ""}`}
                  onMouseDown={(event) => event.preventDefault()}
                  onMouseEnter={() => setMentionIdx(index)}
                  onClick={() => commitMention(suggestion)}
                >
                  <span className="awidat-mention-item-label">{suggestion.label}</span>
                  {suggestion.detail ? (
                    <span className="awidat-mention-item-detail">{suggestion.detail}</span>
                  ) : null}
                </button>
              ))}
            </div>
          ) : null}
          {running ? (
            <div className="stage-chat-action-well">
              <button onClick={onCancel} className="glass-ghost grid h-8 w-8 place-items-center rounded-lg text-[13px]">■</button>
            </div>
          ) : (
            <div className="stage-chat-action-well">
              <button onClick={onSubmit} className="glass-cta grid h-8 w-8 place-items-center rounded-lg text-[13px]">▸</button>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
