/**
 * PrepareButton — "Prepare a starting cut" action (Wave 3 B4).
 *
 * Fires a synthetic `start_turn` with the [montage:prepare] sentinel so
 * the instruction is hidden from the chat transcript. After firing, the
 * button shows "Preparing…" until the agent emits its first pending
 * proposal — at which point the user is looking at proposals, not the
 * button.
 *
 * Per-project once-per-explicit-click: no auto-fire, no persistence.
 * The user can click it again at any time to ask for another pass.
 */

import { useState } from "react";
import { invoke, isTauri } from "@tauri-apps/api/core";
import { Button, type ButtonProps } from "../../ui";
import { useAgentStore } from "../../agent/store";
import { useBriefProposalsStore } from "../../state/briefProposals";
import { PREPARE_PROMPT } from "../../state/introState";

export interface PrepareButtonProps {
  variant?: ButtonProps["variant"];
  size?: ButtonProps["size"];
  className?: string;
  /** Override the label — defaults to "Prepare a starting cut". */
  label?: string;
  /** Override the in-flight label — defaults to "Preparing…". */
  inFlightLabel?: string;
}

export function PrepareButton({
  variant = "primary",
  size = "md",
  className,
  label = "Prepare a starting cut",
  inFlightLabel = "Preparing…",
}: PrepareButtonProps) {
  // Local in-flight flag — flips true on click and back to false once
  // either: a proposal lands (Brief renders, this button moves to the
  // header where the user re-clicks if they want), or the start_turn
  // call resolves/errors. We don't gate on backend response because
  // codex streams proposals piecemeal and there's no single "done" event.
  const [inFlight, setInFlight] = useState(false);
  const running = useAgentStore((s) => s.running);
  const pendingCount = useBriefProposalsStore((s) => s.pending().length);

  // As soon as the agent starts emitting pending work, drop the
  // in-flight flag — the user can see something is happening, no need
  // to keep the button latched.
  if (inFlight && pendingCount > 0) {
    queueMicrotask(() => setInFlight(false));
  }

  async function onClick() {
    if (!isTauri()) {
      // Demo / browser-preview environment — flash the in-flight state
      // so the click reads as registered, but skip the invoke.
      setInFlight(true);
      setTimeout(() => setInFlight(false), 600);
      return;
    }
    setInFlight(true);
    try {
      await invoke("start_turn", { input: PREPARE_PROMPT });
    } catch (e) {
      // If the engine refuses (a turn is already running), bounce to a
      // cancel + retry — mirrors `runEngineCommand` in App.tsx.
      if (String(e).includes("turn is already running")) {
        try {
          await invoke("cancel_turn");
          await invoke("start_turn", { input: PREPARE_PROMPT });
          return;
        } catch (retryErr) {
          console.warn("prepare retry failed", retryErr);
        }
      } else {
        console.warn("prepare start_turn failed", e);
      }
      setInFlight(false);
    }
  }

  const isDisabled = inFlight || running;

  return (
    <Button
      variant={variant}
      size={size}
      className={className}
      disabled={isDisabled}
      onClick={onClick}
      aria-label={label}
    >
      {inFlight ? inFlightLabel : label}
    </Button>
  );
}
