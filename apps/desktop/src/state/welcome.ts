/**
 * useWelcome — first-run consent gate.
 *
 * Montage's agent-driven editorial loop is unusual: a new user has no
 * mental model for "the agent already read your media and is proposing
 * edits". A one-screen welcome on first launch explains the core ideas
 * and requires explicit local/remote data-flow consent.
 *
 * Storage: a single localStorage key (`montage:welcome:consent`). If
 * absent, the modal fires on mount; on consent, a timestamp is written
 * so subsequent launches stay clean. Settings exposes a "Show welcome
 * again" affordance that wipes the key via `reset()`.
 *
 * Owning the storage read/write directly (instead of Zustand `persist`)
 * keeps the test contract small and matches `introState.ts`.
 */

import { create } from "zustand";

export const STORAGE_KEY = "montage:welcome:consent";

interface StorageAdapter {
  getItem(k: string): string | null;
  setItem(k: string, v: string): void;
  removeItem(k: string): void;
}

interface WelcomeStore {
  /** True while the modal is mounted/visible. */
  isOpen: boolean;
  /** True once the user has accepted first-run consent on this machine. */
  shown: boolean;
  /** ISO timestamp for accepted consent, if present. */
  consentedAt: string | null;
  open: () => void;
  consent: () => void;
  dismiss: () => void;
  markShown: () => void;
  /** Wipe the persisted flag and reopen — Settings "Show welcome again". */
  reset: () => void;
}

interface CreateOpts {
  persist?: boolean;
  storage?: StorageAdapter;
}

function loadConsent(storage: StorageAdapter | null): string | null {
  if (!storage) return null;
  const raw = storage.getItem(STORAGE_KEY);
  return typeof raw === "string" && raw.length > 0 ? raw : null;
}

function persistConsent(storage: StorageAdapter | null): string {
  const timestamp = new Date().toISOString();
  if (!storage) return timestamp;
  try {
    storage.setItem(STORAGE_KEY, timestamp);
  } catch {
    // Quota / serialization failures are non-fatal — worst case the
    // consent card re-fires on the next launch.
  }
  return timestamp;
}

function clearConsent(storage: StorageAdapter | null): void {
  if (!storage) return;
  try { storage.removeItem(STORAGE_KEY); } catch { /* see persistConsent */ }
}

/** Framework-agnostic factory so the store can be exercised under
 *  plain node (see tests/welcome.test.ts). */
export function createWelcomeStore(opts: CreateOpts = {}) {
  const persist = opts.persist ?? true;
  const storage: StorageAdapter | null = persist
    ? opts.storage ?? (typeof localStorage !== "undefined" ? localStorage : null)
    : null;
  // Dev-only: VITE_MONTAGE_SKIP_WELCOME=1 suppresses the first-run card
  // (used for native screenshot tours). No effect in production builds.
  const devSkip =
    typeof import.meta !== "undefined" &&
    import.meta.env?.VITE_MONTAGE_SKIP_WELCOME === "1";
  const initialConsent = loadConsent(storage);
  const initialShown = devSkip || initialConsent !== null;

  return create<WelcomeStore>((set, get) => ({
    isOpen: !initialShown,
    shown: initialShown,
    consentedAt: initialConsent,
    open: () =>
      set((state) => (state.isOpen ? state : { ...state, isOpen: true })),
    consent: () => {
      const consentedAt = get().consentedAt ?? persistConsent(storage);
      set({ isOpen: false, shown: true, consentedAt });
    },
    dismiss: () => get().consent(),
    markShown: () => {
      if (get().shown) return;
      const consentedAt = persistConsent(storage);
      set({ shown: true, consentedAt });
    },
    reset: () => {
      clearConsent(storage);
      set({ shown: false, consentedAt: null, isOpen: true });
    },
  }));
}

export const useWelcome = createWelcomeStore();
