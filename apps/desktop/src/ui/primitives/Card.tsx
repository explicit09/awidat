import { forwardRef, type HTMLAttributes } from "react";
import { cva, type VariantProps } from "class-variance-authority";
import { cn } from "../cn";

const card = cva(
  // Obsidian Glass 2026: translucent dark glass content surface. The
  // .glass-content class owns bg/border/radius + the hover lift (see
  // glass.css), so every Card that uses it inherits the new look.
  ["glass-content"],
  {
    variants: {
      interactive: {
        // .glass-content already brightens bg + border on hover; the
        // interactive variant only opts the row into pointer affordance.
        true: "cursor-pointer",
        false: "",
      },
      tone: {
        default: "",
        // Elevated reads through a stronger glass tint; the colored
        // tones wash a faint semantic hue over the glass without adding
        // a hard border or glow.
        elevated: "bg-[var(--glass-content-hover)]",
        flat: "bg-transparent border-transparent",
        accent: "bg-[rgba(56,189,248,0.07)]",
        warning: "bg-[rgba(245,158,11,0.07)]",
        danger: "bg-[rgba(220,100,95,0.07)]",
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
