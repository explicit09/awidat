// The chat pane: subscribes to `awidat://item` and `awidat://turn-end`,
// renders every protocol Item variant, and houses the inline approval /
// input cards.

import { useEffect } from "react";
import { listen } from "@tauri-apps/api/event";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import {
  ITEM_EVENT,
  TURN_END_EVENT,
  type ItemEvent,
  type TurnEndEvent,
  type Item,
} from "../protocol";
import { useAgentStore } from "./store";
import { ApprovalCard } from "./ApprovalCard";
import { UserInputCard } from "./UserInputCard";
import { JobCard } from "./JobCard";
import { EmptyState } from "../app/EmptyState";
import { useProjectStore } from "../app/state";
import { useProposalStore, isProposedEditItem } from "../timeline/proposal";

export function ChatStream() {
  const items = useAgentStore((s) => s.items);
  const running = useAgentStore((s) => s.running);
  const turnError = useAgentStore((s) => s.turnError);
  const upsert = useAgentStore((s) => s.upsert);
  const setRunning = useAgentStore((s) => s.setRunning);
  const setTurnError = useAgentStore((s) => s.setTurnError);
  const ingestProposal = useProposalStore((s) => s.ingest);

  // Subscribe to backend events for the lifetime of the component.
  useEffect(() => {
    const itemsUnlisten = listen<ItemEvent>(ITEM_EVENT, (e) => {
      const item = e.payload.item;
      // Proposed edits feed two stores: the chat (so the user sees
      // a compact reference card) and the proposal store (so the
      // timeline canvas can render the ghost overlay).
      if (isProposedEditItem(item)) {
        ingestProposal(item);
      }
      upsert(item);
    });
    const endUnlisten = listen<TurnEndEvent>(TURN_END_EVENT, (e) => {
      if (e.payload.error) {
        setTurnError(e.payload.error);
      }
      setRunning(false);
    });
    return () => {
      itemsUnlisten.then((u) => u());
      endUnlisten.then((u) => u());
    };
  }, [upsert, setRunning, setTurnError, ingestProposal]);

  const projectReady = useProjectStore((s) => s.current !== null);

  return (
    <div className="chat-items" aria-live="polite">
      {items.length === 0 && !running && !turnError && projectReady && (
        <EmptyState />
      )}
      {items.length === 0 && !running && !turnError && !projectReady && (
        <p className="chat-empty">
          Open or create a project to get started.
        </p>
      )}
      {items.map((item) => (
        <ItemView key={item.id} item={item} />
      ))}
      {turnError && (
        <article className="item item-error">
          <div className="item-meta">turn error</div>
          <div className="item-body">{turnError}</div>
        </article>
      )}
    </div>
  );
}

function ItemView({ item }: { item: Item }) {
  switch (item.kind) {
    case "user_input":
      return (
        <article className="item item-user">
          <div className="item-meta">you</div>
          <div className="item-body">{item.text}</div>
        </article>
      );
    case "text":
      return (
        <article className={`item item-text item-phase-${item.phase}`}>
          <div className="item-body markdown">
            {item.text ? (
              <ReactMarkdown remarkPlugins={[remarkGfm]}>{item.text}</ReactMarkdown>
            ) : (
              <em>…</em>
            )}
          </div>
        </article>
      );
    case "tool_call":
      return (
        <article className={`item item-tool item-phase-${item.phase}`}>
          <div className="item-meta">
            tool · <code>{item.name}</code>
            {item.phase !== "completed" && <span className="phase-tag"> · {item.phase}</span>}
          </div>
          <pre className="item-args">{JSON.stringify(item.args ?? {}, null, 2)}</pre>
          {item.result !== null && (
            <div className="item-result">
              {"Ok" in item.result ? (
                <pre className="result-ok">{item.result.Ok}</pre>
              ) : (
                <pre className="result-err">{item.result.Err}</pre>
              )}
            </div>
          )}
        </article>
      );
    case "plan":
      return (
        <article className="item item-plan">
          <div className="item-meta">plan</div>
          <ul className="plan-list">
            {item.items.map((step, i) => (
              <li key={i} className={`plan-step status-${step.status}`}>
                <span className="plan-glyph">
                  {step.status === "completed" ? "✓" : step.status === "in_progress" ? "▶" : "·"}
                </span>
                {step.step}
              </li>
            ))}
          </ul>
          {item.note && <div className="plan-note">{item.note}</div>}
        </article>
      );
    case "awaiting_user_input":
      return <UserInputCard item={item} />;
    case "approval_request":
      return <ApprovalCard item={item} />;
    case "job":
      return <JobCard item={item} />;
    case "proposed_edit": {
      // Compact reference card. The actual ghost overlay lives on
      // the timeline canvas; this card just acknowledges in chat
      // that a proposal is open / closed.
      const sourceLabel =
        item.source.source === "agent"
          ? `agent · ${item.source.tool_name}`
          : "you";
      const phaseLabel =
        item.phase === "completed"
          ? item.summary // accept/reject summary lands here
          : "see timeline";
      return (
        <article className="item item-proposal">
          <div className="item-meta">
            proposed edit · {sourceLabel}
          </div>
          <div className="item-body">
            <strong>{item.summary}</strong>
            {item.phase !== "completed" && (
              <span className="proposal-phase-hint"> — {phaseLabel}</span>
            )}
          </div>
        </article>
      );
    }
    case "error":
      return (
        <article className="item item-error">
          <div className="item-meta">error</div>
          <div className="item-body">{item.message}</div>
        </article>
      );
  }
}
