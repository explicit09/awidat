import { CheckCheck, History, Sparkles, X } from "lucide-react";
import type { ReactNode } from "react";
import {
  Button,
  ConfidenceMeter,
  Inline,
  Pill,
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
  onInspectDeeper?: (p: BatchProposal) => void;
  onSendFeedback?: (p: BatchProposal) => void;
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
  onInspectDeeper,
  onSendFeedback,
  className,
}: BatchReviewSurfaceProps) {
  const selectedProposal = proposals.find((p) => p.id === selectedProposalId) ?? proposals[0];
  const clipDurations = ["00:48", "00:46", "00:57", "00:42", "01:19"];

  return (
    <div
      className={cn(
        "grid h-full min-h-0 grid-cols-[260px_minmax(0,1fr)_270px] bg-[var(--color-surface-app)]",
        className,
      )}
    >
      <aside className="border-r border-[var(--color-border-subtle)] bg-[var(--color-surface-panel)] flex flex-col min-h-0">
        <div className="h-9 px-2.5 flex items-center border-b border-[var(--color-border-subtle)] shrink-0">
          <Inline gap="2" align="center">
            <History className="h-3.5 w-3.5 stroke-[1.75] text-[var(--color-text-muted)]" />
            <span className="text-[var(--text-label)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-secondary)]">
              Agent command history
            </span>
          </Inline>
        </div>
        <div className="flex-1 overflow-y-auto p-1.5">
          <Stack gap="2">
            {commands.map((cmd) => (
              <button
                key={cmd.id}
                type="button"
                onClick={() => onPickCommand?.(cmd)}
                className={cn(
                  "text-left rounded-[var(--radius-sm)] border px-2 py-1.5",
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
            <section className="rounded-[var(--radius-md)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-card)] p-2">
              <Stack gap="2">
                <span className="text-[var(--text-label)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-muted)]">
                  Active request
                </span>
                <p className="text-[var(--text-body-sm)] font-medium leading-relaxed text-[var(--color-text-primary)]">
                  Find the best 5 short clips for TikTok.
                </p>
                <p className="text-[var(--text-caption)] leading-relaxed text-[var(--color-text-secondary)]">
                  Create high-retention, vertical clips optimized for TikTok (9:16) that drive engagement.
                </p>
              </Stack>
            </section>
            <section className="rounded-[var(--radius-md)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-card)] p-2">
              <Stack gap="2">
                <span className="text-[var(--text-label)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-muted)]">
                  Constraints
                </span>
                {["Max 60s", "Include captions", "Hook in first 3s", "Auto reframe", "No watermarks"].map((constraint) => (
                  <Inline key={constraint} gap="2" align="center">
                    <span className="h-1.5 w-1.5 rounded-full bg-[var(--color-brand-secondary)]" />
                    <span className="text-[var(--text-caption)] text-[var(--color-text-secondary)]">{constraint}</span>
                  </Inline>
                ))}
              </Stack>
            </section>
            <section className="rounded-[var(--radius-md)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-card)] p-2">
              <Stack gap="2">
                <Inline justify="between" align="baseline">
                  <span className="text-[var(--text-label)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-muted)]">
                    Agent plan
                  </span>
                  <span className="font-mono text-[var(--text-caption)] text-[var(--color-brand-secondary)]">76%</span>
                </Inline>
                {[
                  "Analyze episode for viral moments",
                  "Score & rank potential clips",
                  "Create 5 short clip proposals",
                  "Review & package for delivery",
                ].map((step, index) => (
                  <Inline key={step} gap="2" align="start">
                    <span className="font-mono text-[var(--text-caption)] text-[var(--color-text-muted)]">{index + 1}</span>
                    <span className="text-[var(--text-caption)] leading-snug text-[var(--color-text-secondary)]">{step}</span>
                  </Inline>
                ))}
              </Stack>
            </section>
          </Stack>
        </div>
        <div className="border-t border-[var(--color-border-subtle)] p-2 shrink-0">
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

      <main className="grid min-h-0 grid-rows-[80px_minmax(190px,0.68fr)_minmax(0,1fr)_72px]">
        <div className="border-b border-[var(--color-border-subtle)] bg-[var(--color-surface-panel)] px-3 py-2">
          <Inline justify="between" align="start" gap="3">
            <Stack gap="1" className="min-w-0">
              <Inline gap="2" align="center">
                <span className="text-[var(--text-label)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-muted)]">
                  Proposal: Short Clips Pack v2
                </span>
                <Pill status="pending">Pending</Pill>
              </Inline>
              <p className="max-w-[720px] text-[var(--text-caption)] leading-relaxed text-[var(--color-text-secondary)]">
                Create 5 short, vertical clips with reframing, captions, titles, and pacing edits.
              </p>
            </Stack>
            <Inline gap="2" className="shrink-0">
              <SummaryMetric label="Confidence" value="83%" />
              <SummaryMetric label="Output count" value="5 clips" />
              <SummaryMetric label="Duration" value="~4m 32s" />
            </Inline>
          </Inline>
        </div>

        <section className="grid min-h-0 grid-cols-2 gap-2 border-b border-[var(--color-border-subtle)] bg-[var(--color-surface-app)] p-2">
          <FramePane label="Before (Source) · 16:9" content={beforeFrame} />
          <FramePane label="After (Proposed Clip) · 9:16" content={afterFrame} />
        </section>

        <section className="min-h-0 overflow-hidden bg-[var(--color-surface-app)] p-2">
          <Stack gap="2" className="h-full min-h-0">
            <Inline justify="between" align="center">
              <span className="text-[var(--text-label)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-secondary)]">
                Proposed changes — {proposals.length}
              </span>
              <span className="text-[var(--text-caption)] text-[var(--color-text-muted)]">
                Sorted by score
              </span>
            </Inline>
            <div className="min-h-0 overflow-hidden rounded-[var(--radius-md)] border border-[var(--color-border-subtle)]">
              <div className="grid grid-cols-[20px_24px_50px_minmax(190px,1fr)_64px_68px_46px_78px] gap-1.5 border-b border-[var(--color-border-subtle)] bg-[var(--color-surface-input)] px-2 py-1.5 text-[var(--text-caption)] font-semibold uppercase tracking-[var(--text-label--letter-spacing)] text-[var(--color-text-muted)]">
                <span />
                <span>#</span>
                <span>Preview</span>
                <span>Title / Hook</span>
                <span>Duration</span>
                <span>Platform</span>
                <span>Score</span>
                <span>Status</span>
              </div>
              <div className="max-h-full overflow-y-auto">
                {proposals.map((proposal, index) => (
                  <button
                    key={proposal.id}
                    type="button"
                    onClick={() => onSelectProposal?.(proposal)}
                    className={cn(
                      "grid w-full grid-cols-[20px_24px_50px_minmax(190px,1fr)_64px_68px_46px_78px] items-center gap-1.5 border-b border-[var(--color-border-subtle)] px-2 py-1.5 text-left last:border-b-0",
                      selectedProposal?.id === proposal.id
                        ? "bg-[var(--color-surface-selected)]"
                        : proposal.status === "rejected"
                          ? "bg-[rgba(239,68,68,0.045)] opacity-75"
                          : "bg-[var(--color-surface-card)] hover:bg-[var(--color-surface-card-hover)]",
                    )}
                  >
                    <span className="h-3.5 w-3.5 rounded-[var(--radius-xs)] border border-[var(--color-border)] bg-[var(--color-surface-input)]" />
                    <span className="font-mono text-[var(--text-caption)] text-[var(--color-text-muted)]">{index + 1}</span>
                    <span className="h-8 w-11 overflow-hidden rounded-[var(--radius-xs)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-input)]">
                      {proposal.thumbnail ? <img src={proposal.thumbnail} alt="" className="h-full w-full object-cover" /> : null}
                    </span>
                    <Stack gap="0" className="min-w-0">
                      <span className="truncate text-[var(--text-body-sm)] font-semibold text-[var(--color-text-primary)]">
                        {proposal.title}
                      </span>
                      <span className="truncate text-[var(--text-caption)] text-[var(--color-text-muted)]">
                        {proposal.explanation}
                      </span>
                    </Stack>
                    <span className="font-mono text-[var(--text-caption)] text-[var(--color-text-secondary)]">{clipDurations[index] ?? "00:48"}</span>
                    <span className="text-[var(--text-caption)] text-[var(--color-text-secondary)]">TikTok</span>
                    <span className="font-mono text-[var(--text-caption)] text-[var(--color-brand-secondary)]">
                      {typeof proposal.confidence === "number" ? Math.round(proposal.confidence * 100) : "—"}
                    </span>
                    <Pill status={proposal.status} dot={false}>{proposal.status}</Pill>
                  </button>
                ))}
              </div>
            </div>
          </Stack>
        </section>

        <ProposalReviewBand proposals={proposals} selectedProposalId={selectedProposal?.id} />
      </main>

      <aside className="border-l border-[var(--color-border-subtle)] bg-[var(--color-surface-panel)] flex flex-col min-h-0">
        <div className="h-9 px-2.5 flex items-center border-b border-[var(--color-border-subtle)] shrink-0">
          <span className="text-[var(--text-label)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-secondary)]">
            Batch insights
          </span>
        </div>
        <div className="flex-1 overflow-y-auto p-2">
          <Stack gap="3">
            <section className="rounded-[var(--radius-md)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-card)] p-2">
              <Stack gap="2">
                <span className="text-[var(--text-label)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-muted)]">
                  Review rationale
                </span>
                <p className="text-[var(--text-caption)] leading-relaxed text-[var(--color-text-secondary)]">
                  Identified 12 high-retention moments. Selected 5 clips with the strongest hooks, emotional response, and topic relevance for TikTok.
                </p>
              </Stack>
            </section>
            <section className="rounded-[var(--radius-md)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-card)] p-2">
              <Stack gap="2">
                <span className="text-[var(--text-label)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-muted)]">
                  Evidence highlights
                </span>
                <Inline gap="2" wrap="wrap">
                  {["Retention spike", "High engagement", "Speaker emphasis", "Topic relevance"].map((label) => (
                    <span key={label} className="rounded-full border border-[var(--color-border-subtle)] bg-[var(--color-surface-input)] px-2 py-1 text-[var(--text-caption)] text-[var(--color-text-secondary)]">
                      {label}
                    </span>
                  ))}
                </Inline>
              </Stack>
            </section>
            <section className="rounded-[var(--radius-md)] border border-[rgba(245,158,11,0.4)] bg-[rgba(245,158,11,0.075)] p-2">
              <Stack gap="2">
                <span className="text-[var(--text-label)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-warning)]">
                  Risk notes
                </span>
                <p className="text-[var(--text-caption)] leading-relaxed text-[var(--color-text-secondary)]">
                  Risk level: Moderate. Clip 5 may feel too long for TikTok. Clip 3 topic is niche; lower broad appeal.
                </p>
              </Stack>
            </section>
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
            <Stack gap="2">
              <Button variant="accept" onClick={onAcceptAll}>Accept all</Button>
              <Button
                variant="secondary"
                onClick={() => {
                  if (selectedProposal) onAcceptOne?.(selectedProposal);
                }}
              >
                Accept selected
              </Button>
              <Button
                variant="reject"
                onClick={() => {
                  if (selectedProposal) onRejectOne?.(selectedProposal);
                }}
              >
                Reject selected
              </Button>
              <Button variant="revise" onClick={onReviseAll}>Revise with prompt</Button>
              <Button
                variant="ghost"
                onClick={() => {
                  if (selectedProposal) onInspectDeeper?.(selectedProposal);
                }}
              >
                Inspect deeper
              </Button>
              <Button
                variant="ghost"
                onClick={() => {
                  if (selectedProposal) onSendFeedback?.(selectedProposal);
                }}
              >
                Send feedback to agent
              </Button>
            </Stack>
          </Stack>
        </div>
      </aside>
    </div>
  );
}

function SummaryMetric({ label, value }: { label: string; value: string }) {
  return (
    <Stack gap="0" className="min-w-[74px] rounded-[var(--radius-sm)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-card)] px-2 py-1.5">
      <span className="text-[var(--text-caption)] text-[var(--color-text-muted)]">{label}</span>
      <span className="font-mono text-[var(--text-body-sm)] text-[var(--color-text-primary)]">{value}</span>
    </Stack>
  );
}

function ProposalReviewBand({
  proposals,
  selectedProposalId,
}: {
  proposals: BatchProposal[];
  selectedProposalId?: string;
}) {
  return (
    <section className="border-t border-[var(--color-border-subtle)] bg-[var(--color-surface-panel)] p-2">
      <Stack gap="2">
        <Inline justify="between" align="center">
          <span className="text-[var(--text-label)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-muted)]">
            Proposal details
          </span>
          <Inline gap="2" align="center">
            <span className="h-1.5 w-1.5 rounded-full bg-[var(--color-viz-pending)]" />
            <span className="text-[var(--text-caption)] text-[var(--color-text-muted)]">Pending</span>
            <span className="h-1.5 w-1.5 rounded-full bg-[var(--color-viz-accepted)]" />
            <span className="text-[var(--text-caption)] text-[var(--color-text-muted)]">Accepted</span>
          </Inline>
        </Inline>
        <div className="overflow-hidden rounded-[var(--radius-md)] border border-[var(--color-border-subtle)]">
          <div className="grid grid-cols-[34px_minmax(150px,1fr)_104px_78px_78px] border-b border-[var(--color-border-subtle)] bg-[var(--color-surface-input)] px-2 py-1 text-[var(--text-caption)] font-semibold uppercase tracking-[var(--text-label--letter-spacing)] text-[var(--color-text-muted)]">
            <span>#</span>
            <span>Proposal</span>
            <span>Range</span>
            <span>Confidence</span>
            <span>Status</span>
          </div>
          {proposals.map((proposal, index) => (
            <div
              key={`band-${proposal.id}`}
              className={cn(
                "grid grid-cols-[34px_minmax(150px,1fr)_104px_78px_78px] items-center border-b border-[var(--color-border-subtle)] px-2 py-1 last:border-b-0",
                selectedProposalId === proposal.id
                  ? "bg-[var(--color-surface-selected)]"
                  : "bg-[var(--color-surface-card)]",
              )}
            >
              <span className="font-mono text-[var(--text-caption)] text-[var(--color-text-muted)]">
                {String(index + 1).padStart(2, "0")}
              </span>
              <span className="truncate text-[var(--text-body-sm)] font-medium text-[var(--color-text-primary)]">
                {proposal.title}
              </span>
              <span className="truncate font-mono text-[var(--text-caption)] text-[var(--color-text-muted)]">
                {proposal.timeRange.split(" – ")[0]}
              </span>
              <span className="font-mono text-[var(--text-caption)] text-[var(--color-text-secondary)]">
                {typeof proposal.confidence === "number" ? `${Math.round(proposal.confidence * 100)}%` : "—"}
              </span>
              <Pill status={proposal.status} dot={false}>{proposal.status}</Pill>
            </div>
          ))}
        </div>
        <div className="relative h-7 overflow-hidden rounded-[var(--radius-sm)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-input)]">
          {proposals.map((proposal, index) => (
            <span
              key={`marker-${proposal.id}`}
              className="absolute top-1 h-5 rounded-[var(--radius-xs)] border border-black/30"
              style={{
                left: `${8 + index * 24}%`,
                width: `${16 + index * 3}%`,
                background:
                  proposal.status === "accepted"
                    ? "rgba(34,197,94,0.58)"
                    : proposal.status === "reviewing"
                      ? "rgba(168,85,247,0.58)"
                      : "rgba(245,158,11,0.58)",
              }}
            />
          ))}
        </div>
      </Stack>
    </section>
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
