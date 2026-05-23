import { forwardRef, type ButtonHTMLAttributes, type ReactNode } from "react";
import { cva, type VariantProps } from "class-variance-authority";
import { cn } from "../cn";

const iconBtn = cva(
  [
    "inline-flex items-center justify-center",
    "rounded-[var(--radius-sm)] border border-transparent",
    "text-[var(--color-text-muted)]",
    "transition-[background-color,color] duration-[120ms]",
    "ease-[cubic-bezier(0.2,0,0,1)]",
    "hover:bg-[var(--color-surface-hover)] hover:text-[var(--color-text-primary)]",
    "active:bg-[var(--color-surface-card-active)]",
    "focus-visible:outline-none",
    "disabled:opacity-40 disabled:cursor-not-allowed",
  ],
  {
    variants: {
      size: {
        sm: "h-6 w-6 [&>svg]:h-3.5 [&>svg]:w-3.5 [&>svg]:stroke-[1.75]",
        md: "h-7 w-7 [&>svg]:h-4 [&>svg]:w-4 [&>svg]:stroke-[1.75]",
        lg: "h-9 w-9 [&>svg]:h-5 [&>svg]:w-5 [&>svg]:stroke-[1.75]",
      },
      tone: {
        neutral: "",
        accent: "hover:text-[var(--color-brand-secondary)]",
        danger: "hover:text-[var(--color-danger)]",
      },
    },
    defaultVariants: { size: "md", tone: "neutral" },
  },
);

export type IconButtonProps = ButtonHTMLAttributes<HTMLButtonElement> &
  VariantProps<typeof iconBtn> & {
    icon: ReactNode;
    label: string;
  };

export const IconButton = forwardRef<HTMLButtonElement, IconButtonProps>(function IconButton(
  { className, size, tone, icon, label, type, ...rest },
  ref,
) {
  return (
    <button
      ref={ref}
      type={type ?? "button"}
      aria-label={label}
      title={label}
      className={cn(iconBtn({ size, tone }), className)}
      {...rest}
    >
      {icon}
    </button>
  );
});
