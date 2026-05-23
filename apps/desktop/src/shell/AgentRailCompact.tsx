import { type ReactNode } from "react";
import { cn } from "../ui";
import { STARTER_PROMPTS } from "./agentPrompts";

export type AgentRailCompactProps = {
  /** Optional project summary line — e.g. "4 clips · 3:46 · 0 edits". */
  projectSummary?: string;
  /** Suggested prompts to seed the composer. */
  suggestions?: Array<{ id: string; label: string; prompt: string }>;
  /** Composer slot — re-used from the parent's existing composer. */
  composer: ReactNode;
  /** Called when the user clicks "Open conversation". */
  onExpand: () => void;
  /** How many messages are in the (hidden) conversation. */
  messageCount?: number;
  /** Called when the user selects a starter prompt. */
  onUseSuggestion?: (prompt: string) => void;
};

export function AgentRailCompact({
  projectSummary,
  composer,
  onExpand,
  messageCount = 0,
  onUseSuggestion,
}: AgentRailCompactProps) {
  return (
    <div className="flex flex-col gap-3 p-3">
      {/* 1. Project summary */}
      <p className="text-[var(--text-caption)] text-[var(--color-text-muted)] leading-snug">
        {projectSummary ?? "Ready for editing"}
      </p>

      {/* 2. Starter prompts */}
      <div className="flex flex-col gap-0.5">
        {STARTER_PROMPTS.map((prompt) => (
          <button
            key={prompt}
            type="button"
            onClick={() => onUseSuggestion?.(prompt)}
            className={cn(
              "group flex items-start gap-1.5 rounded-[var(--radius-xs)] px-1 py-0.5 text-left",
              "transition-colors hover:bg-[var(--color-surface-hover)]",
            )}
          >
            <span className="mt-0.5 text-[var(--color-text-muted)] group-hover:text-[var(--accent-selection)]">
              ›
            </span>
            <span className="text-[var(--text-caption)] text-[var(--color-text-secondary)] leading-snug group-hover:text-[var(--color-text-primary)]">
              {prompt}
            </span>
          </button>
        ))}
      </div>

      {/* 3. Composer slot */}
      {composer}

      {/* 4. "Open conversation" link — only shown when there are messages */}
      {messageCount > 0 ? (
        <button
          type="button"
          onClick={onExpand}
          className="self-start text-[var(--text-caption)] text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)] transition-colors"
        >
          Open conversation · {messageCount} {messageCount === 1 ? "message" : "messages"}
        </button>
      ) : null}
    </div>
  );
}
