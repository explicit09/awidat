// The chat pane renders every protocol Item variant and houses the
// inline approval / input cards. App-level event subscriptions live in
// App.tsx so toolbar-triggered jobs are heard even when this pane
// remounts during project changes.

import { useEffect, useMemo, useRef, useState } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import type { Item } from "../protocol";
import { useAgentStore } from "./store";
import { ApprovalCard } from "./ApprovalCard";
import { UserInputCard } from "./UserInputCard";
import { useProjectStore } from "../app/state";
import { EmptyState } from "../app/EmptyState";
import { EmptyConversation } from "./EmptyConversation";
import { useTimelineStore, type TimelineItem, type TimelineSnapshot } from "../timeline/store";
import { serializeEdl, type EdlOp } from "../timeline/edlBuilder";
import { editorDispatch } from "../editor/tauriDispatch";
import { isMontageSentinel } from "../state/introState";
import { RationaleHint } from "../ui/components/RationaleHint";
import {
  activityGroupLabel,
  activityStatus,
  groupActivityItems,
  jobLabel,
  jobResultText,
  type ActivityItem,
} from "./chatActivity";
import { visibleChatItems } from "./chatDisplay";

export function ChatStream() {
  const rawItems = useAgentStore((s) => s.items);
  const items = useMemo(() => visibleChatItems(rawItems), [rawItems]);
  const displayItems = useMemo(() => groupActivityItems(items), [items]);
  const running = useAgentStore((s) => s.running);
  const turnError = useAgentStore((s) => s.turnError);
  const scrollerRef = useRef<HTMLDivElement | null>(null);
  const bottomRef = useRef<HTMLDivElement | null>(null);
  const shouldFollowRef = useRef(true);

  const projectReady = useProjectStore((s) => s.current !== null);
  const hasTimelineClips = useTimelineStore((s) =>
    s.snapshot.tracks.some((track) => track.items.length > 0),
  );
  const timelineRefreshing = useTimelineStore((s) => s.refreshing);

  useEffect(() => {
    if (!shouldFollowRef.current) return;
    const frame = requestAnimationFrame(() => {
      bottomRef.current?.scrollIntoView({ block: "end" });
    });
    return () => cancelAnimationFrame(frame);
  }, [items, running, turnError]);

  function handleScroll() {
    const el = scrollerRef.current;
    if (!el) return;
    const distanceFromBottom = el.scrollHeight - el.scrollTop - el.clientHeight;
    shouldFollowRef.current = distanceFromBottom < 96;
  }

  return (
    <div
      ref={scrollerRef}
      onScroll={handleScroll}
      className="chat-items"
      aria-live="polite"
    >
      {items.length === 0 &&
        !running &&
        !turnError &&
        projectReady && (
          hasTimelineClips ? (
            <EmptyConversation />
          ) : timelineRefreshing ? (
            <p className="chat-empty chat-empty-loaded">Loading project...</p>
          ) : (
            <EmptyState />
          )
        )}
      {items.length === 0 && !running && !turnError && !projectReady && (
        <EmptyConversation />
      )}
      {displayItems.map((entry) => (
        entry.kind === "activity_group" ? (
          <ActivityGroupView key={entry.id} items={entry.items} />
        ) : (
          <ItemView key={entry.item.id} item={entry.item} />
        )
      ))}
      {turnError && (
        <article className="item item-error">
          <div className="item-meta">turn error</div>
          <div className="item-body">{turnError}</div>
        </article>
      )}
      <div ref={bottomRef} aria-hidden="true" />
    </div>
  );
}

function ItemView({ item }: { item: Item }) {
  switch (item.kind) {
    case "user_input":
      // Synthetic editorial turns carry an `[montage:*]` sentinel —
      // they are an instruction to the model, not a user message. Hide
      // them from the transcript so the agent's reply reads as a
      // direct, unprompted response to project state. Covers the intro
      // (F3) and "Prepare a starting cut" (B4) flows.
      if (isMontageSentinel(item.text)) return null;
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
      return <ActivityGroupView items={[item]} />;
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
      return <ActivityGroupView items={[item]} />;
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
          {/* Rationale — the agent's one-sentence "why", surfaced
              right under the summary so the user can take the call
              on faith without opening the inspector. */}
          <RationaleHint rationale={item.rationale} className="mt-1" />
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

function ActivityGroupView({ items }: { items: ActivityItem[] }) {
  const runningCount = items.filter((item) => activityStatus(item) === "running").length;
  const failedCount = items.filter((item) => activityStatus(item) === "failed").length;
  const openByDefault = runningCount > 0 || failedCount > 0;
  const label = activityGroupLabel(items);
  const status = failedCount > 0 ? "failed" : runningCount > 0 ? "running" : "done";

  return (
    <article className={`item item-activity item-activity-${status}`}>
      <details className="activity-details" open={openByDefault}>
        <summary className="activity-summary-row">
          <span className="activity-kind">activity</span>
          <span className={`activity-dot activity-dot-${status}`} aria-hidden />
          <span className="activity-summary-text">{label}</span>
          <span className={`activity-status activity-status-${status}`}>{status}</span>
        </summary>
        <div className="activity-detail-body">
          {items.map((item) =>
            item.kind === "tool_call" ? (
              <ToolCallItem key={item.id} item={item} />
            ) : (
              <JobActivityRow key={item.id} item={item} />
            ),
          )}
        </div>
      </details>
    </article>
  );
}

function JobActivityRow({ item }: { item: Extract<Item, { kind: "job" }> }) {
  const status = activityStatus(item);
  const resultSummary =
    item.result && typeof item.result === "object"
      ? "ok" in item.result
        ? item.result.ok.summary
        : item.result.err.message
      : null;
  return (
    <details className="activity-row">
      <summary className="activity-row-summary">
        <span className={`activity-dot activity-dot-${status}`} aria-hidden />
        <span className="activity-row-kind">job</span>
        <code>{jobLabel(item.job_kind)}</code>
        <span className="activity-row-text">
          {item.status}
          {resultSummary ? ` · ${summarizeText(resultSummary, 96)}` : ""}
        </span>
      </summary>
      <div className="activity-row-detail">
        <dl>
          <dt>Status</dt>
          <dd>{item.status}</dd>
          <dt>Result</dt>
          <dd>{jobResultText(item)}</dd>
          {item.output_path && (
            <>
              <dt>Output</dt>
              <dd>{item.output_path}</dd>
            </>
          )}
        </dl>
      </div>
    </details>
  );
}

function ToolCallItem({
  item,
}: {
  item: Extract<Item, { kind: "tool_call" }>;
}) {
  const snapshot = useTimelineStore((s) => s.snapshot);
  const [proposalBusy, setProposalBusy] = useState(false);
  const [proposalError, setProposalError] = useState<string | null>(null);
  const hasError = item.result !== null && "Err" in item.result;
  const isRunning = item.phase !== "completed";
  const status = hasError ? "error" : isRunning ? item.phase : "done";
  const assessment = parseAssessEditQualityResult(item);
  const resultSummary =
    item.result === null
      ? ""
      : "Ok" in item.result
        ? summarizeText(item.result.Ok)
        : summarizeText(item.result.Err);
  const assessmentEdl =
    assessment === null ? null : buildAssessmentProposalEdl(assessment, snapshot);

  async function proposeAssessmentRecommendation() {
    if (!assessmentEdl) return;
    setProposalBusy(true);
    setProposalError(null);
    try {
      await editorDispatch.proposeUserEditText(assessmentEdl);
    } catch (e) {
      setProposalError(e instanceof Error ? e.message : String(e));
    } finally {
      setProposalBusy(false);
    }
  }

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
                <>
                  <pre className="result-ok">{item.result.Ok}</pre>
                  {assessment && (
                    <AssessmentRecommendationAction
                      assessment={assessment}
                      edlText={assessmentEdl}
                      busy={proposalBusy}
                      error={proposalError}
                      onPropose={proposeAssessmentRecommendation}
                    />
                  )}
                </>
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

type AssessEditQualityReport = {
  atS: number;
  kind: string;
  recommendation: {
    action: string;
    cutType: string;
    intent: string;
    edlOp: string | null;
    reason: string;
  };
  edlGuidance: string | null;
};

function AssessmentRecommendationAction({
  assessment,
  edlText,
  busy,
  error,
  onPropose,
}: {
  assessment: AssessEditQualityReport;
  edlText: string | null;
  busy: boolean;
  error: string | null;
  onPropose: () => void;
}) {
  const { recommendation } = assessment;
  return (
    <div className="tool-recommendation">
      <div className="tool-recommendation-copy">
        <span className="tool-recommendation-label">recommendation</span>
        <strong>{labelForRecommendation(recommendation)}</strong>
        <span>{recommendation.reason}</span>
      </div>
      {recommendation.edlOp ? (
        <button
          className="tool-recommendation-action"
          onClick={onPropose}
          disabled={busy || edlText === null}
          title={
            edlText === null
              ? "No nearby cut boundary found in the current timeline"
              : "Open this as a timeline proposal"
          }
        >
          {busy ? "…" : "Propose"}
        </button>
      ) : null}
      {error && <div className="tool-recommendation-error">{error}</div>}
    </div>
  );
}

function parseAssessEditQualityResult(
  item: Extract<Item, { kind: "tool_call" }>,
): AssessEditQualityReport | null {
  if (item.name !== "assess_edit_quality" || item.result === null || !("Ok" in item.result)) {
    return null;
  }
  try {
    const parsed = JSON.parse(item.result.Ok) as Record<string, unknown>;
    const recommendation = parsed.recommendation as Record<string, unknown> | undefined;
    if (!recommendation) return null;
    const atS = typeof parsed.at_s === "number" ? parsed.at_s : NaN;
    const edlOp =
      typeof recommendation.edl_op === "string" ? recommendation.edl_op : null;
    return {
      atS,
      kind: typeof parsed.kind === "string" ? parsed.kind : "cut",
      edlGuidance:
        typeof parsed.edl_guidance === "string" ? parsed.edl_guidance : null,
      recommendation: {
        action:
          typeof recommendation.action === "string"
            ? recommendation.action
            : "review",
        cutType:
          typeof recommendation.cut_type === "string"
            ? recommendation.cut_type
            : "hard_cut",
        intent:
          typeof recommendation.intent === "string"
            ? recommendation.intent
            : "low_attention_edit",
        edlOp,
        reason:
          typeof recommendation.reason === "string"
            ? recommendation.reason
            : "Review this edit before committing.",
      },
    };
  } catch {
    return null;
  }
}

function buildAssessmentProposalEdl(
  assessment: AssessEditQualityReport,
  snapshot: TimelineSnapshot,
): string | null {
  if (!isFinite(assessment.atS)) return null;
  const boundary = findNearestClipBoundary(snapshot, assessment.atS);
  if (!boundary) return null;
  const { recommendation } = assessment;
  const reason = recommendation.reason;
  const confidence = 0.8;
  const cutIntent: EdlOp = {
    kind: "set_cut_intent",
    from: { kind: "clip_uuid", uuid: boundary.from.clip_uuid },
    to: { kind: "clip_uuid", uuid: boundary.to.clip_uuid },
    cutType: recommendation.cutType,
    intent: recommendation.intent,
    audioRelation: audioRelationForCut(recommendation.cutType),
    confidence,
    reason,
  };
  const ops: EdlOp[] = [];
  if (recommendation.edlOp === "Set Audio Lead") {
    ops.push({
      kind: "set_audio_lead",
      anchor: { kind: "clip_uuid", uuid: boundary.to.clip_uuid },
      leadS: splitOffsetFromGuidance(assessment.edlGuidance, 0.35),
      confidence,
      reason,
    });
    ops.push(cutIntent);
  } else if (recommendation.edlOp === "Set Audio Trail") {
    ops.push({
      kind: "set_audio_trail",
      anchor: { kind: "clip_uuid", uuid: boundary.from.clip_uuid },
      trailS: splitOffsetFromGuidance(assessment.edlGuidance, 0.25),
      confidence,
      reason,
    });
    ops.push(cutIntent);
  } else if (recommendation.edlOp === "Set Cut Intent") {
    ops.push(cutIntent);
  } else {
    return null;
  }
  return serializeEdl(ops);
}

function findNearestClipBoundary(snapshot: TimelineSnapshot, atS: number): {
  from: Extract<TimelineItem, { kind: "clip" }>;
  to: Extract<TimelineItem, { kind: "clip" }>;
} | null {
  let best:
    | {
        score: number;
        from: Extract<TimelineItem, { kind: "clip" }>;
        to: Extract<TimelineItem, { kind: "clip" }>;
      }
    | null = null;
  for (const track of snapshot.tracks) {
    if (track.kind === "audio") continue;
    const clips = track.items
      .filter((item): item is Extract<TimelineItem, { kind: "clip" }> => item.kind === "clip")
      .sort((a, b) => a.track_start_s - b.track_start_s);
    for (let i = 0; i < clips.length - 1; i += 1) {
      const from = clips[i];
      const to = clips[i + 1];
      const outgoingBoundary = from.track_start_s + from.duration_s;
      const incomingBoundary = to.track_start_s;
      const midpoint = (outgoingBoundary + incomingBoundary) / 2;
      const score = Math.min(
        Math.abs(outgoingBoundary - atS),
        Math.abs(incomingBoundary - atS),
        Math.abs(midpoint - atS),
      );
      if (best === null || score < best.score) {
        best = { score, from, to };
      }
    }
  }
  return best && best.score <= 3 ? { from: best.from, to: best.to } : null;
}

function audioRelationForCut(cutType: string): string {
  if (cutType === "j_cut") return "audio_leads";
  if (cutType === "l_cut") return "audio_trails";
  return "sync";
}

function splitOffsetFromGuidance(guidance: string | null, fallback: number): number {
  if (!guidance) return fallback;
  const match = guidance.match(/(?:lead_s|trail_s)\D+([0-9]+(?:\.[0-9]+)?)/);
  if (!match) return fallback;
  const value = Number.parseFloat(match[1]);
  return isFinite(value) && value >= 0 ? value : fallback;
}

function labelForRecommendation(
  recommendation: AssessEditQualityReport["recommendation"],
): string {
  if (recommendation.edlOp === "Set Audio Lead") return "Use J-cut";
  if (recommendation.edlOp === "Set Audio Trail") return "Use L-cut";
  if (recommendation.edlOp === "Set Cut Intent") {
    return recommendation.cutType.replace(/_/g, " ");
  }
  return recommendation.action.replace(/_/g, " ");
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
    case "podcast_episode_spans":
      return "Planned candidate episode spans";
    case "podcast_edit_proposal":
      return "Built edit proposal";
    case "podcast_apply_accepted_edits":
      return "Prepared accepted edits";
    case "podcast_audio_polish":
      return "Checked audio polish";
    case "podcast_visual_polish":
      return "Checked visual polish";
    case "podcast_qc_report":
      return "Ran podcast QC";
    case "podcast_smooth_cut_boundaries":
      return "Checked cut smoothness";
    case "podcast_post_draft_check":
      return "Checked draft boundaries";
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
