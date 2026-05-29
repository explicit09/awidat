// Per-target upload metadata form (W5.A3).
//
// When the Deliver view has one or more publishing targets toggled
// on, a small inline form appears letting the user configure each
// target's title / description / tags / visibility / schedule /
// thumbnail before they hit Export. Layout:
//
//   ─── Upload details ─────────────────────────
//   [▾ YouTube]   title: [_____]  desc: [_____]
//                 tags:  [chips]   vis:  [○ ○ ●]
//                 schedule: [____]   thumb: [⇧]
//   [▸ TikTok]
//   [▸ Instagram]
//
// Each provider renders inside a per-target Accordion so a user with
// three targets selected doesn't see a wall of forms — the form only
// expands the target they're currently editing.
//
// Validation runs on every keystroke (cheap — `validateMetadata` is
// pure) and renders inline next to the offending field. The form
// doesn't block submission; we let the user save partial state and
// flag errors next to the field so they can fix at their own pace.
//
// The form lives inline in the Deliver right column — not a modal —
// per W5.A3's "no modal hops" UX rule.

import { Image as ImageIcon, Upload as UploadIcon, X } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { cn, Inline, Stack } from "../../ui";
import {
  PLATFORM_LIMITS,
  validateMetadata,
  visibilityOptionsFor,
  defaultMetadata,
  useUploadMetadata,
  type MetadataValidationError,
  type ProviderKey,
  type UploadMetadata,
  type UploadVisibility,
} from "../../state/uploadMetadata";
import { TARGET_META } from "./targetMeta";
import type { DeliveryTargetKey } from "./types";

/**
 * Inline per-target metadata form. Lives in DeliverySurface's right
 * column when at least one publishing target is opted into upload.
 *
 * `jobIdHint` is the queue entry id that the metadata is keyed to.
 * Before a render is enqueued, the form scopes by the synthesised
 * id `pending:<target>`; on enqueue the worker copies the metadata
 * over to the real `(jobId, provider)` pair. This keeps the form
 * state pre-render usable without forcing the user to type metadata
 * twice (once to draft, once after Export).
 */
export function UploadMetadataForm({
  selectedTargets,
  jobIdHint,
  defaultTitle,
}: {
  /** Publishing-provider keys the user has toggled on. */
  selectedTargets: ReadonlyArray<DeliveryTargetKey>;
  /** Scope key for the metadata store — render job id when known,
   *  a `pending:<target>` placeholder before Export. */
  jobIdHint: string;
  /** Title to seed each new provider's form with. Usually the
   *  render-target's label (e.g. "YouTube 1080p"). */
  defaultTitle: string;
}) {
  const publishingTargets = useMemo(
    () => selectedTargets.filter((t) => isPublishingTarget(t)),
    [selectedTargets],
  );
  // Default the first target open so a single-target user doesn't
  // have to click anywhere to see the form.
  const [openTarget, setOpenTarget] = useState<DeliveryTargetKey | null>(
    publishingTargets[0] ?? null,
  );
  // Keep the open target valid as the user toggles targets on/off.
  useEffect(() => {
    if (openTarget && !publishingTargets.includes(openTarget)) {
      setOpenTarget(publishingTargets[0] ?? null);
    } else if (!openTarget && publishingTargets.length > 0) {
      setOpenTarget(publishingTargets[0]);
    }
  }, [openTarget, publishingTargets]);

  if (publishingTargets.length === 0) return null;

  return (
    <Stack gap="2">
      <span className="text-[var(--text-label)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-muted)]">
        Upload details
      </span>
      <Stack gap="1">
        {publishingTargets.map((target) => (
          <UploadTargetSection
            key={target}
            target={target}
            jobIdHint={jobIdHint}
            defaultTitle={defaultTitle}
            isOpen={openTarget === target}
            onToggle={() =>
              setOpenTarget((cur) => (cur === target ? null : target))
            }
          />
        ))}
      </Stack>
    </Stack>
  );
}

function isPublishingTarget(key: DeliveryTargetKey): boolean {
  return key === "youtube" || key === "tiktok" || key === "instagram";
}

/** One target's collapsible section. */
function UploadTargetSection({
  target,
  jobIdHint,
  defaultTitle,
  isOpen,
  onToggle,
}: {
  target: DeliveryTargetKey;
  jobIdHint: string;
  defaultTitle: string;
  isOpen: boolean;
  onToggle: () => void;
}) {
  const meta = TARGET_META[target];
  // Subscribe to the form state for this target so edits re-render
  // inline. The store also persists to localStorage + the backend so
  // a page reload preserves the in-progress form.
  const stored = useUploadMetadata((s) => s.byKey[`${jobIdHint}::${target}`]);
  const metadata = stored ?? defaultMetadata(defaultTitle);
  const setMetadata = useUploadMetadata((s) => s.set);
  const errors = useMemo(
    () => validateMetadata(target, metadata),
    [target, metadata],
  );
  const errorCount = errors.length;

  function update(next: Partial<UploadMetadata>): void {
    setMetadata(jobIdHint, target, { ...metadata, ...next });
  }

  return (
    <div className="rounded-[var(--radius-md)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-card)]">
      <button
        type="button"
        onClick={onToggle}
        aria-expanded={isOpen}
        className="flex w-full items-center justify-between px-3 py-2 text-left"
      >
        <Inline gap="2" align="center">
          <span aria-hidden className="text-[var(--color-text-muted)]">
            {isOpen ? "▾" : "▸"}
          </span>
          <span className="text-[var(--text-body-sm)] font-medium text-[var(--color-text-primary)]">
            {meta.label}
          </span>
          {errorCount > 0 ? (
            <span
              className="rounded-full bg-[var(--color-danger)] px-1.5 text-[var(--text-caption)] text-[var(--color-text-inverse)]"
              title={`${errorCount} error${errorCount === 1 ? "" : "s"}`}
            >
              {errorCount}
            </span>
          ) : null}
        </Inline>
        <span className="font-mono text-[var(--text-caption)] text-[var(--color-text-muted)]">
          {summarizeMetadata(target, metadata)}
        </span>
      </button>
      {isOpen ? (
        <div className="border-t border-[var(--color-border-subtle)] px-3 py-3">
          <MetadataFields
            target={target}
            metadata={metadata}
            errors={errors}
            onUpdate={update}
          />
        </div>
      ) : null}
    </div>
  );
}

/** Render a one-line summary of the metadata for the section header
 *  so a closed section still gives the user a sense of what's saved. */
function summarizeMetadata(
  target: DeliveryTargetKey,
  metadata: UploadMetadata,
): string {
  const parts: string[] = [];
  if (target === "instagram") {
    if (metadata.description.trim().length > 0) {
      parts.push(`${metadata.description.length} chars`);
    }
  } else if (metadata.title.trim().length > 0) {
    parts.push(metadata.title.slice(0, 28));
  }
  if (metadata.tags.length > 0) parts.push(`${metadata.tags.length} tags`);
  if (metadata.scheduledAt) parts.push(`@ ${formatScheduleShort(metadata.scheduledAt)}`);
  return parts.join(" · ");
}

function formatScheduleShort(secs: number): string {
  try {
    return new Date(secs * 1000).toLocaleString(undefined, {
      month: "short",
      day: "numeric",
      hour: "2-digit",
      minute: "2-digit",
    });
  } catch {
    return "scheduled";
  }
}

/** All form fields — title, description / caption, tags, visibility,
 *  schedule, thumbnail. Each provider gets a slightly different set
 *  (no visibility on IG, no title on IG, different visibility labels
 *  on TikTok). */
function MetadataFields({
  target,
  metadata,
  errors,
  onUpdate,
}: {
  target: DeliveryTargetKey;
  metadata: UploadMetadata;
  errors: MetadataValidationError[];
  onUpdate: (next: Partial<UploadMetadata>) => void;
}) {
  const isInstagram = target === "instagram";
  const limits = PLATFORM_LIMITS[target as ProviderKey];
  const visOpts = visibilityOptionsFor(target);
  const errFor = (field: MetadataValidationError["field"]) =>
    errors.find((e) => e.field === field);

  return (
    <Stack gap="3">
      {!isInstagram ? (
        <Field
          label="Title"
          counterCurrent={metadata.title.length}
          counterMax={limits?.titleMax}
          error={errFor("title")?.message}
        >
          <input
            type="text"
            value={metadata.title}
            onChange={(e) => onUpdate({ title: e.target.value })}
            placeholder="Required"
            className={fieldInputClass}
          />
        </Field>
      ) : null}
      <Field
        label={isInstagram ? "Caption" : "Description"}
        counterCurrent={metadata.description.length}
        counterMax={isInstagram ? limits?.captionMax : limits?.descriptionMax}
        error={(isInstagram ? errFor("caption") : errFor("description"))?.message}
      >
        <textarea
          value={metadata.description}
          onChange={(e) => onUpdate({ description: e.target.value })}
          rows={3}
          placeholder={isInstagram ? "Caption shown under the post" : "Long-form description"}
          className={fieldTextareaClass}
        />
      </Field>
      <Field
        label="Tags"
        counterCurrent={metadata.tags.reduce((acc, t) => acc + t.length, 0)}
        counterMax={limits?.tagsTotalCharsMax}
        error={errFor("tags")?.message}
      >
        <TagChipInput
          value={metadata.tags}
          onChange={(tags) => onUpdate({ tags })}
        />
      </Field>
      {visOpts ? (
        <Field label="Visibility">
          <VisibilityRadio
            options={visOpts}
            value={metadata.visibility}
            onChange={(visibility) => onUpdate({ visibility })}
          />
        </Field>
      ) : (
        <Field label="Visibility">
          <span className="text-[var(--text-caption)] text-[var(--color-text-muted)]">
            Always public on {TARGET_META[target].label}.
          </span>
        </Field>
      )}
      <Field label="Schedule" error={errFor("schedule")?.message}>
        <ScheduleInput
          value={metadata.scheduledAt}
          onChange={(scheduledAt) => onUpdate({ scheduledAt })}
        />
      </Field>
      <Field label="Thumbnail">
        <ThumbnailPicker
          value={metadata.thumbnailPath}
          onChange={(thumbnailPath) => onUpdate({ thumbnailPath })}
        />
      </Field>
    </Stack>
  );
}

/** Token-only field wrapper — label + control + optional counter +
 *  optional inline error. Keeps the form rhythm consistent across
 *  field types (text input, textarea, chip input, radio, etc). */
function Field({
  label,
  counterCurrent,
  counterMax,
  error,
  children,
}: {
  label: string;
  counterCurrent?: number;
  counterMax?: number;
  error?: string;
  children: React.ReactNode;
}) {
  const showCounter = counterMax !== undefined && counterCurrent !== undefined;
  return (
    <Stack gap="1">
      <Inline justify="between" align="baseline">
        <span className="text-[var(--text-caption)] font-medium text-[var(--color-text-secondary)]">
          {label}
        </span>
        {showCounter ? (
          <span
            className={cn(
              "font-mono text-[var(--text-caption)]",
              counterCurrent! > counterMax!
                ? "text-[var(--color-danger)]"
                : "text-[var(--color-text-muted)]",
            )}
          >
            {counterCurrent}/{counterMax}
          </span>
        ) : null}
      </Inline>
      {children}
      {error ? (
        <span className="text-[var(--text-caption)] text-[var(--color-danger)]">
          {error}
        </span>
      ) : null}
    </Stack>
  );
}

const fieldInputClass = cn(
  "w-full rounded-[var(--radius-sm)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-input)] px-2 py-1.5",
  "text-[var(--text-body-sm)] text-[var(--color-text-primary)]",
  "outline-none transition-colors focus:border-[var(--color-brand)]",
);

const fieldTextareaClass = cn(
  fieldInputClass,
  "resize-y min-h-[64px] font-[inherit]",
);

/** Tag chip input — accepts commas + Enter to commit a chip. Empty
 *  chips are filtered out. The user can also paste a comma-separated
 *  list. */
function TagChipInput({
  value,
  onChange,
}: {
  value: string[];
  onChange: (tags: string[]) => void;
}) {
  const [draft, setDraft] = useState("");

  function commit(raw: string): void {
    const next = raw
      .split(",")
      .map((s) => s.trim())
      .filter((s) => s.length > 0);
    if (next.length === 0) return;
    // De-dup against the existing tags so paste-spam doesn't grow
    // the list with the same value.
    const merged = [...value];
    for (const tag of next) {
      if (!merged.includes(tag)) merged.push(tag);
    }
    onChange(merged);
    setDraft("");
  }

  function removeAt(index: number): void {
    const next = value.filter((_, i) => i !== index);
    onChange(next);
  }

  return (
    <div
      className={cn(
        "flex flex-wrap items-center gap-1 rounded-[var(--radius-sm)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-input)] px-2 py-1.5",
        "focus-within:border-[var(--color-brand)]",
      )}
    >
      {value.map((tag, index) => (
        <span
          key={`${tag}-${index}`}
          className="inline-flex items-center gap-1 rounded-full border border-[var(--color-border-subtle)] bg-[var(--color-surface-card)] px-2 py-0.5 text-[var(--text-caption)] text-[var(--color-text-primary)]"
        >
          {tag}
          <button
            type="button"
            onClick={() => removeAt(index)}
            aria-label={`Remove tag ${tag}`}
            className="text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)]"
          >
            <X className="h-3 w-3 stroke-[1.75]" />
          </button>
        </span>
      ))}
      <input
        type="text"
        value={draft}
        placeholder={value.length === 0 ? "Add tags (comma to commit)" : ""}
        onChange={(e) => {
          const v = e.target.value;
          if (v.endsWith(",")) {
            commit(v.slice(0, -1));
          } else {
            setDraft(v);
          }
        }}
        onKeyDown={(e) => {
          if (e.key === "Enter") {
            e.preventDefault();
            commit(draft);
          } else if (e.key === "Backspace" && draft.length === 0 && value.length > 0) {
            removeAt(value.length - 1);
          }
        }}
        onBlur={() => {
          if (draft.trim().length > 0) commit(draft);
        }}
        className="min-w-[8ch] flex-1 bg-transparent text-[var(--text-body-sm)] text-[var(--color-text-primary)] outline-none"
      />
    </div>
  );
}

/** Visibility radio. Uses native radio inputs for keyboard support;
 *  token-styled per the design system. */
function VisibilityRadio({
  options,
  value,
  onChange,
}: {
  options: ReadonlyArray<{ value: UploadVisibility; label: string }>;
  value: UploadVisibility;
  onChange: (next: UploadVisibility) => void;
}) {
  return (
    <Inline gap="3" align="center" wrap="wrap">
      {options.map((opt) => (
        <label
          key={opt.value}
          className={cn(
            "inline-flex cursor-pointer items-center gap-1.5 text-[var(--text-body-sm)]",
            value === opt.value
              ? "text-[var(--color-text-primary)]"
              : "text-[var(--color-text-secondary)]",
          )}
        >
          <input
            type="radio"
            name={`visibility-${opt.value}`}
            value={opt.value}
            checked={value === opt.value}
            onChange={() => onChange(opt.value)}
            className="accent-[var(--color-brand)]"
          />
          {opt.label}
        </label>
      ))}
    </Inline>
  );
}

/** Date+time picker. Native `datetime-local` keeps us out of an extra
 *  npm dep; emits a unix-epoch-seconds value to the store. The
 *  "Clear" affordance removes the schedule entirely. */
function ScheduleInput({
  value,
  onChange,
}: {
  value: number | undefined;
  onChange: (next: number | undefined) => void;
}) {
  // datetime-local wants `YYYY-MM-DDTHH:mm` in the *user's* local
  // tz, no tz suffix. Convert from epoch seconds.
  const localStr = useMemo(() => {
    if (!value) return "";
    try {
      const d = new Date(value * 1000);
      const yyyy = d.getFullYear();
      const mm = String(d.getMonth() + 1).padStart(2, "0");
      const dd = String(d.getDate()).padStart(2, "0");
      const hh = String(d.getHours()).padStart(2, "0");
      const mi = String(d.getMinutes()).padStart(2, "0");
      return `${yyyy}-${mm}-${dd}T${hh}:${mi}`;
    } catch {
      return "";
    }
  }, [value]);

  return (
    <Inline gap="2" align="center">
      <input
        type="datetime-local"
        value={localStr}
        onChange={(e) => {
          const raw = e.target.value;
          if (!raw) {
            onChange(undefined);
            return;
          }
          const parsed = new Date(raw);
          if (Number.isNaN(parsed.getTime())) {
            onChange(undefined);
            return;
          }
          onChange(Math.floor(parsed.getTime() / 1000));
        }}
        className={cn(fieldInputClass, "max-w-[220px]")}
      />
      {value ? (
        <button
          type="button"
          onClick={() => onChange(undefined)}
          className="text-[var(--text-caption)] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)]"
        >
          Clear
        </button>
      ) : (
        <span className="text-[var(--text-caption)] text-[var(--color-text-muted)]">
          Publishes immediately when blank
        </span>
      )}
    </Inline>
  );
}

/** Thumbnail file picker. Uses Tauri's `dialog::open` to grab a path,
 *  then `convertFileSrc` to render a preview thumbnail. Outside the
 *  Tauri shell the button is disabled — no fallback HTML file input
 *  because the backend expects an absolute filesystem path. */
function ThumbnailPicker({
  value,
  onChange,
}: {
  value: string | undefined;
  onChange: (next: string | undefined) => void;
}) {
  const [previewSrc, setPreviewSrc] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!value) {
      setPreviewSrc(null);
      return;
    }
    let cancelled = false;
    (async () => {
      try {
        const { convertFileSrc } = await import("@tauri-apps/api/core");
        if (!cancelled) setPreviewSrc(convertFileSrc(value));
      } catch {
        if (!cancelled) setPreviewSrc(null);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [value]);

  async function pickFile(): Promise<void> {
    setError(null);
    try {
      const { open } = await import("@tauri-apps/plugin-dialog");
      const picked = await open({
        multiple: false,
        directory: false,
        title: "Choose thumbnail image",
        filters: [{ name: "Image", extensions: ["png", "jpg", "jpeg"] }],
      });
      if (typeof picked === "string") onChange(picked);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }

  const fileName = value ? value.split(/[\\/]/).pop() : null;

  return (
    <Stack gap="2">
      <Inline gap="2" align="center">
        <button
          type="button"
          onClick={() => void pickFile()}
          className={cn(
            "inline-flex items-center gap-1.5 rounded-[var(--radius-sm)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-input)] px-2 py-1",
            "text-[var(--text-caption)] text-[var(--color-text-primary)] hover:border-[var(--color-text-muted)]",
          )}
        >
          <UploadIcon className="h-3.5 w-3.5 stroke-[1.75]" />
          {value ? "Replace…" : "Pick image…"}
        </button>
        {value ? (
          <button
            type="button"
            onClick={() => onChange(undefined)}
            className="text-[var(--text-caption)] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)]"
          >
            Clear
          </button>
        ) : null}
        {fileName ? (
          <span
            className="truncate text-[var(--text-caption)] text-[var(--color-text-secondary)]"
            title={value ?? undefined}
          >
            {fileName}
          </span>
        ) : null}
      </Inline>
      {previewSrc ? (
        <div className="flex h-16 w-28 items-center justify-center overflow-hidden rounded-[var(--radius-sm)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-input)]">
          {/* eslint-disable-next-line @next/next/no-img-element */}
          <img
            src={previewSrc}
            alt="Thumbnail preview"
            className="max-h-full max-w-full object-contain"
          />
        </div>
      ) : value ? (
        <div className="flex h-16 w-28 items-center justify-center rounded-[var(--radius-sm)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-input)]">
          <ImageIcon className="h-5 w-5 stroke-[1.5] text-[var(--color-text-muted)]" />
        </div>
      ) : null}
      {error ? (
        <span className="text-[var(--text-caption)] text-[var(--color-danger)]">{error}</span>
      ) : null}
    </Stack>
  );
}
