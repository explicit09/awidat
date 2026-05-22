import { forwardRef, type ButtonHTMLAttributes, type ReactNode } from "react";
import { cva, type VariantProps } from "class-variance-authority";
import { cn } from "../cn";

const button = cva(
  [
    "inline-flex items-center justify-center gap-2",
    "font-medium select-none whitespace-nowrap",
    "rounded-[var(--radius-md)] border",
    "transition-[background-color,border-color,box-shadow,color] duration-[120ms]",
    "ease-[cubic-bezier(0.2,0,0,1)]",
    "focus-visible:outline-2 focus-visible:outline-[var(--color-border-focus)] focus-visible:outline-offset-1",
    "disabled:opacity-50 disabled:cursor-not-allowed",
  ],
  {
    variants: {
      variant: {
        primary: [
          "bg-[var(--color-brand)] border-[var(--color-brand)] text-[var(--color-text-inverse)]",
          "hover:bg-[var(--color-brand-hover)] hover:border-[var(--color-brand-hover)]",
          "active:bg-[var(--color-brand-active)] active:border-[var(--color-brand-active)]",
        ],
        secondary: [
          "bg-[var(--color-surface-card)] border-[var(--color-border)] text-[var(--color-text-primary)]",
          "hover:bg-[var(--color-surface-card-hover)] hover:border-[var(--color-border-strong)]",
          "active:bg-[var(--color-surface-card-active)]",
        ],
        ghost: [
          "bg-transparent border-transparent text-[var(--color-text-primary)]",
          "hover:bg-[var(--color-surface-hover)]",
        ],
        accept: [
          "bg-[rgba(34,197,94,0.14)] border-[rgba(34,197,94,0.46)] text-[var(--color-pill-accepted-text)]",
          "hover:bg-[rgba(34,197,94,0.2)] hover:border-[rgba(74,222,128,0.58)]",
        ],
        reject: [
          "bg-[rgba(239,68,68,0.13)] border-[rgba(239,68,68,0.46)] text-[var(--color-pill-rejected-text)]",
          "hover:bg-[rgba(239,68,68,0.2)] hover:border-[rgba(248,113,113,0.58)]",
        ],
        revise: [
          "bg-[rgba(59,130,246,0.12)] border-[rgba(59,130,246,0.55)] text-[var(--color-pill-proposed-text)]",
          "hover:bg-[rgba(59,130,246,0.2)] hover:border-[var(--color-border-strong)] hover:text-[#DBEAFE]",
        ],
        repair: [
          "bg-[rgba(168,85,247,0.14)] border-[rgba(168,85,247,0.55)] text-[var(--color-pill-reviewing-text)]",
          "hover:bg-[rgba(168,85,247,0.22)]",
        ],
        danger: [
          "bg-[var(--color-failure)] border-[var(--color-failure)] text-[var(--color-text-on-danger)]",
          "hover:bg-[#B91C1C]",
        ],
      },
      size: {
        md: "h-[var(--layout-btn-h)] px-2.5 text-[var(--text-body-sm)]",
        sm: "h-[var(--layout-btn-h-compact)] px-2 text-[var(--text-caption)]",
        xs: "h-5 px-1.5 text-[var(--text-caption)] gap-1",
      },
    },
    defaultVariants: {
      variant: "primary",
      size: "md",
    },
  },
);

export type ButtonProps = ButtonHTMLAttributes<HTMLButtonElement> &
  VariantProps<typeof button> & {
    leadingIcon?: ReactNode;
    trailingIcon?: ReactNode;
  };

export const Button = forwardRef<HTMLButtonElement, ButtonProps>(function Button(
  { className, variant, size, leadingIcon, trailingIcon, children, type, ...rest },
  ref,
) {
  return (
    <button
      ref={ref}
      type={type ?? "button"}
      className={cn(button({ variant, size }), className)}
      {...rest}
    >
      {leadingIcon}
      {children}
      {trailingIcon}
    </button>
  );
});
