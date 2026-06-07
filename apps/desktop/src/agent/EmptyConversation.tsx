// Empty state for the agent rail's conversation pane. Renders a brief
// "Montage" intro card followed by four project-aware starter prompts.
// Mounted by ChatStream when `items.length === 0`; the Composer and
// SessionBar continue to render outside this component.

import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useAgentStore } from "./store";
import { useProjectStore } from "../app/state";
import { getStarterPrompts } from "./starterPrompts";
import type { ProjectType } from "../protocol";

export function EmptyConversation() {
  const current = useProjectStore((s) => s.current);
  const running = useAgentStore((s) => s.running);
  const setRunning = useAgentStore((s) => s.setRunning);
  const setTurnError = useAgentStore((s) => s.setTurnError);
  const [projectType, setProjectType] = useState<ProjectType | null>(null);

  // Match ProjectBanner's pattern: pull the project type from the
  // backend whenever the active project changes. `Other` is treated as
  // "no specific type" — fall through to the generic starter prompts.
  useEffect(() => {
    if (!current) {
      setProjectType(null);
      return;
    }
    invoke<ProjectType>("get_project_type")
      .then((pt) => setProjectType(pt))
      .catch(() => setProjectType(null));
  }, [current]);

  const typeKey = projectTypeKey(projectType);
  const prompts = getStarterPrompts(typeKey);

  // TODO: wire real indexing readiness from App-level state. The
  // IndexReadinessSnapshot currently lives as local state in App.tsx;
  // until it's lifted into a store the agent rail can subscribe to we
  // fall back to a project-agnostic opener.
  const allReady = false;
  const opener = allReady
    ? "I've indexed your clip — speech, scenes, color, audio all ready. Tell me how you want this cut, or pick a starting move below."
    : current
      ? "Reading your media in the background. You can still send a message; I'll work on it once indexing finishes."
      : "Open or create a project, then tell me how you want it cut — or pick a starting move below.";

  async function sendPrompt(prompt: string) {
    if (running || !current) return;
    setTurnError(null);
    setRunning(true);
    try {
      await invoke("start_turn", { input: prompt });
    } catch (err) {
      setTurnError(String(err));
      setRunning(false);
    }
  }

  return (
    <div className="flex flex-col h-full">
      <div
        className="m-3 rounded-lg border border-[var(--color-border-subtle)] p-3"
        style={{
          background:
            "linear-gradient(180deg, rgba(239,68,68,0.05), transparent), var(--color-surface-panel)",
        }}
      >
        <div className="mb-1 text-[12px] font-semibold text-[var(--color-text-primary)]">
          <span className="text-[var(--color-brand)]">◆</span> Montage
          <span className="ml-2 font-mono text-[10px] font-normal text-[var(--color-text-muted)]">
            {typeKey ?? "editor"} mode
          </span>
        </div>
        <p className="m-0 text-[11px] leading-snug text-[var(--color-text-secondary)]">
          {opener}
        </p>
      </div>

      <div className="flex flex-col gap-1.5 px-3">
        <div className="text-[9px] font-bold uppercase tracking-[0.08em] text-[var(--color-text-muted)]">
          Try
        </div>
        {prompts.map((p) => (
          <button
            key={p}
            type="button"
            onClick={() => sendPrompt(p)}
            disabled={running || !current}
            className="flex items-center gap-2 rounded-md border border-[var(--color-border-subtle)] bg-[var(--color-surface-panel)] px-2.5 py-1.5 text-left text-[11px] text-[var(--color-text-secondary)] hover:border-[var(--color-border)] hover:bg-[var(--color-surface-card-hover)] hover:text-[var(--color-text-primary)] disabled:cursor-not-allowed disabled:opacity-50"
          >
            <span className="text-[var(--color-brand)]">▸</span>
            <span>{p}</span>
          </button>
        ))}
      </div>
    </div>
  );
}

/** Map the discriminated `ProjectType` union to a flat string key for
 * the starter-prompt lookup. Returns undefined for `other` or null so
 * the lookup falls through to the fallback set. */
function projectTypeKey(pt: ProjectType | null): string | undefined {
  if (!pt) return undefined;
  switch (pt.kind) {
    case "podcast":
      return "podcast";
    // The PROMPTS_BY_TYPE table also defines "interview" and
    // "highlight". The backend doesn't currently emit those discriminants
    // (`shorts`, `tutorial`, `other` are the other options), so they
    // intentionally fall through to FALLBACK until the protocol catches up.
    default:
      return undefined;
  }
}
