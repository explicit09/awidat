// AI-disclosure compliance store (W5.A4).
//
// Two pieces of state live here:
//
//  1. `byJobId` — the per-render `AiDisclosure` the backend computed
//     by walking the timeline against the generated-media registry.
//     Populated via `computeAiDisclosure(jobId)` (Tauri command) and
//     re-hydrated from the backend on app boot via `refreshFromBackend`.
//
//  2. `autoDiscloseEnabled` — the "Auto-disclose AI content" toggle
//     (default ON, per FTC 2024 / EU AI Act safety stance). Mirrored
//     to localStorage so a reload preserves the choice.
//
// The toggle is intentionally a *power-user override*: when OFF, the
// upload calls don't set the platform flag and the disclosure banner
// shows a warning that the user must self-flag on the platform. The
// default-ON stance is the load-bearing safety default — if a user
// hasn't deliberately opted out, we always disclose.

import { create } from "zustand";

/** One credit pulled out of the cut for the disclosure banner. */
export type GeneratedMediaCredit = {
  provider: string;
  model?: string;
  prompt: string;
  /** Unix epoch seconds. */
  generated_at?: number;
  asset_id: string;
};

/** Full disclosure payload. Mirrors the Rust shape verbatim. */
export type AiDisclosure = {
  has_synthetic_content: boolean;
  credits: GeneratedMediaCredit[];
};

/** Empty disclosure — used as the safe default for clean cuts. */
export function emptyAiDisclosure(): AiDisclosure {
  return { has_synthetic_content: false, credits: [] };
}

/** localStorage key for the auto-disclose toggle. */
const AUTO_DISCLOSE_KEY = "awidat.publishing.autoDiscloseAi.v1";

/** Read the persisted toggle. Default ON — see file header. */
export function loadAutoDiscloseEnabled(): boolean {
  if (typeof localStorage === "undefined") return true;
  try {
    const raw = localStorage.getItem(AUTO_DISCLOSE_KEY);
    if (raw === null) return true;
    // Only the literal string "false" disables — anything else
    // (including malformed values) stays ON, defensive.
    return raw !== "false";
  } catch {
    return true;
  }
}

function persistAutoDisclose(enabled: boolean): void {
  if (typeof localStorage === "undefined") return;
  try {
    localStorage.setItem(AUTO_DISCLOSE_KEY, enabled ? "true" : "false");
  } catch {
    // localStorage may be disabled — defaults remain ON.
  }
}

interface AiDisclosureState {
  /** Per-render disclosure, keyed by render-job id (or `pending:<…>`
   *  before a render kicks). */
  byJobId: Record<string, AiDisclosure>;
  /** The "Auto-disclose AI content" toggle. */
  autoDiscloseEnabled: boolean;
  /** Replace one job's disclosure. Called after a backend
   *  `compute_ai_disclosure` round trip or via the worker on register. */
  set: (jobId: string, disclosure: AiDisclosure) => void;
  /** Drop a job's disclosure — terminal cleanup. */
  forget: (jobId: string) => void;
  /** Toggle the auto-disclose preference. Persists to localStorage. */
  setAutoDiscloseEnabled: (enabled: boolean) => void;
}

export const useAiDisclosure = create<AiDisclosureState>((set) => ({
  byJobId: {},
  autoDiscloseEnabled: loadAutoDiscloseEnabled(),
  set: (jobId, disclosure) =>
    set((state) => ({
      byJobId: { ...state.byJobId, [jobId]: disclosure },
    })),
  forget: (jobId) =>
    set((state) => {
      if (!(jobId in state.byJobId)) return state;
      const next = { ...state.byJobId };
      delete next[jobId];
      return { byJobId: next };
    }),
  setAutoDiscloseEnabled: (enabled) => {
    persistAutoDisclose(enabled);
    set({ autoDiscloseEnabled: enabled });
  },
}));

/** Truncate a credit's prompt for the banner display. Centralised so
 *  the banner + the tooltip use the same cap. */
export function truncatePrompt(prompt: string, max = 80): string {
  if (prompt.length <= max) return prompt;
  return `${prompt.slice(0, max - 1)}…`;
}

/** Build a one-line summary for a credit — used in the tooltip on the
 *  RenderQueue's AI chip. Format: `<provider>/<model> · <prompt prefix>`. */
export function summarizeCredit(credit: GeneratedMediaCredit): string {
  const providerModel = credit.model ? `${credit.provider}/${credit.model}` : credit.provider;
  const prompt = truncatePrompt(credit.prompt);
  return prompt.length > 0 ? `${providerModel} · ${prompt}` : providerModel;
}
