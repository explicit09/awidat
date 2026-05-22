import { forwardRef, type HTMLAttributes } from "react";
import { cva, type VariantProps } from "class-variance-authority";
import { cn } from "../cn";

const card = cva(
  [
    "rounded-[var(--radius-md)] border bg-[var(--color-surface-card)]",
    "border-[var(--color-border)]",
    "transition-[background-color,border-color,box-shadow] duration-[120ms]",
    "ease-[cubic-bezier(0.2,0,0,1)]",
  ],
  {
    variants: {
      interactive: {
        true: "hover:bg-[var(--color-surface-card-hover)] hover:border-[var(--color-border-strong)] cursor-pointer",
        false: "",
      },
      tone: {
        default: "",
        elevated: "bg-[var(--color-surface-modal)] elev-1",
        flat: "bg-transparent border-[var(--color-border-subtle)]",
        accent: "border-[var(--color-border-active)] glow-active",
        warning: "border-[var(--color-warning)] glow-warning",
        danger: "border-[var(--color-danger)] glow-danger",
      },
      padding: {
        none: "p-0",
        sm: "p-2",
        md: "p-2.5",
        lg: "p-3",
      },
    },
    defaultVariants: { interactive: false, tone: "default", padding: "md" },
  },
);

export type CardProps = HTMLAttributes<HTMLDivElement> & VariantProps<typeof card>;

export const Card = forwardRef<HTMLDivElement, CardProps>(function Card(
  { className, interactive, tone, padding, ...rest },
  ref,
) {
  return <div ref={ref} className={cn(card({ interactive, tone, padding }), className)} {...rest} />;
});
