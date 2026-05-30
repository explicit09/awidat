import { useState, useEffect, useRef, type ReactNode } from "react";
import { useMode } from "../../state/mode";

export type RevealLevel = "always" | "pro" | "advanced";

interface Props {
  title: string;
  /**
   * "always" — both modes default expanded.
   * "pro"     — expanded in Pro mode, collapsed in Creator mode.
   * "advanced"— always collapsed by default in both modes; user opens manually.
   */
  revealLevel?: RevealLevel;
  /** Per-instance override of the initial state; otherwise derived from mode + revealLevel. */
  initialOpen?: boolean;
  children: ReactNode;
}

export function CollapsiblePanel({ title, revealLevel = "always", initialOpen, children }: Props) {
  const mode = useMode((s) => s.mode);
  const initial = initialOpen ?? deriveInitialOpen(mode, revealLevel);
  const [open, setOpen] = useState(initial);

  const userTouched = useRef(false);
  useEffect(() => {
    if (!userTouched.current) setOpen(deriveInitialOpen(mode, revealLevel));
  }, [mode, revealLevel]);

  return (
    <section className="properties-section">
      <button
        type="button"
        onClick={() => { setOpen((v) => !v); userTouched.current = true; }}
        className="flex w-full items-center justify-between px-0 py-0 text-left bg-transparent border-0 cursor-pointer"
        aria-expanded={open}
      >
        <h3 className="m-0">{title}</h3>
        <span className="text-[var(--color-text-disabled)] text-[12px] mr-2" aria-hidden>
          {open ? "▾" : "▸"}
        </span>
      </button>
      {open && children}
    </section>
  );
}

function deriveInitialOpen(mode: "pro" | "creator", level: RevealLevel): boolean {
  if (level === "always") return true;
  if (level === "advanced") return false;
  return mode === "pro";
}
