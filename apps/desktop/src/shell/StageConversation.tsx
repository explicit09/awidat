import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { ChatStream } from "../agent/ChatStream";
import type { MediaSuggestion } from "./CommandRail";

type ConversationPanelProps = {
  agentRead?: string;
  draft: string;
  running: boolean;
  onDraft: (value: string) => void;
  onSubmit: () => void;
  onCancel: () => void;
  mediaSuggestions?: MediaSuggestion[];
  onPickMedia?: (suggestion: MediaSuggestion) => void;
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
}: ConversationPanelProps) {
  const inputRef = useRef<HTMLTextAreaElement | null>(null);
  const [mention, setMention] = useState<{ start: number; query: string } | null>(null);
  const [mentionIdx, setMentionIdx] = useState(0);

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
      {agentRead ? (
        <div className="border-b border-[var(--glass-border)] px-3 py-2">
          <span className="text-[10px] text-[var(--color-text-muted)]">{agentRead}</span>
        </div>
      ) : null}
      <div className="min-h-0 flex-1 overflow-auto">
        <ChatStream />
      </div>
      <div className="stage-chat-composer border-t border-[var(--glass-border)] p-2">
        <div className="stage-chat-card glass glass-reactive relative flex items-end gap-2 rounded-lg px-3 py-2">
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
            <button onClick={onCancel} className="glass-ghost grid h-8 w-8 place-items-center rounded-lg text-[13px]">■</button>
          ) : (
            <button onClick={onSubmit} className="glass-cta grid h-8 w-8 place-items-center rounded-lg text-[13px]">▸</button>
          )}
        </div>
      </div>
    </div>
  );
}
