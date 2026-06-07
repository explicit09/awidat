// Upload-prefs store — user's opt-in for "publish after render".
//
// Mirrors the backend `UploadPrefs` (apps/desktop/src-tauri/src/
// publishing/upload_queue.rs). The set of provider keys here is what
// gets attached as `uploadTargets` on a fresh render queue entry.
//
// Persistence:
//   - Source of truth is the backend (`get_default_upload_targets`),
//     so the same prefs apply across projects.
//   - We mirror to localStorage for instant hydration on app boot so
//     the Deliver UI doesn't flash an "off" state on every reload.
//   - Writes go to both the backend (via Tauri command) and the
//     in-memory + localStorage mirror.

import { create } from "zustand";
import type { DeliveryTargetKey } from "../shell/delivery/types";

/** Provider keys that map to a publishing target. Mirrors the backend
 *  provider registry. Keep in sync with `UPLOAD_CAPABLE_TARGETS` in
 *  `TargetsList.tsx`. */
const SUPPORTED: ReadonlySet<DeliveryTargetKey> = new Set([
  "youtube",
  "tiktok",
  "instagram",
  "twitter_x",
]);

const STORAGE_KEY = "montage.deliver.uploadPrefs.v1";

interface UploadPrefsState {
  /** Provider keys the user has opted into auto-publishing for. */
  enabled: ReadonlySet<DeliveryTargetKey>;
  /** Monotonic local edit counter used to reject stale deferred hydrates. */
  revision: number;
  /** Toggle one provider on or off. Persists both backend + local. */
  toggle: (key: DeliveryTargetKey) => Promise<void>;
  /** Replace the full set (used on hydrate). */
  setEnabled: (keys: DeliveryTargetKey[]) => void;
  /** Pull the latest from the backend. Call once on app mount. */
  hydrate: () => Promise<void>;
}

export function shouldApplyBackendUploadPrefs(
  scheduledRevision: number,
  currentRevision: number,
): boolean {
  return scheduledRevision === currentRevision;
}

function loadLocal(): DeliveryTargetKey[] {
  if (typeof localStorage === "undefined") return [];
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return [];
    const parsed = JSON.parse(raw) as unknown;
    if (!Array.isArray(parsed)) return [];
    return parsed.filter(
      (k): k is DeliveryTargetKey =>
        typeof k === "string" && SUPPORTED.has(k as DeliveryTargetKey),
    );
  } catch {
    return [];
  }
}

function persistLocal(keys: ReadonlySet<DeliveryTargetKey>): void {
  if (typeof localStorage === "undefined") return;
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify([...keys]));
  } catch {
    // localStorage may be disabled; ignore.
  }
}

/** Push the current set to the backend. Best-effort — failure to
 *  reach Tauri (e.g. running outside the desktop shell) is non-fatal;
 *  the local mirror still works. */
async function persistBackend(
  keys: ReadonlySet<DeliveryTargetKey>,
): Promise<void> {
  try {
    const { invoke } = await import("@tauri-apps/api/core");
    await invoke<void>("set_default_upload_targets", {
      providers: [...keys],
    });
  } catch {
    // Backend unavailable; localStorage mirror is the fallback.
  }
}

export const useUploadPrefs = create<UploadPrefsState>((set, get) => ({
  enabled: new Set<DeliveryTargetKey>(loadLocal()),
  revision: 0,
  toggle: async (key) => {
    if (!SUPPORTED.has(key)) return;
    const current = get().enabled;
    const next = new Set(current);
    if (next.has(key)) {
      next.delete(key);
    } else {
      next.add(key);
    }
    set((state) => ({ enabled: next, revision: state.revision + 1 }));
    persistLocal(next);
    await persistBackend(next);
  },
  setEnabled: (keys) => {
    const filtered = keys.filter((k) => SUPPORTED.has(k));
    const next = new Set(filtered);
    set((state) => ({ enabled: next, revision: state.revision + 1 }));
    persistLocal(next);
  },
  hydrate: async () => {
    const scheduledRevision = get().revision;
    try {
      const { invoke } = await import("@tauri-apps/api/core");
      const prefs = await invoke<{ default_targets: string[] }>(
        "get_default_upload_targets",
      );
      const keys = prefs.default_targets.filter(
        (k): k is DeliveryTargetKey => SUPPORTED.has(k as DeliveryTargetKey),
      );
      if (!shouldApplyBackendUploadPrefs(scheduledRevision, get().revision)) {
        return;
      }
      const next = new Set(keys);
      set({ enabled: next });
      persistLocal(next);
    } catch {
      // Already hydrated from localStorage; leave it.
    }
  },
}));

/** Translate a selected DeliveryTargetKey into the backend's
 *  provider key. Identity for publisher targets — kept as
 *  a function so future renames (e.g. "youtube_shorts") have one
 *  place to update. */
export function providerKeyForTarget(key: DeliveryTargetKey): string | null {
  return SUPPORTED.has(key) ? key : null;
}
