import { invoke } from "@tauri-apps/api/core";
import { create } from "zustand";

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
