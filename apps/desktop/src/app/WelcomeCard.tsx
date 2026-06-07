// WelcomeCard — one-screen first-launch welcome (Wave 3 W1).
//
// Awidat is structurally unusual: a local-first AI editorial NLE where
// the agent has READ your media and AGENTS.md, proposes editorial
// work, and the human reviews/accepts/rejects. New users don't have a
// mental model for this. The welcome explains the three core ideas in
// one screen — read once, dismiss, done.
//
// Storage lives in `useWelcome`. Composition follows the modal pattern
// used by SettingsModal / AgentsMdEditor: `.modal-backdrop` ▸ `.modal`
// ▸ header/body/footer. Tokens-only styling; no new CSS file.

import { BookOpen, Sparkles, GitBranch } from "lucide-react";
import { useEffect, type ReactNode } from "react";
import mark from "../brand/awidat-mark.svg";
import { useWelcome } from "../state/welcome";
import { Button, Inline, Stack } from "../ui";

const CORE_IDEAS: ReadonlyArray<{ icon: ReactNode; title: string; body: string }> = [
  {
    icon: <BookOpen size={16} />,
    title: "An agent reads your media.",
    body: "Drop a file, and Montage indexes it — transcript, scenes, speakers, silences. Your editorial brief lives in AGENTS.md.",
  },
  {
    icon: <Sparkles size={16} />,
    title: "It proposes editorial cuts with rationale.",
    body: "The agent suggests trims, B-roll, color, captions — each with a one-sentence reason. You see them in the Brief and on the timeline as ghost overlays.",
  },
  {
    icon: <GitBranch size={16} />,
    title: "You accept, reject, or revise inline.",
    body: "Every decision becomes part of the History. Nothing happens without your call.",
  },
];

export function WelcomeCard() {
  const isOpen = useWelcome((s) => s.isOpen);
  const dismiss = useWelcome((s) => s.dismiss);

  // ⌘W / Esc dismisses. Registered at the document level so they win
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
        className="modal"
        onClick={(event) => event.stopPropagation()}
        style={{
          width: "min(640px, calc(100vw - 48px))",
          maxHeight: "min(520px, calc(100vh - 48px))",
        }}
        role="dialog"
        aria-modal="true"
        aria-label="Welcome to Montage"
      >
        <header className="modal-header">
          <Inline gap="2" align="center">
            <img src={mark} alt="" width={20} height={20} />
            <h2>Welcome to Montage</h2>
          </Inline>
          <button type="button" className="modal-close" onClick={dismiss} aria-label="Dismiss welcome">
            ×
          </button>
        </header>
        <div className="modal-body">
          <Stack gap="2">
            {CORE_IDEAS.map((idea, index) => (
              <IdeaCard key={index} icon={idea.icon} title={idea.title} body={idea.body} />
            ))}
          </Stack>
        </div>
        <footer className="modal-footer">
          <span className="font-mono text-[var(--text-caption)] text-[var(--color-text-muted)]" title="Dismiss">
            ⌘W or Esc
          </span>
          <Inline gap="2" align="center">
            <Button variant="primary" size="sm" onClick={dismiss}>
              Get started
            </Button>
          </Inline>
        </footer>
      </div>
    </div>
  );
}

/** One of the three core-idea cards. Orange icon chip on the left,
 *  two-line copy on the right. Built from tokens so visual weight
 *  tracks the rest of the app. */
function IdeaCard({ icon, title, body }: { icon: ReactNode; title: string; body: string }) {
  return (
    <div className="flex items-start gap-3 p-3 rounded-[var(--radius-sm)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-input)]">
      <div
        className="grid place-items-center w-7 h-7 shrink-0 rounded-[var(--radius-xs)] text-[var(--color-brand)] bg-[color-mix(in_srgb,var(--color-brand)_14%,transparent)] border border-[color-mix(in_srgb,var(--color-brand)_28%,transparent)]"
        aria-hidden
      >
        {icon}
      </div>
      <Stack gap="0" className="min-w-0 flex-1">
        <span className="text-[var(--text-body-sm)] font-semibold text-[var(--color-text-primary)]">
          {title}
        </span>
        <span className="text-[var(--text-body-sm)] text-[var(--color-text-secondary)]">
          {body}
        </span>
      </Stack>
    </div>
  );
}
