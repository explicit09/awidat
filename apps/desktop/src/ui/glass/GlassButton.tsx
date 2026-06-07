import type { ReactNode } from "react";

type Variant = "cta" | "ghost";

/**
 * GlassButton — brand light, not flat paint.
 *
 *   cta   → filled red that radiates a soft glow + inner specular line.
 *   ghost → frosted translucent that lights up on hover.
 *
 * `kbd` renders a trailing monospace keyboard hint chip.
 */
export function GlassButton({
  children,
  variant = "ghost",
  kbd,
  onClick,
  disabled,
  className = "",
  type = "button",
}: {
  children: ReactNode;
  variant?: Variant;
  kbd?: string;
  onClick?: () => void;
  disabled?: boolean;
  className?: string;
  type?: "button" | "submit";
}) {
  const base =
    "inline-flex h-9 items-center gap-2 rounded-xl px-4 text-[12px] font-semibold " +
    "transition disabled:opacity-40 disabled:pointer-events-none select-none";
  const variantClass = variant === "cta" ? "glass-cta" : "glass-ghost";
  const kbdClass =
    variant === "cta"
      ? "rounded-md border border-[rgba(26,14,4,0.30)] px-1.5 py-px font-mono text-[9px] text-[rgba(26,14,4,0.62)]"
      : "rounded-md border border-[var(--glass-border)] px-1.5 py-px font-mono text-[9px] text-[var(--color-text-muted)]";
  return (
    <button
      type={type}
      onClick={onClick}
      disabled={disabled}
      className={`${base} ${variantClass} ${className}`}
    >
      {children}
      {kbd ? <span className={kbdClass}>{kbd}</span> : null}
    </button>
  );
}
