// AI-disclosure inspection store. `byJobId` holds the per-render
// disclosure computed from the local timeline and generated-media
// registry.

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

interface AiDisclosureState {
  /** Per-render disclosure, keyed by render-job id (or `pending:<…>`
   *  before a render kicks). */
  byJobId: Record<string, AiDisclosure>;
  /** Replace one job's disclosure after backend inspection. */
  set: (jobId: string, disclosure: AiDisclosure) => void;
  /** Drop a job's disclosure — terminal cleanup. */
  forget: (jobId: string) => void;
}

export const useAiDisclosure = create<AiDisclosureState>((set) => ({
  byJobId: {},
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
