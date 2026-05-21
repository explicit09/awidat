import { CheckCheck, History, Sparkles, X } from "lucide-react";
import type { ReactNode } from "react";
import {
  Button,
  ConfidenceMeter,
  Inline,
  Pill,
  ProposalCard,
  ReviewActions,
  Stack,
  cn,
  type PillStatus,
  type RiskLevel,
} from "../ui";

/**
 * BatchReviewSurface — concept Screen 3.
 *
 * Three-column command-center for supervising the agent across many
 * proposals at once:
 *
 *   ┌─ Agent command history ─┬─ Proposal list + before/after ─┬─ Batch insights ─┐
 *   │ • request 1 (done)       │  [ ProposalCard ]              │ Overall stats    │
 *   │ • request 2 (running)    │  [ ProposalCard ] (selected)   │ Confidence avg   │
 *   │ • request 3 (queued)     │  [ ProposalCard ]              │ Risk distribution│
 *   │                          │  ─────────────────             │                  │
 *   │  Batch actions:          │  Before / After preview        │ Recent decisions │
 *   │  Accept all | Reject all │  (placeholder until 2.11+)      │                  │
 *   └──────────────────────────┴─────────────────────────────────┴──────────────────┘
 */

export type AgentCommand = {
  id: string;
  text: string;
  status: "queued" | "running" | "complete" | "failed";
  proposalCount?: number;
  startedAt?: string;
};

export type BatchProposal = {
  id: string;
  title: string;
  status: PillStatus;
  timeRange: string;
  cutType?: string;
  explanation?: string;
  confidence?: number;
  risk?: RiskLevel;
  thumbnail?: string;
};

export type BatchReviewSurfaceProps = {
  commands?: AgentCommand[];
  proposals?: BatchProposal[];
  selectedProposalId?: string;
  beforeFrame?: ReactNode;
  afterFrame?: ReactNode;
  /** Insights numbers driven by the parent. */
  insights?: {
    pending: number;
    accepted: number;
    rejected: number;
    avgConfidence?: number;
    riskHigh?: number;
    riskMedium?: number;
    riskLow?: number;
  };
  onSelectProposal?: (p: BatchProposal) => void;
  onAcceptOne?: (p: BatchProposal) => void;
  onRejectOne?: (p: BatchProposal) => void;
  onAcceptAll?: () => void;
  onRejectAll?: () => void;
  onReviseAll?: () => void;
  onPickCommand?: (cmd: AgentCommand) => void;
  className?: string;
};

export function BatchReviewSurface({
  commands = [],
  proposals = [],
  selectedProposalId,
  beforeFrame,
  afterFrame,
  insights,
  onSelectProposal,
  onAcceptOne,
  onRejectOne,
  onAcceptAll,
  onRejectAll,
  onReviseAll,
  onPickCommand,
  className,
}: BatchReviewSurfaceProps) {
  return (
    <div
      className={cn(
        "grid h-full grid-cols-[280px_1fr_280px] bg-[var(--color-surface-app)]",
        className,
      )}
    >
      {/* Command history column */}
      <aside className="border-r border-[var(--color-border-subtle)] bg-[var(--color-surface-panel)] flex flex-col min-h-0">
        <div className="h-10 px-3 flex items-center border-b border-[var(--color-border-subtle)] shrink-0">
          <Inline gap="2" align="center">
            <History className="h-3.5 w-3.5 stroke-[1.75] text-[var(--color-text-muted)]" />
            <span className="text-[var(--text-label)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-secondary)]">
              Agent command history
            </span>
          </Inline>
        </div>
        <div className="flex-1 overflow-y-auto p-2">
          <Stack gap="1">
            {commands.map((cmd) => (
              <button
                key={cmd.id}
                type="button"
                onClick={() => onPickCommand?.(cmd)}
                className={cn(
                  "text-left rounded-[var(--radius-sm)] border px-2.5 py-2",
                  "border-[var(--color-border-subtle)] bg-[var(--color-surface-card)]",
                  "hover:border-[var(--color-border)] hover:bg-[var(--color-surface-card-hover)] transition-colors",
                )}
              >
                <Stack gap="1">
                  <Inline justify="between" align="center">
                    <span className="text-[var(--text-caption)] text-[var(--color-text-muted)] font-mono">
                      {cmd.startedAt ?? "—"}
                    </span>
                    <CommandStatusPill status={cmd.status} count={cmd.proposalCount} />
                  </Inline>
                  <span className="text-[var(--text-body-sm)] text-[var(--color-text-primary)] leading-snug">
                    {cmd.text}
                  </span>
                </Stack>
              </button>
            ))}
            {commands.length === 0 ? (
              <span className="text-[var(--text-caption)] text-[var(--color-text-muted)] p-2">
                No commands yet.
              </span>
            ) : null}
          </Stack>
        </div>
        <div className="border-t border-[var(--color-border-subtle)] p-3 shrink-0">
          <Stack gap="2">
            <span className="text-[var(--text-label)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-muted)]">
              Batch actions
            </span>
            <Inline gap="2">
              <Button
                variant="accept"
                size="sm"
                onClick={onAcceptAll}
                leadingIcon={<CheckCheck className="h-3.5 w-3.5 stroke-[1.75]" />}
                className="flex-1"
              >
                Accept all
              </Button>
              <Button
                variant="reject"
                size="sm"
                onClick={onRejectAll}
                leadingIcon={<X className="h-3.5 w-3.5 stroke-[1.75]" />}
                className="flex-1"
              >
                Reject all
              </Button>
            </Inline>
            <Button
              variant="secondary"
              size="sm"
              onClick={onReviseAll}
              leadingIcon={<Sparkles className="h-3.5 w-3.5 stroke-[1.75]" />}
            >
              Revise with prompt
            </Button>
          </Stack>
        </div>
      </aside>

      {/* Center: proposals + before/after preview */}
      <main className="flex flex-col min-h-0">
        <div className="h-10 px-4 flex items-center border-b border-[var(--color-border-subtle)] shrink-0">
          <span className="text-[var(--text-label)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-secondary)]">
            Proposed changes — {proposals.length}
          </span>
        </div>
        <div className="grid grid-cols-2 flex-1 min-h-0">
          <div className="overflow-y-auto p-3 border-r border-[var(--color-border-subtle)]">
            <Stack gap="2">
              {proposals.map((p) => (
                <ProposalCard
                  key={p.id}
                  title={p.title}
                  status={p.status}
                  timeRange={p.timeRange}
                  cutType={p.cutType}
                  explanation={p.explanation}
                  confidence={p.confidence}
                  risk={p.risk}
                  thumbnail={p.thumbnail}
                  selected={selectedProposalId === p.id}
                  onSelect={() => onSelectProposal?.(p)}
                  footer={
                    selectedProposalId === p.id ? (
                      <ReviewActions
                        size="sm"
                        onAccept={() => onAcceptOne?.(p)}
                        onReject={() => onRejectOne?.(p)}
                      />
                    ) : undefined
                  }
                />
              ))}
            </Stack>
          </div>
          <div className="flex flex-col min-h-0">
            <div className="h-9 px-3 flex items-center border-b border-[var(--color-border-subtle)] shrink-0">
              <span className="text-[var(--text-label)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-muted)]">
                Before / After
              </span>
            </div>
            <div className="grid grid-rows-2 flex-1 min-h-0">
              <FramePane label="Before" content={beforeFrame} />
              <div className="border-t border-[var(--color-border-subtle)]">
                <FramePane label="After" content={afterFrame} />
              </div>
            </div>
          </div>
        </div>
      </main>

      {/* Insights column */}
      <aside className="border-l border-[var(--color-border-subtle)] bg-[var(--color-surface-panel)] flex flex-col min-h-0">
        <div className="h-10 px-3 flex items-center border-b border-[var(--color-border-subtle)] shrink-0">
          <span className="text-[var(--text-label)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-secondary)]">
            Batch insights
          </span>
        </div>
        <div className="flex-1 overflow-y-auto p-3">
          <Stack gap="4">
            <Stack gap="2">
              <KV label="Pending" value={insights?.pending ?? proposals.length} tone="warning" />
              <KV label="Accepted" value={insights?.accepted ?? 0} tone="success" />
              <KV label="Rejected" value={insights?.rejected ?? 0} tone="danger" />
            </Stack>
            {typeof insights?.avgConfidence === "number" ? (
              <Stack gap="2">
                <span className="text-[var(--text-label)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-muted)]">
                  Avg confidence
                </span>
                <ConfidenceMeter score={insights.avgConfidence} label="" size="sm" />
              </Stack>
            ) : null}
            {insights ? (
              <Stack gap="2">
                <span className="text-[var(--text-label)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-muted)]">
                  Risk distribution
                </span>
                <Inline gap="2">
                  <RiskTile label="Low" count={insights.riskLow ?? 0} color="var(--color-success)" />
                  <RiskTile label="Med" count={insights.riskMedium ?? 0} color="var(--color-warning)" />
                  <RiskTile label="High" count={insights.riskHigh ?? 0} color="var(--color-risk)" />
                </Inline>
              </Stack>
            ) : null}
          </Stack>
        </div>
      </aside>
    </div>
  );
}

function CommandStatusPill({ status, count }: { status: AgentCommand["status"]; count?: number }) {
  const map: Record<AgentCommand["status"], PillStatus> = {
    queued: "neutral",
    running: "processing",
    complete: "accepted",
    failed: "failed",
  };
  return (
    <Pill status={map[status]} dot={false}>
      {status === "complete" && count ? `${count}` : status}
    </Pill>
  );
}

function KV({
  label,
  value,
  tone,
}: {
  label: string;
  value: number;
  tone: "success" | "warning" | "danger";
}) {
  const color =
    tone === "success"
      ? "var(--color-success)"
      : tone === "warning"
        ? "var(--color-warning)"
        : "var(--color-danger)";
  return (
    <Inline justify="between" align="center">
      <span className="text-[var(--text-caption)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-muted)]">
        {label}
      </span>
      <span className="font-mono text-[var(--text-h3)] font-semibold" style={{ color }}>
        {value}
      </span>
    </Inline>
  );
}

function RiskTile({ label, count, color }: { label: string; count: number; color: string }) {
  return (
    <div className="flex-1 rounded-[var(--radius-sm)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-card)] p-2 text-center">
      <div className="text-[var(--text-caption)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-muted)]">
        {label}
      </div>
      <div className="font-mono text-[var(--text-h3)] font-semibold mt-1" style={{ color }}>
        {count}
      </div>
    </div>
  );
}

function FramePane({ label, content }: { label: string; content: ReactNode }) {
  return (
    <div className="relative bg-black flex items-center justify-center min-h-0">
      <span className="absolute top-1.5 left-2 text-[var(--text-caption)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-muted)] z-10 pointer-events-none">
        {label}
      </span>
      {content ?? (
        <span className="text-[var(--text-caption)] text-[var(--color-text-muted)]">
          (preview frame slots in via parent)
        </span>
      )}
    </div>
  );
}
