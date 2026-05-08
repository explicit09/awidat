// The chat pane renders every protocol Item variant and houses the
// inline approval / input cards. App-level event subscriptions live in
// App.tsx so toolbar-triggered jobs are heard even when this pane
// remounts during project changes.

import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import type { Item } from "../protocol";
import { useAgentStore } from "./store";
import { ApprovalCard } from "./ApprovalCard";
import { UserInputCard } from "./UserInputCard";
import { JobCard } from "./JobCard";
import { useProjectStore } from "../app/state";
import { useTimelineStore } from "../timeline/store";

export function ChatStream() {
  const items = useAgentStore((s) => s.items);
  const running = useAgentStore((s) => s.running);
  const turnError = useAgentStore((s) => s.turnError);

  const projectReady = useProjectStore((s) => s.current !== null);
  const hasTimelineClips = useTimelineStore((s) =>
    s.snapshot.tracks.some((track) => track.items.length > 0),
  );
  const timelineRefreshing = useTimelineStore((s) => s.refreshing);

  return (
    <div className="chat-items" aria-live="polite">
      {items.length === 0 &&
        !running &&
        !turnError &&
        projectReady && (
          <p className="chat-empty chat-empty-loaded">
            {timelineRefreshing
              ? "Loading project..."
              : hasTimelineClips
                ? "Timeline loaded. Ask Awidat for an edit, or select a clip below to inspect it."
                : "No chat history yet. Import media or ask Awidat to get started."}
          </p>
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
      return <ToolCallItem item={item} />;
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

function ToolCallItem({
  item,
}: {
  item: Extract<Item, { kind: "tool_call" }>;
}) {
  const hasError = item.result !== null && "Err" in item.result;
  const isRunning = item.phase !== "completed";
  const status = hasError ? "error" : isRunning ? item.phase : "done";
  const resultSummary =
    item.result === null
      ? ""
      : "Ok" in item.result
        ? summarizeText(item.result.Ok)
        : summarizeText(item.result.Err);

  return (
    <article
      className={`item item-tool item-phase-${item.phase}${
        hasError ? " item-tool-error" : ""
      }`}
    >
      <details className="tool-details" open={isRunning || hasError}>
        <summary className="tool-summary-row">
          <span className="tool-kind">tool</span>
          <code>{item.name}</code>
          <span className={`tool-status tool-status-${status}`}>{status}</span>
          <span className="tool-summary-text">
            {summarizeToolCall(item)}
            {resultSummary && <span> · {resultSummary}</span>}
          </span>
        </summary>
        <div className="tool-detail-body">
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
        </div>
      </details>
    </article>
  );
}

function summarizeToolCall(item: Extract<Item, { kind: "tool_call" }>): string {
  const args = item.args;
  if (!args || typeof args !== "object" || Array.isArray(args)) {
    return "Running tool";
  }
  const record = args as Record<string, unknown>;
  switch (item.name) {
    case "apply_edl":
      return typeof record.reasoning === "string"
        ? summarizeText(record.reasoning, 96)
        : "Proposed timeline edit";
    case "view_timeline":
      return "Read current timeline";
    case "view_episode":
      return "Read episode map";
    case "find_episode_start":
      return "Found publishable episode start";
    case "find_beat":
      return typeof record.kind === "string"
        ? `Found ${record.kind} beats`
        : "Found editorial beats";
    case "inspect_moment":
      return typeof record.moment_id === "string"
        ? `Inspected ${record.moment_id}`
        : "Inspected moment context";
    case "read_index":
      return typeof record.channel === "string"
        ? `Read ${record.channel} index`
        : "Read index";
    case "start_indexing":
      return "Started indexing";
    case "start_render":
      return "Started render";
    default:
      return "Tool call";
  }
}

function summarizeText(text: string, max = 120): string {
  const oneLine = text.replace(/\s+/g, " ").trim();
  if (!oneLine) return "";
  return oneLine.length > max ? `${oneLine.slice(0, max - 1)}…` : oneLine;
}
