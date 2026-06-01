import {
  AudioWaveform,
  Captions,
  ChevronRight,
  Eye,
  FileText,
  Info,
  Maximize2,
  PanelRightClose,
  SearchCheck,
  Users,
  VolumeX,
  type LucideIcon,
} from "lucide-react";
import type { ReactNode } from "react";
import {
  Button,
  ConfidenceMeter,
  ConfidenceRing,
  Inline,
  ReviewActions,
  RiskIndicator,
  Stack,
  StatusPill,
  cn,
  type ConfidenceLevel,
  type ProposalPillState,
  type RiskLevel,
} from "../ui";

/**
 * ProposalInspector — the right panel from the design spec (Screens 2 + 5).
 *
 * Renders the selected cut/proposal with optional confidence, risk, evidence,
 * explanation, and alternatives. All optional fields render only when present
 * so the UI degrades gracefully while the backend protocol catches up
 * (Phase 2.8). The shape mirrors the planned protocol extension.
 *
 * Hard rule from spec §10: every proposed edit must have a visible reason.
 * That means "Explanation" is shown even when other fields are missing, with
 * a fallback prompt to compute one if the agent didn't supply it.
 */

export type EvidenceKind =
  | "transcript"
  | "audio_energy"
  | "speaker_handoff"
  | "silence"
  | "pacing"
  | "filler"
  | "visual"
  | "cut_boundary";

const EVIDENCE_ICON: Record<EvidenceKind, LucideIcon> = {
  transcript: FileText,
  audio_energy: AudioWaveform,
  speaker_handoff: Users,
  silence: VolumeX,
  pacing: SearchCheck,
  filler: Captions,
  visual: Eye,
  cut_boundary: SearchCheck,
};

export type EvidenceItem = {
  id: string;
  kind: EvidenceKind;
  label: string;
  confidence?: number;
  confidenceLevel?: ConfidenceLevel;
};

export type Alternative = {
  id: string;
  label: string;
  detail?: string;
};

export type ProposalInspectorData = {
  title: string;
  /**
   * Proposal-family pill state. The inspector is always proposal-flavored,
   * so we restrict the surface to the proposal state set. Display string
   * can still be customized via `statusLabel`.
   */
  proposalState: ProposalPillState;
  statusLabel?: string;
  /** Time range in 24-frame TC string format (e.g. "00:06:35:21 - 00:06:42:05"). */
  timeRange?: string;
  /** Duration label, e.g. "6.7s". */
  duration?: string;
  /** Cut type label, e.g. "J-cut (audio leads)". */
  cutType?: string;
  /** Long-form agent intent text. */
  intent?: string;
  /** Long-form explanation. */
  explanation?: string;
  /**
   * Short-form rationale — the agent's one-sentence justification
   * for this proposal ("trimmed 0.42s silence per podcast defaults").
   * Wave 3 surfaces this on every proposal pill / Brief row;
   * the Inspector renders it as a compact "Rationale" row above
   * Confidence so reviewers see *why* before *how strong*.
   */
  rationale?: string;
  /** 0..1 — drives both Ring + Bar render. */
  confidence?: number;
  risk?: RiskLevel;
  evidence?: EvidenceItem[];
  alternatives?: Alternative[];
  /** "Compare alternatives" — show only if length > 0. */
  /** Optional sub-headers, e.g. "View all (3)". */
  alternativesCountLabel?: string;
};

export type ProposalInspectorProps = {
  data?: ProposalInspectorData;
  onAccept?: () => void;
  onReject?: () => void;
  onRevise?: () => void;
  onAgentRepair?: () => void;
  onInspectDeeper?: () => void;
  onSelectAlternative?: (alt: Alternative) => void;
  /** Fired when the user clicks the title-row maximize icon (opens the deep-dive view). */
  onMaximize?: () => void;
  onCollapse?: () => void;
};

export function ProposalInspector({
  data,
  onAccept,
  onReject,
  onRevise,
  onAgentRepair,
  onInspectDeeper,
  onSelectAlternative,
  onMaximize,
  onCollapse,
}: ProposalInspectorProps) {
  if (!data) {
    return (
      <Stack gap="3" className="p-3 h-full">
        <Header onCollapse={onCollapse} />
        <Stack gap="2" className="text-[var(--color-text-muted)]">
          <span className="text-[var(--text-body-sm)]">
            No proposal selected. Pick a change from the timeline to see why the agent suggested it.
          </span>
        </Stack>
      </Stack>
    );
  }

  return (
    <div className="flex h-full flex-col">
      <Header onMaximize={onMaximize} onCollapse={onCollapse} />

      <div className="flex-1 overflow-y-auto">
        <Stack gap="3" className="p-3 pb-2">
          {/* Title + status */}
          <Stack gap="2">
            <Inline justify="between" align="start" gap="2">
              <span className="text-[var(--text-h3)] font-semibold text-[var(--color-text-primary)]">
                {data.title}
              </span>
              <StatusPill family="proposal" state={data.proposalState} label={data.statusLabel} />
            </Inline>
            <KeyValue label="Time">
              <span className="font-mono text-[var(--text-body-sm)] text-[var(--color-text-primary)]">
                {data.timeRange}
                {data.duration ? (
                  <span className="text-[var(--color-text-muted)]"> ({data.duration})</span>
                ) : null}
              </span>
            </KeyValue>
            {data.cutType ? (
              <KeyValue label="Type">
                <span className="text-[var(--text-body-sm)] text-[var(--color-text-primary)]">{data.cutType}</span>
              </KeyValue>
            ) : null}
          </Stack>

          {/* Agent intent */}
          {data.intent ? (
            <Section title="Agent intent">
              <p className="text-[var(--text-body-sm)] text-[var(--color-text-secondary)] leading-relaxed">
                {data.intent}
              </p>
            </Section>
          ) : null}

          {/* Rationale — the one-sentence "why" surfaced on every
              proposal pill / Brief row. Placed above Confidence so
              reviewers see *why* before *how strong*. */}
          {data.rationale ? (
            <Section title="Rationale">
              <p className="text-[var(--text-body-sm)] text-[var(--color-text-primary)] leading-relaxed">
                {data.rationale}
              </p>
            </Section>
          ) : null}

          {/* Confidence */}
          {typeof data.confidence === "number" ? (
            <Section title="Confidence">
              <Inline gap="3" align="center">
                <ConfidenceRing score={data.confidence} size={48} thickness={4} />
                <div className="flex-1">
                  <ConfidenceMeter score={data.confidence} label="" size="sm" />
                </div>
              </Inline>
            </Section>
          ) : null}

          {/* Risk */}
          {data.risk ? (
            <Section title="Risk">
              <RiskIndicator level={data.risk} label="" />
            </Section>
          ) : null}

          {/* Evidence */}
          {data.evidence && data.evidence.length > 0 ? (
            <Section title="Evidence" trailing={<Info className="h-3 w-3 stroke-[1.75] text-[var(--color-text-muted)]" />}>
              <Stack gap="1">
                {data.evidence.map((e) => (
                  <EvidenceRow key={e.id} item={e} />
                ))}
              </Stack>
            </Section>
          ) : null}

          {/* Explanation */}
          {data.explanation ? (
            <Section title="Explanation">
              <p className="text-[var(--text-body-sm)] text-[var(--color-text-secondary)] leading-relaxed">
                {data.explanation}
              </p>
            </Section>
          ) : null}

          {/* Alternatives */}
          {data.alternatives && data.alternatives.length > 0 ? (
            <Section
              title="Alternatives"
              trailing={
                <button
                  type="button"
                  className="text-[var(--text-caption)] text-[var(--color-brand-secondary)] hover:underline"
                >
                  {data.alternativesCountLabel ?? `View all (${data.alternatives.length})`}
                </button>
              }
            >
              <Stack gap="1">
                {data.alternatives.map((alt) => (
                  <button
                    key={alt.id}
                    type="button"
                    onClick={() => onSelectAlternative?.(alt)}
                    className="text-left rounded-[var(--radius-sm)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-card)] hover:bg-[var(--color-surface-card-hover)] hover:border-[var(--color-border)] px-2.5 py-2 transition-colors"
                  >
                    <Inline justify="between" align="center" gap="2">
                      <span className="text-[var(--text-body-sm)] text-[var(--color-text-primary)]">{alt.label}</span>
                      {alt.detail ? (
                        <span className="font-mono text-[var(--text-caption)] text-[var(--color-text-muted)] shrink-0">
                          {alt.detail}
                        </span>
                      ) : null}
                    </Inline>
                  </button>
                ))}
              </Stack>
            </Section>
          ) : null}
        </Stack>
      </div>

      {/* Footer actions */}
      <div className="border-t border-[var(--color-border-subtle)] bg-[var(--color-surface-panel)] p-3 shrink-0">
        <Stack gap="2">
          <ReviewActions
            size="sm"
            onAccept={onAccept}
            onReject={onReject}
            onRevise={onRevise}
            onRepair={onAgentRepair}
          />
          <Button
            variant="secondary"
            size="sm"
            onClick={onInspectDeeper}
            className="!w-full"
            trailingIcon={<ChevronRight className="h-3.5 w-3.5 stroke-[1.75]" />}
          >
            Inspect deeper
          </Button>
        </Stack>
      </div>
    </div>
  );
}

function Header({ onMaximize, onCollapse }: { onMaximize?: () => void; onCollapse?: () => void } = {}) {
  return (
    <div className="h-9 px-3 border-b border-[var(--color-border-subtle)] flex items-center justify-between shrink-0">
      <span className="text-[var(--text-label)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-secondary)]">
        Proposal Inspector
      </span>
      <Inline gap="1" align="center">
        {onMaximize ? (
          <button
            type="button"
            onClick={onMaximize}
            className="h-6 w-6 inline-flex items-center justify-center rounded-[var(--radius-sm)] text-[var(--color-text-muted)] hover:bg-[var(--color-surface-hover)] hover:text-[var(--color-text-primary)] transition-colors"
            aria-label="Open detail view"
          >
            <Maximize2 className="h-3 w-3 stroke-[1.75]" />
          </button>
        ) : null}
        {onCollapse ? (
          <button
            type="button"
            onClick={onCollapse}
            className="h-6 w-6 inline-flex items-center justify-center rounded-[var(--radius-sm)] text-[var(--color-text-muted)] hover:bg-[var(--color-surface-hover)] hover:text-[var(--color-text-primary)] transition-colors"
            aria-label="Collapse"
          >
            <PanelRightClose className="h-3 w-3 stroke-[1.75]" />
          </button>
        ) : null}
      </Inline>
    </div>
  );
}

function Section({ title, trailing, children }: { title: string; trailing?: ReactNode; children: ReactNode }) {
  return (
    <Stack gap="2">
      <Inline justify="between" align="center" gap="2">
        <span className="text-[var(--text-label)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-muted)]">
          {title}
        </span>
        {trailing}
      </Inline>
      <div>{children}</div>
    </Stack>
  );
}

function KeyValue({ label, children }: { label: string; children: ReactNode }) {
  return (
    <Inline gap="2" align="baseline">
      <span className="text-[var(--text-caption)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-muted)] w-12 shrink-0">
        {label}
      </span>
      <span>{children}</span>
    </Inline>
  );
}

function EvidenceRow({ item }: { item: EvidenceItem }) {
  const Icon = EVIDENCE_ICON[item.kind];
  const level = item.confidenceLevel ?? (item.confidence !== undefined ? toLevel(item.confidence) : undefined);
  return (
    <div className="flex items-center justify-between gap-2 rounded-[var(--radius-sm)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-card)] px-2.5 py-1.5">
      <Inline gap="2" align="center" className="min-w-0">
        <span className="text-[var(--color-text-muted)] shrink-0">
          <Icon className="h-3.5 w-3.5 stroke-[1.75]" />
        </span>
        <span className="text-[var(--text-body-sm)] text-[var(--color-text-primary)] truncate">{item.label}</span>
      </Inline>
      {level ? (
        <span
          className={cn(
            "text-[var(--text-caption)] uppercase font-semibold tracking-[var(--text-label--letter-spacing)] shrink-0",
          )}
          style={{ color: LEVEL_COLOR[level] }}
        >
          {LEVEL_LABEL[level]}
        </span>
      ) : null}
    </div>
  );
}

const LEVEL_LABEL: Record<ConfidenceLevel, string> = {
  high: "High",
  medium: "Medium",
  low: "Low",
  "very-low": "Very low",
};

const LEVEL_COLOR: Record<ConfidenceLevel, string> = {
  high: "var(--color-success)",
  medium: "var(--color-warning)",
  low: "var(--color-risk)",
  "very-low": "var(--color-danger)",
};

function toLevel(score: number): ConfidenceLevel {
  if (score >= 0.8) return "high";
  if (score >= 0.55) return "medium";
  if (score >= 0.3) return "low";
  return "very-low";
}

