import { create } from "zustand";

export type Mode = "pro" | "creator";
export const STORAGE_KEY = "montage:mode";

const isValid = (v: unknown): v is Mode => v === "pro" || v === "creator";

interface StorageAdapter {
  getItem(k: string): string | null;
  setItem(k: string, v: string): void;
  removeItem(k: string): void;
}

interface ModeStore {
  get: () => Mode;
  set: (next: Mode) => void;
  toggle: () => void;
  subscribe: (cb: (mode: Mode) => void) => () => void;
}

interface CreateOpts {
  persist?: boolean;
  storage?: StorageAdapter;
  defaultMode?: Mode;
}

/**
 * Framework-agnostic mode store. The Zustand React hook wraps this so the
 * core logic is testable under plain node.
 */
export function createModeStore(opts: CreateOpts = {}): ModeStore {
  const persist = opts.persist ?? true;
  const storage: StorageAdapter | null = persist
    ? opts.storage ?? (typeof localStorage !== "undefined" ? localStorage : null)
    : null;
  const defaultMode: Mode = opts.defaultMode ?? "pro";

  let current: Mode = defaultMode;
  if (storage) {
    const raw = storage.getItem(STORAGE_KEY);
    if (raw && isValid(raw)) current = raw;
  }

  const subscribers = new Set<(m: Mode) => void>();

  return {
    get: () => current,
    set: (next) => {
      if (!isValid(next) || next === current) return;
      current = next;
      storage?.setItem(STORAGE_KEY, next);
      subscribers.forEach((cb) => cb(next));
    },
    toggle: function () {
      this.set(current === "pro" ? "creator" : "pro");
    },
    subscribe: (cb) => {
      subscribers.add(cb);
      return () => { subscribers.delete(cb); };
    },
  };
}

/** React-facing Zustand hook. */
interface UseModeState { mode: Mode; setMode: (m: Mode) => void; toggle: () => void; }
const _store = createModeStore();
export const useMode = create<UseModeState>((set) => ({
  mode: _store.get(),
  setMode: (m) => { _store.set(m); set({ mode: _store.get() }); },
  toggle:   () => { _store.toggle();  set({ mode: _store.get() }); },
}));
