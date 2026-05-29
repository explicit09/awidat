/**
 * useWelcome — first-run welcome card open-state (Wave 3 W1).
 *
 * Awidat's agent-driven editorial loop is unusual: a new user has no
 * mental model for "the agent already read your media and is proposing
 * edits". A one-screen welcome on first launch ever explains the three
 * core ideas — read once, dismiss, done.
 *
 * Storage: a single localStorage key (`awidat:welcome:shown`). If
 * absent, the modal fires on mount; on dismiss, a timestamp is written
 * so subsequent launches stay clean. Settings exposes a "Show welcome
 * again" affordance that wipes the key via `reset()`.
 *
 * Owning the storage read/write directly (instead of Zustand `persist`)
 * keeps the test contract small and matches `introState.ts`.
 */

import { create } from "zustand";

export const STORAGE_KEY = "awidat:welcome:shown";

interface StorageAdapter {
  getItem(k: string): string | null;
  setItem(k: string, v: string): void;
  removeItem(k: string): void;
}

interface WelcomeStore {
  /** True while the modal is mounted/visible. */
  isOpen: boolean;
  /** True once the user has dismissed the welcome on this machine. */
  shown: boolean;
  open: () => void;
  dismiss: () => void;
  markShown: () => void;
  /** Wipe the persisted flag and reopen — Settings "Show welcome again". */
  reset: () => void;
}

interface CreateOpts {
  persist?: boolean;
  storage?: StorageAdapter;
}

function loadShown(storage: StorageAdapter | null): boolean {
  if (!storage) return false;
  const raw = storage.getItem(STORAGE_KEY);
  return typeof raw === "string" && raw.length > 0;
}

function persistShown(storage: StorageAdapter | null): void {
  if (!storage) return;
  try {
    storage.setItem(STORAGE_KEY, new Date().toISOString());
  } catch {
    // Quota / serialization failures are non-fatal — worst case the
    // welcome re-fires on the next launch.
  }
}

function clearShown(storage: StorageAdapter | null): void {
  if (!storage) return;
  try { storage.removeItem(STORAGE_KEY); } catch { /* see persistShown */ }
}

/** Framework-agnostic factory so the store can be exercised under
 *  plain node (see tests/welcome.test.ts). */
export function createWelcomeStore(opts: CreateOpts = {}) {
  const persist = opts.persist ?? true;
  const storage: StorageAdapter | null = persist
    ? opts.storage ?? (typeof localStorage !== "undefined" ? localStorage : null)
    : null;
  const initialShown = loadShown(storage);

  return create<WelcomeStore>((set, get) => ({
    isOpen: !initialShown,
    shown: initialShown,
    open: () =>
      set((state) => (state.isOpen ? state : { ...state, isOpen: true })),
    dismiss: () => {
      if (!get().shown) persistShown(storage);
      set({ isOpen: false, shown: true });
    },
    markShown: () => {
      if (get().shown) return;
      persistShown(storage);
      set({ shown: true });
    },
    reset: () => {
      clearShown(storage);
      set({ shown: false, isOpen: true });
    },
  }));
}

export const useWelcome = createWelcomeStore();
