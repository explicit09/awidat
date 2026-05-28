import type { ReactNode } from "react";
import {
  StatusPillFromMapping,
  type StatusPillMapping,
} from "../primitives/StatusPill";
import { Inline, Stack } from "../primitives/Stack";
import { cn } from "../cn";

export type MediaIndexingStatus =
  | "imported"
  | "indexing"
  | "processing"
  | "queued"
  | "indexed"
  | "partial"
  | "failed"
  | "missing";

// Map each indexing status to a (family, state) pair on the new StatusPill API.
// `processing`/`queued` were `reviewing` (awaiting human action) → proposal/proposed.
// `partial` was `warning` — lossy: becomes job/failed visually per Task 4 mapping.
// `missing` becomes job/idle.
const PILL_FOR: Record<MediaIndexingStatus, StatusPillMapping> = {
  imported: { family: "job", state: "ready" },
  indexing: { family: "job", state: "running" },
  processing: { family: "proposal", state: "proposed" },
  queued: { family: "proposal", state: "proposed" },
  indexed: { family: "job", state: "ready" },
  partial: { family: "job", state: "failed" },
  failed: { family: "job", state: "failed" },
  missing: { family: "job", state: "idle" },
};

const LABEL: Record<MediaIndexingStatus, string> = {
  imported: "Imported",
  indexing: "Indexing",
  processing: "Processing",
  queued: "Queued",
  indexed: "Indexed",
  partial: "Partial",
  failed: "Failed",
  missing: "Missing",
};

export type MediaStatusRowProps = {
  icon?: ReactNode;
  thumbnail?: string;
  thumbnailAlt?: string;
  title: string;
  detail?: string;
  status: MediaIndexingStatus;
  progress?: number;
  meta?: ReactNode;
  className?: string;
  onClick?: () => void;
};

export function MediaStatusRow({
  icon,
  thumbnail,
  thumbnailAlt,
  title,
  detail,
  status,
  progress,
  meta,
  className,
  onClick,
}: MediaStatusRowProps) {
  const isInteractive = Boolean(onClick);
  const Wrapper = isInteractive ? "button" : "div";
  return (
    <Wrapper
      type={isInteractive ? "button" : undefined}
      onClick={onClick}
      className={cn(
        "w-full text-left",
        "flex items-center gap-3",
        "rounded-[var(--radius-md)] border border-[var(--color-border-subtle)]",
        "bg-[var(--color-surface-card)] px-3 py-2.5",
        "transition-[background-color,border-color] duration-[120ms]",
        isInteractive && "hover:bg-[var(--color-surface-card-hover)] hover:border-[var(--color-border)] cursor-pointer",
        className,
      )}
    >
      {thumbnail ? (
        <img
          src={thumbnail}
          alt={thumbnailAlt ?? ""}
          className="h-10 w-16 shrink-0 rounded-[var(--radius-sm)] border border-[var(--color-border-subtle)] object-cover bg-[var(--color-surface-input)]"
        />
      ) : icon ? (
        <span className="flex h-9 w-9 shrink-0 items-center justify-center rounded-[var(--radius-sm)] bg-[var(--color-surface-input)] text-[var(--color-text-secondary)] [&>svg]:h-4 [&>svg]:w-4 [&>svg]:stroke-[1.75]">
          {icon}
        </span>
      ) : null}
      <Stack gap="1" className="min-w-0 flex-1">
        <span className="text-[var(--text-body)] font-medium text-[var(--color-text-primary)] truncate" title={title}>
          {title}
        </span>
        {detail ? (
          <span className="text-[var(--text-caption)] text-[var(--color-text-muted)] truncate" title={detail}>{detail}</span>
        ) : null}
        {typeof progress === "number" ? (
          <div className="mt-1 h-1 w-full overflow-hidden rounded-full bg-[var(--color-surface-input)]">
            <div
              className="h-full rounded-full bg-[var(--color-processing)] transition-[width] duration-[400ms] ease-[cubic-bezier(0.16,1,0.3,1)]"
              style={{ width: `${Math.max(0, Math.min(100, progress))}%` }}
            />
          </div>
        ) : null}
      </Stack>
      <Inline gap="2" align="center" className="shrink-0 max-w-[45%]">
        {meta}
        <StatusPillFromMapping mapping={PILL_FOR[status]} label={LABEL[status]} />
      </Inline>
    </Wrapper>
  );
}
