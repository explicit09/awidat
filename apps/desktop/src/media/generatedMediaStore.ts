import { invoke } from "@tauri-apps/api/core";
import { create } from "zustand";
import type { BrollEntry } from "../state/briefProposals";

export type GeneratedMediaState =
  | "draft"
  | "queued"
  | "running"
  | "succeeded"
  | "failed"
  | "cancelled";

export type GeneratedMediaEntry = {
  job_id: string;
  artifact_kind: string;
  workflow_purpose: string;
  provider: string;
  model: string;
  state: GeneratedMediaState | string;
  prompt_excerpt: string;
  prompt_hash: string;
  video_path?: string | null;
  absolute_video_path?: string | null;
  uses_likeness: boolean;
  requires_disclosure: boolean;
  cost_estimate_usd?: number | null;
  cost_actual_usd?: number | null;
  failure_message?: string | null;
};

/**
 * A generated-media entry is ready for the Brief once the provider has
 * landed the asset (`succeeded`) and we have a video to insert. Other
 * states (`draft`, `queued`, `running`) still belong in the Media-tab
 * history but the user has nothing to decide on yet.
 */
export function isBrollProposalCandidate(entry: GeneratedMediaEntry): boolean {
  if (entry.workflow_purpose !== "broll") return false;
  if (entry.state !== "succeeded") return false;
  return Boolean(entry.absolute_video_path ?? entry.video_path);
}

/**
 * Project a generated-broll registry entry to a Brief proposal row.
 * The Brief renders the prompt as the rationale; provider + model
 * power the muted "AI-generated · provider X · model Y" disclosure.
 */
export function brollEntryFromGenerated(entry: GeneratedMediaEntry): BrollEntry {
  const videoPath = entry.absolute_video_path ?? entry.video_path ?? undefined;
  return {
    id: entry.job_id,
    title: `Generated B-roll · ${entry.provider}`,
    rationale: entry.prompt_excerpt,
    firstSeenAt: Date.now(),
    metadata: {
      prompt: entry.prompt_excerpt,
      provider: entry.provider,
      model: entry.model,
      videoPath: videoPath ?? undefined,
    },
  };
}

type GeneratedMediaStateShape = {
  entries: GeneratedMediaEntry[];
  loading: boolean;
  error: string | null;
  refresh: () => Promise<void>;
  clear: () => void;
};

export const useGeneratedMediaStore = create<GeneratedMediaStateShape>((set) => ({
  entries: [],
  loading: false,
  error: null,
  refresh: async () => {
    set({ loading: true, error: null });
    try {
      const entries = await invoke<GeneratedMediaEntry[]>("list_generated_media");
      set({ entries, loading: false, error: null });
    } catch (e) {
      set({ loading: false, error: String(e) });
    }
  },
  clear: () => set({ entries: [], loading: false, error: null }),
}));
