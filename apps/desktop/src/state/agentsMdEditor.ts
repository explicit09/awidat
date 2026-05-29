/**
 * useAgentsMdEditor — open-state + dirty tracking for the AGENTS.md
 * in-product editor modal (Wave 3 T2).
 *
 * Why a separate store: the modal can be opened from multiple
 * surfaces (Settings now; the Brief surface later) and dirty state
 * must survive a re-render of either. The Settings modal owns its
 * own open flag in `state/settings.ts` — this mirrors that shape so
 * call sites read the same way.
 *
 * Persistence is intentionally NOT wired — an unsaved draft must
 * never survive an app restart (we'd be silently desyncing the on-
 * disk file from the agent's view). On `close()` the dirty flag is
 * cleared so a re-open starts fresh.
 */

import { create } from "zustand";

interface AgentsMdEditorStore {
  isOpen: boolean;
  /** True after `markDirty` and until the next `open`/`close`/`markClean`. */
  isDirty: boolean;
  open: () => void;
  close: () => void;
  /** Caller signals an unsaved edit — the Save button enables, the
   *  Esc handler shows a confirm before discarding. */
  markDirty: () => void;
  /** Caller signals the buffer matches what's on disk (post-save, or
   *  user reverted edits manually). */
  markClean: () => void;
}

/** Framework-agnostic factory so the store can be exercised under
 *  plain node (see tests/agents-md-editor.test.ts). */
export function createAgentsMdEditorStore() {
  return create<AgentsMdEditorStore>((set) => ({
    isOpen: false,
    isDirty: false,
    open: () => set({ isOpen: true, isDirty: false }),
    close: () => set({ isOpen: false, isDirty: false }),
    markDirty: () =>
      set((state) => (state.isDirty ? state : { ...state, isDirty: true })),
    markClean: () =>
      set((state) => (state.isDirty ? { ...state, isDirty: false } : state)),
  }));
}

export const useAgentsMdEditor = createAgentsMdEditorStore();
