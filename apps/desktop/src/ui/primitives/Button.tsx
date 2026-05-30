import { forwardRef, type ButtonHTMLAttributes, type ReactNode } from "react";
import { cva, type VariantProps } from "class-variance-authority";
import { cn } from "../cn";

const button = cva(
  [
    "inline-flex items-center justify-center gap-2",
    "font-medium select-none whitespace-nowrap",
    "rounded-[var(--radius-md)] border border-transparent",
    "transition-[background-color,color] duration-[120ms]",
    "ease-[cubic-bezier(0.2,0,0,1)]",
    "focus-visible:outline-none",
    "disabled:opacity-40 disabled:cursor-not-allowed",
  ],
  {
    variants: {
      // Cursor-style: ghost by default, semantic fills only for
      // primary actions, no colored borders anywhere.
      variant: {
        primary: [
          "bg-[var(--color-brand)] text-[var(--color-text-inverse)]",
          "hover:bg-[var(--color-brand-hover)]",
          "active:bg-[var(--color-brand-active)]",
        ],
        secondary: [
          "bg-[var(--color-surface-card)] text-[var(--color-text-primary)]",
          "hover:bg-[var(--color-surface-card-hover)]",
          "active:bg-[var(--color-surface-card-active)]",
        ],
        ghost: [
          "bg-transparent text-[var(--color-text-secondary)]",
          "hover:bg-[var(--color-surface-hover)] hover:text-[var(--color-text-primary)]",
        ],
        accept: [
          "bg-[rgba(34,197,94,0.12)] text-[var(--color-proposal-accepted-text)]",
          "hover:bg-[rgba(34,197,94,0.18)]",
        ],
        reject: [
          "bg-[rgba(239,68,68,0.11)] text-[var(--color-proposal-rejected-text)]",
          "hover:bg-[rgba(239,68,68,0.17)]",
        ],
        revise: [
          "bg-[rgba(59,130,246,0.1)] text-[var(--color-proposal-proposed-text)]",
          "hover:bg-[rgba(59,130,246,0.16)] hover:text-[#DBEAFE]",
        ],
        repair: [
          "bg-[rgba(168,85,247,0.12)] text-[var(--color-proposal-proposed-text)]",
          "hover:bg-[rgba(168,85,247,0.18)]",
        ],
        danger: [
          "bg-[var(--color-failure)] text-[var(--color-text-on-danger)]",
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
