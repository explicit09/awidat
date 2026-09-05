// Themed wrapper around @radix-ui/react-popover. Used for the EDL
// preview popover today; designed for any future "click to open a
// floating panel anchored to a button" affordance.
//
// API: composable like the underlying radix primitive — caller
// brings their own trigger and content. We just theme the surface
// and apply the project's positioning conventions (collision
// padding, side offset).

import * as RadixPopover from "@radix-ui/react-popover";
import type { ReactNode } from "react";
import { clsx } from "clsx";
import "./popover.css";

export const Popover = RadixPopover.Root;
export const PopoverTrigger = RadixPopover.Trigger;

type ContentProps = {
  children: ReactNode;
  /** Extra className on the popover surface. Use for size overrides
   *  (e.g. a wider EDL viewer). */
  className?: string;
  /** Side to position relative to the trigger. */
  side?: RadixPopover.PopoverContentProps["side"];
  /** Alignment along the chosen side. */
  align?: RadixPopover.PopoverContentProps["align"];
  /** Pixel offset from the trigger. */
  sideOffset?: number;
};

export function PopoverContent({
  children,
  className,
  side = "bottom",
  align = "start",
  sideOffset = 6,
}: ContentProps) {
  return (
    <RadixPopover.Portal>
      <RadixPopover.Content
        className={clsx("montage-popover", className)}
        side={side}
        align={align}
        sideOffset={sideOffset}
        collisionPadding={8}
      >
        {children}
      </RadixPopover.Content>
    </RadixPopover.Portal>
  );
}
