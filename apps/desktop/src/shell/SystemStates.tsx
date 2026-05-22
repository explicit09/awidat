import { CircleAlert, Loader, RotateCw, Sparkles, type LucideIcon } from "lucide-react";
import type { ReactNode } from "react";
import { Button, Card, Inline, Stack, cn } from "../ui";

/**
 * SystemStates — Screen 8 from the design spec.
 *
 * Three primitives used across every stage:
 *   - LoadingState   — long-running work with current/up-next/progress
 *   - ErrorState     — what / impact / how-to-resolve + Agent repair + Retry
 *   - GenericEmpty   — generic empty pattern for local empty states
 *
 * Each renders centered inside the parent.
 */

export type LoadingStateProps = {
  title: string;
  currentTask?: string;
  upNext?: string;
  progress?: number;
  reassurance?: string;
  className?: string;
};

export function LoadingState({
  title,
  currentTask,
  upNext,
  progress,
  reassurance,
  className,
}: LoadingStateProps) {
  const determinate = typeof progress === "number";
  return (
    <CenterCard className={className}>
      <Stack gap="4" align="center">
        <span className="flex h-12 w-12 items-center justify-center rounded-full border border-[var(--color-border-subtle)] bg-[var(--color-surface-input)] text-[var(--color-brand)]">
          <Loader className="h-6 w-6 stroke-[1.5] animate-spin" />
        </span>
        <Stack gap="2" align="center" className="text-center">
          <span className="text-[var(--text-h2)] font-semibold text-[var(--color-text-primary)]">
            {title}
          </span>
          {currentTask ? (
            <span className="text-[var(--text-body-sm)] text-[var(--color-text-secondary)]">
              {currentTask}
            </span>
          ) : null}
        </Stack>
        <div className="w-full h-1.5 rounded-full bg-[var(--color-surface-input)] overflow-hidden">
          {determinate ? (
            <div
              className="h-full rounded-full bg-[var(--color-brand-secondary)] transition-[width] duration-[400ms] ease-[cubic-bezier(0.16,1,0.3,1)]"
              style={{ width: `${Math.max(0, Math.min(100, (progress ?? 0) * 100))}%` }}
            />
          ) : (
            <div className="h-full w-1/3 rounded-full bg-[var(--color-brand-secondary)] animate-pulse" />
          )}
        </div>
        {determinate ? (
          <span className="font-mono text-[var(--text-caption)] text-[var(--color-text-muted)]">
            {Math.round((progress ?? 0) * 100)}%
          </span>
        ) : null}
        {upNext ? (
          <Stack gap="1" align="center" className="text-center">
            <span className="text-[var(--text-caption)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-muted)]">
              Up next
            </span>
            <span className="text-[var(--text-body-sm)] text-[var(--color-text-secondary)]">{upNext}</span>
          </Stack>
        ) : null}
        {reassurance ? (
          <span className="text-[var(--text-caption)] text-[var(--color-text-muted)] text-center max-w-sm leading-relaxed">
            {reassurance}
          </span>
        ) : null}
      </Stack>
    </CenterCard>
  );
}

export type ErrorStateProps = {
  title: string;
  detail?: string;
  fixes?: string[];
  onAgentRepair?: () => void;
  onRetry?: () => void;
  onCancel?: () => void;
  className?: string;
};

export function ErrorState({
  title,
  detail,
  fixes,
  onAgentRepair,
  onRetry,
  onCancel,
  className,
}: ErrorStateProps) {
  return (
    <CenterCard tone="danger" className={className}>
      <Stack gap="4" align="center">
        <span className="flex h-12 w-12 items-center justify-center rounded-full border border-[var(--color-danger)] bg-[#3A1111] text-[var(--color-danger)]">
          <CircleAlert className="h-6 w-6 stroke-[1.5]" />
        </span>
        <Stack gap="2" align="center" className="text-center">
          <span className="text-[var(--text-h2)] font-semibold text-[var(--color-text-primary)]">{title}</span>
          {detail ? (
            <span className="text-[var(--text-body-sm)] text-[var(--color-text-secondary)] leading-relaxed">
              {detail}
            </span>
          ) : null}
        </Stack>
        {fixes && fixes.length > 0 ? (
          <Stack gap="1" className="w-full text-left">
            <span className="text-[var(--text-label)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-muted)]">
              How to resolve
            </span>
            {fixes.map((fix, i) => (
              <span
                key={i}
                className="text-[var(--text-body-sm)] text-[var(--color-text-secondary)] leading-snug"
              >
                · {fix}
              </span>
            ))}
          </Stack>
        ) : null}
        <Inline gap="2">
          {onAgentRepair ? (
            <Button
              variant="repair"
              onClick={onAgentRepair}
              leadingIcon={<Sparkles className="h-3.5 w-3.5 stroke-[1.75]" />}
            >
              Repair with agent
            </Button>
          ) : null}
          {onRetry ? (
            <Button
              variant="secondary"
              onClick={onRetry}
              leadingIcon={<RotateCw className="h-3.5 w-3.5 stroke-[1.75]" />}
            >
              Retry
            </Button>
          ) : null}
          {onCancel ? (
            <Button variant="ghost" onClick={onCancel}>
              Cancel
            </Button>
          ) : null}
        </Inline>
      </Stack>
    </CenterCard>
  );
}

export type GenericEmptyProps = {
  icon?: LucideIcon;
  title: string;
  description?: string;
  primaryLabel?: string;
  onPrimary?: () => void;
  secondaryLabel?: string;
  onSecondary?: () => void;
  className?: string;
};

export function GenericEmpty({
  icon: Icon,
  title,
  description,
  primaryLabel,
  onPrimary,
  secondaryLabel,
  onSecondary,
  className,
}: GenericEmptyProps) {
  return (
    <CenterCard tone="default" className={className}>
      <Stack gap="3" align="center">
        {Icon ? (
          <span className="flex h-12 w-12 items-center justify-center rounded-full border border-[var(--color-border-subtle)] bg-[var(--color-surface-input)] text-[var(--color-text-secondary)]">
            <Icon className="h-6 w-6 stroke-[1.5]" />
          </span>
        ) : null}
        <Stack gap="2" align="center" className="text-center">
          <span className="text-[var(--text-h2)] font-semibold text-[var(--color-text-primary)]">{title}</span>
          {description ? (
            <span className="text-[var(--text-body-sm)] text-[var(--color-text-secondary)] leading-relaxed">
              {description}
            </span>
          ) : null}
        </Stack>
        {(primaryLabel || secondaryLabel) ? (
          <Inline gap="2">
            {primaryLabel ? (
              <Button variant="primary" onClick={onPrimary}>
                {primaryLabel}
              </Button>
            ) : null}
            {secondaryLabel ? (
              <Button variant="ghost" onClick={onSecondary}>
                {secondaryLabel}
              </Button>
            ) : null}
          </Inline>
        ) : null}
      </Stack>
    </CenterCard>
  );
}

function CenterCard({
  tone = "default",
  className,
  children,
}: {
  tone?: "default" | "danger";
  className?: string;
  children: ReactNode;
}) {
  return (
    <div className={cn("flex h-full w-full items-center justify-center p-8", className)}>
      <Card tone={tone === "danger" ? "danger" : "elevated"} padding="lg" className="w-full max-w-[480px]">
        {children}
      </Card>
    </div>
  );
}
