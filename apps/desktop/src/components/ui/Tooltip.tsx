// Themed wrapper around @radix-ui/react-tooltip. Replaces the
// hand-rolled `title=` attrs scattered through the app with a
// viewport-aware, themable hover tooltip.
//
// API: `<Tooltip content="...">{children}</Tooltip>`. The first
// child must be focusable (a button, link, etc.) — radix forwards
// the trigger props onto it via Slot. Wrap a non-focusable element
// with a `<span tabIndex={0}>` if you really need to.
//
// Theming: amber accent on a dark surface, matching the rest of the
// app. Lives in component-local CSS so we don't have to thread
// styles through App.css.

import * as RadixTooltip from "@radix-ui/react-tooltip";
import type { ReactNode } from "react";
import "./tooltip.css";

type Props = {
  /** Content to render inside the tooltip. String for short text;
   *  ReactNode if you need formatting. */
  content: ReactNode;
  /** The trigger element. Must be focusable. */
  children: ReactNode;
  /** Side to position the tooltip relative to the trigger. Default
   *  "top" matches what most `title=` browsers did. */
  side?: RadixTooltip.TooltipContentProps["side"];
  /** Pixel offset from the trigger. Default 6. */
  sideOffset?: number;
  /** Open-delay in ms. Default 250 — short enough for affordance,
   *  long enough not to flash on incidental hover. */
  delayDuration?: number;
};

export function Tooltip({
  content,
  children,
  side = "top",
  sideOffset = 6,
  delayDuration = 250,
}: Props) {
  return (
    <RadixTooltip.Provider delayDuration={delayDuration}>
      <RadixTooltip.Root>
        <RadixTooltip.Trigger asChild>{children}</RadixTooltip.Trigger>
        <RadixTooltip.Portal>
          <RadixTooltip.Content
            className="awidat-tooltip"
            side={side}
            sideOffset={sideOffset}
            collisionPadding={8}
          >
            {content}
            <RadixTooltip.Arrow className="awidat-tooltip-arrow" />
          </RadixTooltip.Content>
        </RadixTooltip.Portal>
      </RadixTooltip.Root>
    </RadixTooltip.Provider>
  );
}
