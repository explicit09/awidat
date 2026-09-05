// Upload-prefs store — user's opt-in for "publish after render".
//
// The set of provider keys here gets attached as `uploadTargets` on a
// fresh render queue entry.
//
// Persistence:
//   - localStorage is the source of truth across projects and reloads.

import { create } from "zustand";
import type { DeliveryTargetKey } from "../shell/delivery/types";

/** Provider keys currently exposed as publishing targets. Keep in sync
 *  with `isUploadCapableTarget` in `targetMeta.ts`. */
const SUPPORTED: ReadonlySet<DeliveryTargetKey> = new Set([
  "youtube",
  "twitter_x",
]);

const STORAGE_KEY = "montage.deliver.uploadPrefs.v1";

interface UploadPrefsState {
  /** Provider keys the user has opted into auto-publishing for. */
  enabled: ReadonlySet<DeliveryTargetKey>;
  /** Toggle one provider on or off and persist locally. */
  toggle: (key: DeliveryTargetKey) => void;
  /** Replace the full set. */
  setEnabled: (keys: DeliveryTargetKey[]) => void;
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

export const useUploadPrefs = create<UploadPrefsState>((set, get) => ({
  enabled: new Set<DeliveryTargetKey>(loadLocal()),
  toggle: (key) => {
    if (!SUPPORTED.has(key)) return;
    const current = get().enabled;
    const next = new Set(current);
    if (next.has(key)) {
      next.delete(key);
    } else {
      next.add(key);
    }
    set({ enabled: next });
    persistLocal(next);
  },
  setEnabled: (keys) => {
    const filtered = keys.filter((k) => SUPPORTED.has(k));
    const next = new Set(filtered);
    set({ enabled: next });
    persistLocal(next);
  },
}));

/** Translate a selected DeliveryTargetKey into the social provider key.
 *  Identity for publisher targets — kept as
 *  a function so future renames (e.g. "youtube_shorts") have one
 *  place to update. */
export function providerKeyForTarget(key: DeliveryTargetKey): string | null {
  return SUPPORTED.has(key) ? key : null;
}
