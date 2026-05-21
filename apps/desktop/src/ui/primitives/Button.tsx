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
          "bg-[var(--color-success-active)] border-[var(--color-success)] text-[#ECFDF5]",
          "hover:bg-[var(--color-success)] hover:border-[var(--color-success-hover)]",
        ],
        reject: [
          "bg-[#7F1D1D] border-[var(--color-danger)] text-[var(--color-text-on-danger)]",
          "hover:bg-[#991B1B] hover:border-[#F87171]",
        ],
        revise: [
          "bg-[var(--color-surface-modal)] border-[var(--color-border-strong)] text-[var(--color-text-primary)]",
          "hover:bg-[#281A4D] hover:border-[var(--color-brand-purple)] hover:text-[#DDD6FE]",
        ],
        repair: [
          "bg-[#3B0764] border-[var(--color-brand-purple)] text-[#EDE9FE]",
          "hover:bg-[#5B21B6]",
        ],
        danger: [
          "bg-[var(--color-failure)] border-[var(--color-failure)] text-[var(--color-text-on-danger)]",
          "hover:bg-[#B91C1C]",
        ],
      },
      size: {
        md: "h-[var(--layout-btn-h)] px-3 text-[var(--text-body)]",
        sm: "h-[var(--layout-btn-h-compact)] px-2.5 text-[var(--text-body-sm)]",
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
