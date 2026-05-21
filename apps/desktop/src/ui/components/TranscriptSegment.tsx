import type { ReactNode } from "react";
import { cva, type VariantProps } from "class-variance-authority";
import { Inline, Stack } from "../primitives/Stack";
import { cn } from "../cn";

const segment = cva(
  [
    "w-full text-left",
    "rounded-[var(--radius-md)] border",
    "px-3 py-2.5",
    "transition-[background-color,border-color] duration-[120ms] ease-[cubic-bezier(0.2,0,0,1)]",
    "focus-visible:outline-2 focus-visible:outline-[var(--color-border-focus)] focus-visible:outline-offset-1",
  ],
  {
    variants: {
      state: {
        default:
          "border-[var(--color-border-subtle)] bg-transparent hover:bg-[var(--color-surface-hover)] hover:border-[var(--color-border)]",
        selected:
          "border-[var(--color-border-focus)] bg-[var(--color-surface-selected)]",
        accepted:
          "border-[var(--color-success-active)] bg-[#0B2F1A]",
        rejected:
          "border-[var(--color-failure)] bg-[#3A1111]",
        warning:
          "border-[var(--color-warning)] bg-[#3A2605]",
      },
    },
    defaultVariants: { state: "default" },
  },
);

export type TranscriptSegmentProps = VariantProps<typeof segment> & {
  speaker: string;
  speakerColor?: string;
  startTime: string;
  text: ReactNode;
  evidence?: ReactNode;
  actions?: ReactNode;
  confidence?: number;
  onClick?: () => void;
  className?: string;
};

export function TranscriptSegment({
  speaker,
  speakerColor,
  startTime,
  text,
  evidence,
  actions,
  confidence,
  state,
  onClick,
  className,
}: TranscriptSegmentProps) {
  return (
    <button type="button" onClick={onClick} className={cn(segment({ state }), className)}>
      <Stack gap="2">
        <Inline justify="between" align="center" gap="3">
          <Inline gap="2" align="center">
            <span
              className="flex h-5 w-5 items-center justify-center rounded-full text-[var(--text-micro)] font-semibold text-white"
              style={{ backgroundColor: speakerColor ?? "var(--color-viz-speaker-a)" }}
              aria-hidden
            >
              {speaker.slice(0, 1).toUpperCase()}
            </span>
            <span className="text-[var(--text-caption)] font-medium text-[var(--color-text-secondary)]">
              {speaker}
            </span>
            <span className="font-mono text-[var(--text-caption)] text-[var(--color-text-muted)]">
              {startTime}
            </span>
          </Inline>
          {typeof confidence === "number" ? (
            <span className="font-mono text-[var(--text-caption)] text-[var(--color-text-muted)]">
              {Math.round(confidence * 100)}
            </span>
          ) : null}
        </Inline>
        <p className="text-[var(--text-body)] text-[var(--color-text-primary)] leading-relaxed">{text}</p>
        {evidence ? <Inline gap="2" wrap="wrap">{evidence}</Inline> : null}
        {actions ? <Inline gap="2">{actions}</Inline> : null}
      </Stack>
    </button>
  );
}
