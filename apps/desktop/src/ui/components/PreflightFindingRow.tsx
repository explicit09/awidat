import { CircleCheck, CircleX, Info, OctagonX, TriangleAlert } from "lucide-react";
import type { ReactNode } from "react";
import { Inline, Stack } from "../primitives/Stack";
import { cn } from "../cn";

export type PreflightSeverity = "pass" | "info" | "warning" | "error" | "failure";

const META: Record<PreflightSeverity, { color: string; icon: ReactNode; label: string }> = {
  pass: { color: "var(--color-success)", icon: <CircleCheck />, label: "Pass" },
  info: { color: "var(--color-info)", icon: <Info />, label: "Info" },
  warning: { color: "var(--color-warning)", icon: <TriangleAlert />, label: "Warning" },
  error: { color: "var(--color-danger)", icon: <CircleX />, label: "Error" },
  failure: { color: "var(--color-failure)", icon: <OctagonX />, label: "Failure" },
};

export type PreflightFindingRowProps = {
  severity: PreflightSeverity;
  time?: string;
  message: string;
  asset?: string;
  suggestedFix?: string;
  actions?: ReactNode;
  className?: string;
  compact?: boolean;
  onClick?: () => void;
};

export function PreflightFindingRow({
  severity,
  time,
  message,
  asset,
  suggestedFix,
  actions,
  className,
  compact = false,
  onClick,
}: PreflightFindingRowProps) {
  const meta = META[severity];
  const isInteractive = Boolean(onClick);
  const Wrapper = isInteractive ? "button" : "div";
  return (
    <Wrapper
      type={isInteractive ? "button" : undefined}
      onClick={onClick}
      className={cn(
        "w-full text-left",
        "flex items-start gap-3",
        "rounded-[var(--radius-md)] border border-[var(--color-border-subtle)]",
        "bg-[var(--color-surface-card)]",
        compact ? "px-2.5 py-1" : "px-3 py-2.5",
        isInteractive && "hover:bg-[var(--color-surface-card-hover)] cursor-pointer",
        className,
      )}
    >
      <span
        className={cn(
          "mt-0.5 flex shrink-0 items-center justify-center",
          compact ? "h-4 w-4 [&_svg]:h-3.5 [&_svg]:w-3.5" : "h-5 w-5 [&_svg]:h-4 [&_svg]:w-4",
        )}
        style={{ color: meta.color }}
        aria-label={meta.label}
      >
        {meta.icon}
      </span>
      <Stack gap={compact ? "0" : "1"} className="min-w-0 flex-1">
        <Inline gap="2" align="center" wrap="wrap">
          <span className={cn(compact ? "text-[var(--text-body-sm)]" : "text-[var(--text-body)]", "text-[var(--color-text-primary)]")}>{message}</span>
          {time ? (
            <span className="font-mono text-[var(--text-caption)] text-[var(--color-text-muted)]">{time}</span>
          ) : null}
        </Inline>
        {compact && (asset || suggestedFix) ? (
          <span className="truncate text-[var(--text-caption)] text-[var(--color-text-muted)]">
            {asset}
            {suggestedFix ? (
              <>
                <span className="text-[var(--color-text-disabled)]"> · </span>
                <span className="font-semibold uppercase tracking-[var(--text-label--letter-spacing)]">Fix</span>
                <span className="text-[var(--color-text-disabled)]"> · </span>
                <span className="text-[var(--color-text-secondary)]">{suggestedFix}</span>
              </>
            ) : null}
          </span>
        ) : asset ? (
          <span className="text-[var(--text-caption)] text-[var(--color-text-muted)]">{asset}</span>
        ) : null}
        {!compact && suggestedFix ? (
          <span className="text-[var(--text-caption)] text-[var(--color-text-secondary)]">
            <span className="font-semibold uppercase tracking-[var(--text-label--letter-spacing)] text-[var(--color-text-muted)]">
              Fix
            </span>{" "}
            · {suggestedFix}
          </span>
        ) : null}
      </Stack>
      {actions ? <Inline gap="2" className="shrink-0">{actions}</Inline> : null}
    </Wrapper>
  );
}
