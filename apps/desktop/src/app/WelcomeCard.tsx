// WelcomeCard - one-screen first-launch welcome.
//
// Montage is structurally unusual: a local-first AI editorial NLE where
// the agent has READ your media and AGENTS.md, proposes editorial
// work, and the human reviews/accepts/rejects. New users don't have a
// mental model for this. The welcome explains the three core ideas in
// one screen: read once, dismiss, done.
//
// Storage lives in `useWelcome`. The shell uses the shared glass system
// so first launch feels like the rest of the Montage desktop surface.

import { useEffect } from "react";
import mark from "../brand/montage-icon.png";
import { useWelcome } from "../state/welcome";

const CORE_IDEAS: ReadonlyArray<{ step: string; title: string; body: string }> = [
  {
    step: "01",
    title: "An agent reads your media.",
    body: "Drop a file and Montage indexes transcript, scenes, speakers, and silences. Your editorial brief lives in AGENTS.md.",
  },
  {
    step: "02",
    title: "It proposes editorial cuts with rationale.",
    body: "The agent suggests trims, B-roll, color, and captions, each with a one-sentence reason. You see them in the Brief and on the timeline as ghost overlays.",
  },
  {
    step: "03",
    title: "You accept, reject, or revise inline.",
    body: "Every decision becomes part of the History. Nothing happens without your call.",
  },
];

export function WelcomeCard() {
  const isOpen = useWelcome((s) => s.isOpen);
  const dismiss = useWelcome((s) => s.dismiss);

  // Cmd+W / Esc dismisses. Registered at the document level so they win
  // regardless of focus; only mounts while the modal is open.
  useEffect(() => {
    if (!isOpen) return;
    function onKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape") {
        event.preventDefault();
        dismiss();
        return;
      }
      const meta = event.metaKey || event.ctrlKey;
      if (meta && (event.key === "w" || event.key === "W")) {
        event.preventDefault();
        dismiss();
      }
    }
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [isOpen, dismiss]);

  if (!isOpen) return null;

  return (
    <div className="modal-backdrop" onClick={dismiss} role="presentation">
      <div
        className="glass glass-strong flex flex-col overflow-hidden text-[var(--color-text-primary)]"
        onClick={(event) => event.stopPropagation()}
        style={{
          width: "min(640px, calc(100vw - 48px))",
          maxHeight: "min(520px, calc(100vh - 48px))",
          borderRadius: 14,
          boxShadow: "0 24px 80px rgba(0,0,0,0.58), 0 0 0 1px rgba(239,68,68,0.14)",
        }}
        role="dialog"
        aria-modal="true"
        aria-label="Welcome to Montage"
      >
        <header className="flex items-center justify-between border-b border-[var(--glass-border)] bg-[rgba(10,10,14,0.58)] px-4 py-3">
          <div className="flex min-w-0 items-center gap-2.5">
            <img
              src={mark}
              alt=""
              width={20}
              height={20}
              className="drop-shadow-[0_0_14px_rgba(239,68,68,0.38)]"
            />
            <h2 className="truncate text-[17px] font-bold tracking-normal text-[var(--color-text-primary)]">
              Welcome to Montage
            </h2>
          </div>
          <button
            type="button"
            className="glass-content grid h-8 w-8 place-items-center rounded-lg text-[18px] leading-none text-[var(--color-text-secondary)] transition-colors hover:text-[var(--color-text-primary)]"
            onClick={dismiss}
            aria-label="Dismiss welcome"
          >
            ×
          </button>
        </header>
        <div className="flex flex-col gap-2.5 bg-[rgba(8,9,12,0.26)] p-4">
          {CORE_IDEAS.map((idea, index) => (
            <IdeaCard key={index} step={idea.step} title={idea.title} body={idea.body} />
          ))}
        </div>
        <footer className="flex items-center justify-end gap-3 border-t border-[var(--glass-border)] bg-[rgba(10,10,14,0.52)] px-4 py-3">
          <span
            className="font-mono text-[var(--text-caption)] text-[var(--color-text-muted)]"
            title="Dismiss"
          >
            ⌘W or Esc
          </span>
          <button
            type="button"
            className="glass-cta rounded-lg px-4 py-2 text-[12px] font-semibold tracking-normal"
            onClick={dismiss}
          >
            Get started
          </button>
        </footer>
      </div>
    </div>
  );
}

/** One of the three core-idea cards. */
function IdeaCard({ step, title, body }: { step: string; title: string; body: string }) {
  return (
    <div className="glass-content flex items-start gap-3 rounded-lg border-l border-[rgba(239,68,68,0.42)] p-3 pl-3.5">
      <span
        className="mt-0.5 w-6 shrink-0 font-mono text-[10px] font-semibold leading-4 text-[var(--color-brand)]"
        aria-hidden
      >
        {step}
      </span>
      <div className="min-w-0 flex-1">
        <span className="text-[var(--text-body-sm)] font-semibold text-[var(--color-text-primary)]">
          {title}
        </span>
        <span className="mt-0.5 block text-[var(--text-body-sm)] leading-snug text-[var(--color-text-secondary)]">
          {body}
        </span>
      </div>
    </div>
  );
}
