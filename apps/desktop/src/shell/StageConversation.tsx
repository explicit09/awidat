import { ChatStream } from "../agent/ChatStream";

type ConversationPanelProps = {
  agentRead?: string;
  draft: string;
  running: boolean;
  onDraft: (value: string) => void;
  onSubmit: () => void;
  onCancel: () => void;
};

export function ConversationPanel({
  agentRead,
  draft,
  running,
  onDraft,
  onSubmit,
  onCancel,
}: ConversationPanelProps) {
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
        <div className="glass glass-reactive flex items-center gap-2 rounded-lg px-3 py-2">
          <input
            value={draft}
            onChange={(event) => onDraft(event.target.value)}
            onKeyDown={(event) => { if (event.key === "Enter") onSubmit(); }}
            placeholder="ask, trim, propose..."
            className="min-w-0 flex-1 bg-transparent text-[13px] text-[var(--color-text-primary)] placeholder:text-[var(--color-text-muted)] outline-none"
          />
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
