import { Minimize2, PanelRight } from "lucide-react";
import { ChatStream } from "../agent/ChatStream";

type ConversationPanelProps = {
  agentRead?: string;
  onCollapse: () => void;
};

export function ConversationPanel({
  agentRead,
  onCollapse,
}: ConversationPanelProps) {
  return (
    <div
      data-stage-chat-panel
      className="glass glass-strong stage-convo flex min-h-0 flex-1 flex-col overflow-hidden"
      style={{ borderRadius: 10 }}
    >
      <div className="flex items-center gap-2 border-b border-[var(--glass-border)] px-3 py-2">
        <PanelRight className="h-3.5 w-3.5 text-[var(--color-brand-hover)]" aria-hidden />
        <span className="text-[11px] font-semibold text-[var(--color-text-secondary)]">Conversation</span>
        {agentRead ? <span className="min-w-0 truncate text-[10px] text-[var(--color-text-muted)]">· {agentRead}</span> : null}
        <div className="ml-auto flex items-center gap-1">
          <button
            onClick={onCollapse}
            aria-label="Collapse conversation"
            title="Collapse conversation"
            className="stage-chat-icon"
          >
            <Minimize2 className="h-3.5 w-3.5" aria-hidden />
          </button>
        </div>
      </div>
      <div className="min-h-0 flex-1 overflow-auto">
        <ChatStream />
      </div>
    </div>
  );
}
