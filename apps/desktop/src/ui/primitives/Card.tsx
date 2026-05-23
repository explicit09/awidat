import { forwardRef, type HTMLAttributes } from "react";
import { cva, type VariantProps } from "class-variance-authority";
import { cn } from "../cn";

const card = cva(
  [
    "rounded-[var(--radius-md)] border bg-[var(--color-surface-card)]",
    "border-[var(--color-border-subtle)]",
    "transition-[background-color,color] duration-[120ms]",
    "ease-[cubic-bezier(0.2,0,0,1)]",
  ],
  {
    variants: {
      interactive: {
        // Hover changes only background, not the border color. Border
        // stays subtle so neighboring cards don't visually "pop" on
        // each hover.
        true: "hover:bg-[var(--color-surface-card-hover)] cursor-pointer",
        false: "",
      },
      tone: {
        default: "",
        // Elevated reads through stronger background tint, not a
        // colored shadow. Same for tone variants — color through fill,
        // not border or glow.
        elevated: "bg-[var(--color-surface-modal)]",
        flat: "bg-transparent",
        accent: "bg-[rgba(56,189,248,0.05)]",
        warning: "bg-[rgba(245,158,11,0.05)]",
        danger: "bg-[rgba(239,68,68,0.05)]",
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
