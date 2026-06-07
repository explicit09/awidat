import { ChatStream } from "../agent/ChatStream";

type ConversationPanelProps = {
  agentRead?: string;
};

export function ConversationPanel({
  agentRead,
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
    </div>
  );
}
