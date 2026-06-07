// Selected delivery targets store + target catalog.
//
// The DeliverySurface renders 6 platform cards (YouTube / TikTok /
// Instagram / Twitter/X / Captions / Cover / Custom frame). This store tracks
// which of those the user has *selected* for the next export, and
// persists the selection per-project so reloads keep the choices.
//
// Resolution per target — width / height / video bitrate — also
// lives here so the render queue worker can pass them straight to
// `start_reframe_render` without re-deriving from a card label.

import { create } from "zustand";

export type DeliveryTargetKey =
  | "youtube"
  | "tiktok"
  | "instagram"
  | "twitter_x"
  | "captions"
  | "cover"
  | "custom";

/** Static catalog of what each target represents on disk. */
export type DeliveryTargetSpec = {
  key: DeliveryTargetKey;
  label: string;
  /** "1080p · 16:9 · h264" — shown under the label. */
  spec: string;
  /** What artifact kind this target produces. */
  kind: "video_master" | "video_reframe" | "captions" | "still";
  /** For video targets: resolution + bitrate the reframe step uses. */
  width?: number;
  height?: number;
  videoBitrateKbps?: number;
  /** For still targets: which still kind. */
  stillKind?: "cover" | "custom";
};

export const DELIVERY_TARGETS: Record<DeliveryTargetKey, DeliveryTargetSpec> = {
  // YouTube is the master render — 16:9 at full bitrate. Every other
  // video target reframes from this one.
  youtube: {
    key: "youtube",
    label: "YouTube",
    spec: "1080p · 16:9 · h264",
    kind: "video_master",
    width: 1920,
    height: 1080,
    videoBitrateKbps: 12_000,
  },
  tiktok: {
    key: "tiktok",
    label: "TikTok",
    spec: "1080p · 9:16 · h264",
    kind: "video_reframe",
    width: 1080,
    height: 1920,
    videoBitrateKbps: 9_000,
  },
  instagram: {
    key: "instagram",
    label: "Instagram",
    spec: "1080p · 1:1 · h264",
    kind: "video_reframe",
    width: 1080,
    height: 1080,
    videoBitrateKbps: 8_000,
  },
  twitter_x: {
    key: "twitter_x",
    label: "Twitter/X",
    spec: "1080p · 9:16 · h264",
    kind: "video_reframe",
    width: 1080,
    height: 1920,
    videoBitrateKbps: 9_000,
  },
  captions: {
    key: "captions",
    label: "Captions",
    spec: "SRT + VTT",
    kind: "captions",
  },
  cover: {
    key: "cover",
    label: "Cover",
    spec: "1280×720 PNG",
    kind: "still",
    stillKind: "cover",
  },
  custom: {
    key: "custom",
    label: "Custom frame",
    spec: "User-selected",
    kind: "still",
    stillKind: "custom",
  },
};

export function renderQueueLabelForTarget(
  key: DeliveryTargetKey,
  selected: ReadonlySet<DeliveryTargetKey>,
): string {
  if (key === "youtube" && !selected.has("youtube")) {
    return "Source master render";
  }
  return DELIVERY_TARGETS[key].label;
}

type State = {
  /** Selected target keys. Use a Set to keep order stable for the
   *  render queue without callers having to dedupe. */
  selected: Set<DeliveryTargetKey>;
  toggle: (key: DeliveryTargetKey) => void;
  setSelected: (keys: DeliveryTargetKey[]) => void;
  selectAll: () => void;
  clear: () => void;
};

const PERSIST_KEY = "montage.deliver.targets.v1";

function loadPersisted(): Set<DeliveryTargetKey> {
  if (typeof localStorage === "undefined") return new Set();
  try {
    const raw = localStorage.getItem(PERSIST_KEY);
    if (!raw) return new Set();
    const parsed = JSON.parse(raw) as DeliveryTargetKey[];
    if (!Array.isArray(parsed)) return new Set();
    const valid = parsed.filter(
      (k): k is DeliveryTargetKey => k in DELIVERY_TARGETS,
    );
    return new Set(valid);
  } catch {
    return new Set();
  }
}

function persist(selected: Set<DeliveryTargetKey>): void {
  if (typeof localStorage === "undefined") return;
  try {
    localStorage.setItem(PERSIST_KEY, JSON.stringify(Array.from(selected)));
  } catch {
    // ignore
  }
}

export const useDeliveryTargetsStore = create<State>((set) => ({
  selected: loadPersisted(),
  toggle: (key) =>
    set((state) => {
      const next = new Set(state.selected);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      persist(next);
      return { selected: next };
    }),
  setSelected: (keys) =>
    set(() => {
      const next = new Set(keys);
      persist(next);
      return { selected: next };
    }),
  selectAll: () =>
    set(() => {
      const next = new Set<DeliveryTargetKey>(
        Object.keys(DELIVERY_TARGETS) as DeliveryTargetKey[],
      );
      persist(next);
      return { selected: next };
    }),
  clear: () =>
    set(() => {
      const next = new Set<DeliveryTargetKey>();
      persist(next);
      return { selected: next };
    }),
}));
