// Per-target upload metadata store.
//
// Keyed by `(jobId, provider)` → `UploadMetadata`. The server-backed
// publishing path reads this store directly when it creates a target.
//
// Why separate from `useRenderQueueStore`:
//   - The render queue tracks ephemeral lifecycle (running %, terminal
//     state) and re-saves on every transition. Metadata is bigger and
//     only mutates when the user edits a field — keeping them in
//     different stores avoids re-persisting form state on every
//     progress tick.
//   - Both sources are addressed by the same `(jobId, provider)` pair,
//     so the form can look up its values without coupling.
//
// Persistence:
//   - localStorage mirror keyed by `montage.deliver.uploadMetadata.v1`
//     so reloading the app preserves form state across reload.

import { create } from "zustand";

/** Visibility options that map 1:1 onto the backend's `Visibility`
 *  enum. Provider-specific labels (e.g. "Friends only" on TikTok) are
 *  handled at the form layer; this is the wire shape. */
export type UploadVisibility = "private" | "unlisted" | "public";

export type TikTokInteractionSettings = {
  disableDuet: boolean;
  disableComment: boolean;
  disableStitch: boolean;
};

/** Per-target upload metadata used to build social-server target fields. */
export type UploadMetadata = {
  /** Required — provider rejects empty titles. Validation pipeline
   *  flags this as `title.required` when the user clears the field. */
  title: string;
  /** Long-form description / caption. May be empty. */
  description: string;
  /** Tags / hashtags. The form's chip input splits by comma + trims
   *  whitespace; empty tags are dropped before the array hits the
   *  store. */
  tags: string[];
  /** Visibility on publish. Defaults to `"private"` for safety. */
  visibility: UploadVisibility;
  /** Optional schedule. Unix epoch seconds. `undefined` = publish
   *  immediately on render-done. */
  scheduledAt?: number;
  /** Absolute path to a cover image on disk. Picked via the Tauri
   *  `dialog::open` plugin; the form's preview uses `convertFileSrc`
   *  so the WKWebView can render it. */
  thumbnailPath?: string;
  tiktokInteractions?: TikTokInteractionSettings;
};

export type PublishTimingMode = "now" | "scheduled";

export function publishTimingMode(metadata: UploadMetadata): PublishTimingMode {
  return metadata.scheduledAt === undefined ? "now" : "scheduled";
}

export function applyPublishTiming(
  metadata: UploadMetadata,
  mode: PublishTimingMode,
  scheduledAt?: number,
): UploadMetadata {
  if (mode === "now") {
    return { ...metadata, scheduledAt: undefined };
  }
  return { ...metadata, scheduledAt };
}

/** Compose a stable storage key for one `(jobId, provider)` pair. */
function composeKey(jobId: string, provider: string): string {
  return `${jobId}::${provider}`;
}

/** Per-platform validation limits. Each field's cap is the platform's
 *  documented hard limit (or in the case of YouTube tags, the sum of
 *  tag bytes the API will accept). The form shows these inline as a
 *  countdown ("87/100") so the user can self-correct before the
 *  upload would otherwise reject. */
export const PLATFORM_LIMITS = {
  youtube: {
    titleMax: 100,
    descriptionMax: 5000,
    tagsTotalCharsMax: 500,
    captionMax: undefined as number | undefined,
  },
  tiktok: {
    titleMax: 150,
    descriptionMax: undefined as number | undefined,
    tagsTotalCharsMax: undefined as number | undefined,
    captionMax: undefined as number | undefined,
  },
  instagram: {
    titleMax: undefined as number | undefined,
    descriptionMax: undefined as number | undefined,
    tagsTotalCharsMax: undefined as number | undefined,
    captionMax: 2200,
  },
  twitter_x: {
    titleMax: 280,
    descriptionMax: undefined as number | undefined,
    tagsTotalCharsMax: undefined as number | undefined,
    captionMax: undefined as number | undefined,
  },
} as const;

export type ProviderKey = keyof typeof PLATFORM_LIMITS;

/** Errors discovered by the metadata validator. Each entry is shaped
 *  so the form can render it inline next to the offending field. */
export type MetadataValidationError = {
  /** Stable key — `title.required`, `title.too_long`, etc. */
  code: string;
  /** Which field the error belongs to. The form uses this to render
   *  inline next to the input. */
  field: "title" | "description" | "tags" | "caption" | "schedule" | "thumbnail";
  /** Human-facing copy. */
  message: string;
};

/** Validate one provider's metadata against its platform limits.
 *  Pure — no side effects, so this is the surface unit tests target. */
export function validateMetadata(
  provider: string,
  metadata: UploadMetadata,
): MetadataValidationError[] {
  const errors: MetadataValidationError[] = [];
  const limits = PLATFORM_LIMITS[provider as ProviderKey];

  if (provider === "instagram") {
    // Instagram has no title field — the "caption" maps to the
    // backend `description`. Treat description as caption for
    // validation purposes.
    const cap = limits?.captionMax;
    if (cap !== undefined && metadata.description.length > cap) {
      errors.push({
        code: "caption.too_long",
        field: "caption",
        message: `Caption is ${metadata.description.length}/${cap} characters.`,
      });
    }
  } else {
    if (metadata.title.trim().length === 0) {
      errors.push({
        code: "title.required",
        field: "title",
        message: "Title is required.",
      });
    }
    if (limits?.titleMax !== undefined && metadata.title.length > limits.titleMax) {
      errors.push({
        code: "title.too_long",
        field: "title",
        message: `Title is ${metadata.title.length}/${limits.titleMax} characters.`,
      });
    }
    if (
      limits?.descriptionMax !== undefined &&
      metadata.description.length > limits.descriptionMax
    ) {
      errors.push({
        code: "description.too_long",
        field: "description",
        message: `Description is ${metadata.description.length}/${limits.descriptionMax} characters.`,
      });
    }
  }

  if (limits?.tagsTotalCharsMax !== undefined) {
    const total = metadata.tags.reduce((acc, t) => acc + t.length, 0);
    if (total > limits.tagsTotalCharsMax) {
      errors.push({
        code: "tags.too_long",
        field: "tags",
        message: `Tags total ${total}/${limits.tagsTotalCharsMax} characters.`,
      });
    }
  }

  if (metadata.scheduledAt !== undefined) {
    if (!Number.isFinite(metadata.scheduledAt) || metadata.scheduledAt <= 0) {
      errors.push({
        code: "schedule.invalid",
        field: "schedule",
        message: "Schedule must be a valid date/time.",
      });
    } else if (metadata.scheduledAt * 1000 < Date.now() - 60_000) {
      // 1-minute grace window so a user who just picked "now" doesn't
      // trip the past-time error by the time the form submits.
      errors.push({
        code: "schedule.in_past",
        field: "schedule",
        message: "Schedule is in the past.",
      });
    }
  }

  return errors;
}

/** Default metadata for a freshly-registered `(jobId, provider)`. The
 *  title falls back to the render-job label so the user doesn't have
 *  to retype "YouTube 1080p" into the form before exporting. */
export function defaultMetadata(label: string): UploadMetadata {
  return {
    title: label,
    description: "",
    tags: [],
    visibility: "private",
    scheduledAt: undefined,
    thumbnailPath: undefined,
  };
}

interface UploadMetadataState {
  /** Map from `(jobId, provider)` → metadata. Sparse — only entries
   *  the user has actually touched are stored. */
  byKey: Record<string, UploadMetadata>;
  /** Read one entry, falling back to defaults derived from `label`. */
  get: (jobId: string, provider: string, label: string) => UploadMetadata;
  /** Replace metadata for one entry and persist it locally. */
  set: (jobId: string, provider: string, metadata: UploadMetadata) => void;
  /** Drop metadata for a job. Called when the render terminates so
   *  the store doesn't accumulate stale entries across sessions. */
  forgetJob: (jobId: string) => void;
}

const STORAGE_KEY = "montage.deliver.uploadMetadata.v1";

function loadLocal(): Record<string, UploadMetadata> {
  if (typeof localStorage === "undefined") return {};
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return {};
    const parsed = JSON.parse(raw) as unknown;
    if (typeof parsed !== "object" || parsed === null) return {};
    return parsed as Record<string, UploadMetadata>;
  } catch {
    return {};
  }
}

function persistLocal(byKey: Record<string, UploadMetadata>): void {
  if (typeof localStorage === "undefined") return;
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(byKey));
  } catch {
    // localStorage may be disabled (private window). Ignore.
  }
}

export const useUploadMetadata = create<UploadMetadataState>((set, get) => ({
  byKey: loadLocal(),
  get: (jobId, provider, label) => {
    const key = composeKey(jobId, provider);
    return get().byKey[key] ?? defaultMetadata(label);
  },
  set: (jobId, provider, metadata) => {
    const key = composeKey(jobId, provider);
    set((state) => {
      const next = { ...state.byKey, [key]: metadata };
      persistLocal(next);
      return { byKey: next };
    });
  },
  forgetJob: (jobId) => {
    set((state) => {
      const next: Record<string, UploadMetadata> = {};
      const prefix = `${jobId}::`;
      for (const [k, v] of Object.entries(state.byKey)) {
        if (!k.startsWith(prefix)) next[k] = v;
      }
      persistLocal(next);
      return { byKey: next };
    });
  },
}));

/** Visibility options the form exposes for each provider. Maps to the
 *  backend's three-value `Visibility` enum but the labels (and which
 *  values are surfaced) differ per platform — Instagram and Twitter/X
 *  have no visibility toggle in Montage's current publish path. */
export function visibilityOptionsFor(
  provider: string,
): Array<{ value: UploadVisibility; label: string }> | null {
  if (provider === "instagram" || provider === "twitter_x") return null;
  if (provider === "tiktok") {
    return [
      { value: "private", label: "Private" },
      // TikTok's "Friends only" surfaces as `unlisted` on the wire so
      // the backend doesn't need a new variant. The provider impl
      // (when it lands) maps `unlisted` → `MUTUAL_FOLLOW_FRIENDS`.
      { value: "unlisted", label: "Friends only" },
      { value: "public", label: "Public" },
    ];
  }
  // YouTube + future providers — three-way private/unlisted/public.
  return [
    { value: "private", label: "Private" },
    { value: "unlisted", label: "Unlisted" },
    { value: "public", label: "Public" },
  ];
}
