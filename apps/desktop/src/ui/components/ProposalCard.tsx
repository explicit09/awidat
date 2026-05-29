import type { ReactNode } from "react";
import { MoreHorizontal } from "lucide-react";
import { Card } from "../primitives/Card";
import { StatusPill, type ProposalPillState } from "../primitives/StatusPill";
import { ConfidenceMeter } from "../primitives/ConfidenceMeter";
import { RiskIndicator, type RiskLevel } from "../primitives/RiskIndicator";
import { IconButton } from "../primitives/IconButton";
import { Inline, Stack } from "../primitives/Stack";
import { RationaleHint } from "./RationaleHint";
import { cn } from "../cn";

export type ProposalCardProps = {
  title: string;
  /**
   * Proposal-family pill state. Display label can be customized via
   * `statusLabel`; default labels are "Proposed/Accepted/Rejected/Revised".
   */
  proposalState: ProposalPillState;
  statusLabel?: string;
  timeRange: string;
  cutType?: string;
  explanation?: string;
  /**
   * One-sentence rationale ("why this specific edit"). Surfaced as a
   * small italic muted line between the title block and the
   * explanation/thumbnail block. Empty → not rendered. Full text in
   * a native title= tooltip. See `RationaleHint`.
   */
  rationale?: string;
  confidence?: number;
  risk?: RiskLevel;
  thumbnail?: string;
  thumbnailAlt?: string;
  selected?: boolean;
  onSelect?: () => void;
  onOverflow?: () => void;
  footer?: ReactNode;
  className?: string;
};

export function ProposalCard({
  title,
  proposalState,
  statusLabel,
  timeRange,
  cutType,
  explanation,
  rationale,
  confidence,
  risk,
  thumbnail,
  thumbnailAlt,
  selected,
  onSelect,
  onOverflow,
  footer,
  className,
}: ProposalCardProps) {
  return (
    <Card
      tone={selected ? "accent" : "default"}
      interactive={Boolean(onSelect)}
      onClick={onSelect}
      className={cn("min-w-[220px]", className)}
      aria-pressed={selected}
    >
      <Stack gap="3">
        <Inline justify="between" align="start" gap="3">
          <Stack gap="1" className="min-w-0">
            <span className="text-[var(--text-h3)] font-semibold text-[var(--color-text-primary)] truncate">
              {title}
            </span>
            <Inline gap="2" align="center">
              <span className="font-mono text-[var(--text-caption)] text-[var(--color-text-muted)]">
                {timeRange}
              </span>
              {cutType ? (
                <span className="text-[var(--text-caption)] text-[var(--color-text-secondary)]">{cutType}</span>
              ) : null}
            </Inline>
          </Stack>
          <Inline gap="1" align="center">
            <StatusPill family="proposal" state={proposalState} label={statusLabel} />
            {onOverflow ? (
              <IconButton
                size="sm"
                icon={<MoreHorizontal />}
                label="More actions"
                onClick={(e) => {
                  e.stopPropagation();
                  onOverflow();
                }}
              />
            ) : null}
          </Inline>
        </Inline>

        {/* Rationale — single-line italic justification between the
            title block and the thumbnail/explanation. Renders nothing
            when absent (handled inside `RationaleHint`). */}
        <RationaleHint rationale={rationale} />

        {thumbnail || explanation ? (
          <Inline gap="3" align="start">
            {thumbnail ? (
              <img
                src={thumbnail}
                alt={thumbnailAlt ?? ""}
                className="h-12 w-20 shrink-0 rounded-[var(--radius-sm)] border border-[var(--color-border-subtle)] object-cover bg-[var(--color-surface-input)]"
              />
            ) : null}
            {explanation ? (
              <p className="text-[var(--text-body-sm)] text-[var(--color-text-secondary)] leading-snug">
                {explanation}
              </p>
            ) : null}
          </Inline>
        ) : null}

        {confidence !== undefined || risk ? (
          <Stack gap="2">
            {confidence !== undefined ? <ConfidenceMeter score={confidence} size="sm" /> : null}
            {risk ? <RiskIndicator level={risk} /> : null}
          </Stack>
        ) : null}

        {footer}
      </Stack>
    </Card>
  );
}
