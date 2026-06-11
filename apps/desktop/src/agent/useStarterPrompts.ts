// Project-aware starter prompts + the "fire one as an agent turn"
// action, shared between the agent rail's empty conversation and the
// preview review queue's empty state so both stay in sync.

import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useAgentStore } from "./store";
import { isAuthReadyForAgent } from "./composerAuthGate";
import { useProjectStore } from "../app/state";
import { getStarterPrompts } from "./starterPrompts";
import type { ProjectType } from "../protocol";
import { useAuth } from "../state/auth";

export type StarterPrompts = {
  /** Prompt strings for the current project type. */
  prompts: string[];
  /** Flat project-type key ("podcast", …) or undefined for generic. */
  typeKey: string | undefined;
  /** True while an agent turn is running (disable the buttons). */
  running: boolean;
  /** True when a project is loaded (buttons are no-ops otherwise). */
  hasProject: boolean;
  /** Send a prompt as a new agent turn (opens auth if needed). */
  send: (prompt: string) => Promise<void>;
};

export function useStarterPrompts(): StarterPrompts {
  const current = useProjectStore((s) => s.current);
  const running = useAgentStore((s) => s.running);
  const setRunning = useAgentStore((s) => s.setRunning);
  const setTurnError = useAgentStore((s) => s.setTurnError);
  const authStatus = useAuth((s) => s.status);
  const openAuth = useAuth((s) => s.open);
  const [projectType, setProjectType] = useState<ProjectType | null>(null);
  const authReady = isAuthReadyForAgent(authStatus);

  // Pull the project type from the backend whenever the active
  // project changes; `other`/unknown falls through to generic prompts.
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

  async function send(prompt: string) {
    if (running || !current) return;
    if (!authReady) {
      openAuth();
      return;
    }
    setTurnError(null);
    setRunning(true);
    try {
      await invoke("start_turn", { input: prompt });
    } catch (err) {
      setTurnError(String(err));
      setRunning(false);
    }
  }

  return {
    prompts: getStarterPrompts(typeKey),
    typeKey,
    running,
    hasProject: Boolean(current),
    send,
  };
}

/** Map the discriminated `ProjectType` union to a flat string key for
 * the starter-prompt lookup. Returns undefined for `other` or null so
 * the lookup falls through to the fallback set. */
export function projectTypeKey(pt: ProjectType | null): string | undefined {
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
