import type { HTMLAttributes, ReactNode } from "react";
import { cn } from "../cn";

export type EvidenceChipProps = HTMLAttributes<HTMLSpanElement> & {
  icon?: ReactNode;
  label: ReactNode;
  confidence?: number;
};

export function EvidenceChip({ icon, label, confidence, className, ...rest }: EvidenceChipProps) {
  return (
    <span
      className={cn(
        // Obsidian Glass 2026: small translucent glass chip.
        "inline-flex items-center gap-1.5 h-6 px-2",
        "rounded-[var(--radius-sm)] border border-[var(--glass-border)]",
        "bg-[rgba(255,255,255,0.05)] text-[var(--color-text-secondary)]",
        "transition-[background-color,border-color] duration-[120ms]",
        "hover:bg-[rgba(255,255,255,0.08)] hover:border-[var(--glass-border-strong)]",
        "text-[var(--text-caption)] leading-[1] tracking-[var(--text-caption--letter-spacing)]",
        "font-medium whitespace-nowrap",
        className,
      )}
      {...rest}
    >
      {icon ? <span className="text-[var(--color-text-muted)] [&_svg]:h-3 [&_svg]:w-3">{icon}</span> : null}
      <span>{label}</span>
      {typeof confidence === "number" ? (
        <span className="font-mono text-[var(--color-text-muted)]">{Math.round(confidence * 100)}</span>
      ) : null}
    </span>
  );
}
