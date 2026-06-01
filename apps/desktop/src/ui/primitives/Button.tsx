import { forwardRef, type ButtonHTMLAttributes, type ReactNode } from "react";
import { cva, type VariantProps } from "class-variance-authority";
import { cn } from "../cn";

const button = cva(
  [
    "inline-flex items-center justify-center gap-2",
    "font-medium select-none whitespace-nowrap",
    "rounded-[var(--radius-md)] border",
    "transition-[background-color,color,border-color,box-shadow,filter,transform] duration-[120ms]",
    "ease-[cubic-bezier(0.2,0,0,1)]",
    "focus-visible:outline-none",
    "disabled:opacity-40 disabled:cursor-not-allowed",
  ],
  {
    variants: {
      // Obsidian Glass 2026: primary is the radiating orange CTA
      // (glass-cta), everything else is frosted translucent glass
      // (glass-ghost) tinted toward its semantic hue on hover.
      variant: {
        // Radiating orange CTA with dark text — see .glass-cta in glass.css.
        primary: "glass-cta border-transparent",
        // Frosted translucent ghost that lights up on hover.
        secondary: "glass-ghost",
        ghost: "glass-ghost",
        accept: [
          "glass-ghost",
          "text-[var(--color-proposal-accepted-text)]",
          "hover:bg-[rgba(45,212,191,0.16)] hover:text-[var(--color-proposal-accepted-text)]",
          "hover:border-[rgba(45,212,191,0.3)]",
        ],
        reject: [
          "glass-ghost",
          "text-[var(--color-proposal-rejected-text)]",
          "hover:bg-[rgba(220,100,95,0.16)] hover:text-[var(--color-proposal-rejected-text)]",
          "hover:border-[rgba(220,100,95,0.3)]",
        ],
        revise: [
          "glass-ghost",
          "text-[var(--color-proposal-proposed-text)]",
          "hover:bg-[rgba(245,158,11,0.16)] hover:text-[var(--color-proposal-proposed-text)]",
          "hover:border-[rgba(245,158,11,0.3)]",
        ],
        repair: [
          "glass-ghost",
          "text-[var(--color-proposal-revised-text)]",
          "hover:bg-[rgba(168,85,247,0.16)] hover:text-[var(--color-proposal-revised-text)]",
          "hover:border-[rgba(168,85,247,0.3)]",
        ],
        danger: [
          "glass-ghost",
          "text-[var(--color-proposal-rejected-text)]",
          "hover:bg-[rgba(220,100,95,0.16)] hover:text-[#FCA5A5]",
          "hover:border-[rgba(220,100,95,0.45)]",
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
