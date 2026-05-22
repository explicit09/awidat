import { forwardRef, type HTMLAttributes } from "react";
import { cva, type VariantProps } from "class-variance-authority";
import { cn } from "../cn";

const stack = cva("flex", {
  variants: {
    direction: { row: "flex-row", col: "flex-col" },
    align: {
      start: "items-start",
      center: "items-center",
      end: "items-end",
      baseline: "items-baseline",
      stretch: "items-stretch",
    },
    justify: {
      start: "justify-start",
      center: "justify-center",
      end: "justify-end",
      between: "justify-between",
      around: "justify-around",
    },
    gap: {
      "0": "gap-0",
      "1": "gap-1",
      "2": "gap-2",
      "3": "gap-3",
      "4": "gap-4",
      "5": "gap-5",
      "6": "gap-6",
      "8": "gap-8",
    },
    wrap: { wrap: "flex-wrap", nowrap: "flex-nowrap" },
  },
  defaultVariants: { direction: "col", align: "stretch", justify: "start", gap: "2", wrap: "nowrap" },
});

export type StackProps = HTMLAttributes<HTMLDivElement> & VariantProps<typeof stack>;

export const Stack = forwardRef<HTMLDivElement, StackProps>(function Stack(
  { className, direction, align, justify, gap, wrap, ...rest },
  ref,
) {
  return <div ref={ref} className={cn(stack({ direction, align, justify, gap, wrap }), className)} {...rest} />;
});

export const Inline = forwardRef<HTMLDivElement, StackProps>(function Inline(
  { className, direction = "row", align = "center", gap = "2", ...rest },
  ref,
) {
  return <Stack ref={ref} direction={direction} align={align} gap={gap} className={className} {...rest} />;
});

export function Divider({ className, vertical = false }: { className?: string; vertical?: boolean }) {
  return (
    <hr
      aria-hidden
      className={cn(
        "border-0",
        vertical
          ? "h-full w-px bg-[var(--color-border-subtle)]"
          : "h-px w-full bg-[var(--color-border-subtle)]",
        className,
      )}
    />
  );
}
