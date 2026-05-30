/**
 * SkillsSurface — the workspace surface for the Skills tab.
 *
 * Two-pane layout from the spec:
 *   [ list of skills (40%) | detail pane (60%) ]
 *
 * Left column: card per discovered skill with name, version chip,
 * one-line description, enable/disable switch. Selected card is
 * highlighted; clicking anywhere but the switch selects it.
 *
 * Right column: when a skill is selected, fetches the full SKILL.md
 * body and renders it via `react-markdown` (already a dependency for
 * the chat stream). The "Disabled" state mutes the card and adds a
 * pill — the toggle is UI-only today and does NOT affect the agent's
 * runtime loadout (see store comment).
 */

import { useCallback, useEffect, useMemo, useState } from "react";
import { invoke, isTauri } from "@tauri-apps/api/core";
import { openPath, revealItemInDir } from "@tauri-apps/plugin-opener";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { BookOpen, FolderOpen, Plus, Sparkles } from "lucide-react";

import { cn } from "../ui";
import { useProjectStore } from "../app/state";
import { useSkillsStore } from "../state";
import type { PinnedSkill } from "../state/skills";

/** Kebab-case name validator — lowercase letters / digits / dashes,
 *  must start and end with an alphanumeric. Mirrors the directory
 *  naming convention enforced by `SkillRegistry`. */
const SKILL_NAME_PATTERN = /^[a-z0-9]+(?:-[a-z0-9]+)*$/;

/**
 * Mirror of the Rust `SkillEntry` returned by `list_skills`.
 *
 * `provenance` identifies which discovery layer the skill came from:
 *   - "bundled" → ships with Awidat (lowest priority)
 *   - "user"    → ~/Library/Application Support/awidat/skills (etc.)
 *   - "project" → <project>/skills/  (highest priority — wins on name conflict)
 */
export type SkillProvenance = "bundled" | "user" | "project";
export type SkillEntry = {
  name: string;
  display_name: string;
  description: string;
  when_to_use: string | null;
  version: string | null;
  path: string;
  provenance: SkillProvenance;
};

/**
 * Read the list of bundled + user-overridden skills from the
 * desktop backend. Returns an empty list when not running inside
 * Tauri (e.g., dev-mode browser preview) so the surface can still
 * render its empty state without crashing.
 */
function useSkills(): {
  skills: SkillEntry[];
  loading: boolean;
  error: string | null;
  refresh: () => void;
} {
  const [skills, setSkills] = useState<SkillEntry[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [nonce, setNonce] = useState(0);
  const projectRoot = useProjectStore((s) => s.current);

  useEffect(() => {
    let cancelled = false;
    if (!isTauri()) {
      setSkills([]);
      setLoading(false);
      return;
    }
    setLoading(true);
    setError(null);
    invoke<SkillEntry[]>("list_skills")
      .then((rows) => {
        if (cancelled) return;
        setSkills(rows);
      })
      .catch((e: unknown) => {
        if (cancelled) return;
        setError(typeof e === "string" ? e : String(e));
      })
      .finally(() => {
        if (cancelled) return;
        setLoading(false);
      });
    return () => {
      cancelled = true;
    };
    // Refresh when the project changes — different projects can
    // ship different skill folders. `nonce` lets callers (e.g., the
    // "+ New skill" flow) force a refresh after writing a new SKILL.md.
  }, [projectRoot, nonce]);

  const refresh = useCallback(() => setNonce((n) => n + 1), []);
  return { skills, loading, error, refresh };
}

/** Lazy-load the SKILL.md body when a skill is selected. */
function useSkillBody(name: string | null): {
  body: string | null;
  loading: boolean;
  error: string | null;
} {
  const [body, setBody] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    if (!name || !isTauri()) {
      setBody(null);
      setLoading(false);
      setError(null);
      return;
    }
    setLoading(true);
    setError(null);
    invoke<string>("read_skill_body", { name })
      .then((text) => {
        if (cancelled) return;
        setBody(text);
      })
      .catch((e: unknown) => {
        if (cancelled) return;
        setError(typeof e === "string" ? e : String(e));
        setBody(null);
      })
      .finally(() => {
        if (cancelled) return;
        setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [name]);

  return { body, loading, error };
}

/**
 * Lazy-resolve the per-user skills directory the first time the
 * Skills tab opens. The backend creates it if missing so the
 * "Open skills folder" link always lands somewhere real on a fresh
 * install. Returns `null` while the call is pending or when not
 * running inside Tauri.
 */
function useUserSkillsDir(): string | null {
  const [dir, setDir] = useState<string | null>(null);
  useEffect(() => {
    if (!isTauri()) return;
    let cancelled = false;
    invoke<string>("ensure_user_skills_dir")
      .then((path) => {
        if (cancelled) return;
        setDir(path);
      })
      .catch((e: unknown) => {
        // Non-fatal: the "Open skills folder" link just stays
        // hidden. A locked-down home dir is the only realistic
        // cause; logging is enough.
        // eslint-disable-next-line no-console
        console.warn("ensure_user_skills_dir failed", e);
      });
    return () => {
      cancelled = true;
    };
  }, []);
  return dir;
}

export function SkillsSurface() {
  const { skills, loading, error, refresh } = useSkills();
  const userSkillsDir = useUserSkillsDir();
  const projectRoot = useProjectStore((s) => s.current);
  const disabledByProject = useSkillsStore((s) => s.disabledByProject);
  const pinnedByProject = useSkillsStore((s) => s.pinnedByProject);
  const toggle = useSkillsStore((s) => s.toggle);
  const setPin = useSkillsStore((s) => s.setPin);
  const clearPin = useSkillsStore((s) => s.clearPin);

  // Keep selection alive across rerenders. Default to the first
  // skill on first load so the right pane is never empty when there
  // are skills to show.
  const [selected, setSelected] = useState<string | null>(null);
  useEffect(() => {
    if (selected && skills.some((s) => s.name === selected)) return;
    setSelected(skills[0]?.name ?? null);
  }, [skills, selected]);

  const [createOpen, setCreateOpen] = useState(false);
  const onCreated = useCallback(
    (createdName: string) => {
      setCreateOpen(false);
      refresh();
      setSelected(createdName);
    },
    [refresh],
  );

  // Reactive lookups — pull from the maps directly so the
  // component re-renders when the user toggles or pins a skill.
  const isDisabled = useMemo(() => {
    const set = disabledByProject.get(projectRoot ?? "__global__");
    return (name: string) => set?.has(name) ?? false;
  }, [disabledByProject, projectRoot]);

  const getPin = useMemo(() => {
    const list = pinnedByProject.get(projectRoot ?? "__global__");
    return (name: string) =>
      list?.find((p) => p.name === name) ?? null;
  }, [pinnedByProject, projectRoot]);

  return (
    <div className="grid h-full min-h-0 grid-cols-[2fr_3fr] gap-2 p-3">
      <SkillList
        skills={skills}
        loading={loading}
        error={error}
        selected={selected}
        onSelect={setSelected}
        isDisabled={isDisabled}
        getPin={getPin}
        onToggle={(name) => toggle(projectRoot, name)}
        onSetPin={(pin) => setPin(projectRoot, pin)}
        onClearPin={(name) => clearPin(projectRoot, name)}
        userSkillsDir={userSkillsDir}
        onOpenCreate={() => setCreateOpen(true)}
      />
      <SkillDetail
        skill={skills.find((s) => s.name === selected) ?? null}
        disabled={selected ? isDisabled(selected) : false}
        pin={selected ? getPin(selected) : null}
      />
      {createOpen && (
        <CreateSkillModal
          existingNames={skills.map((s) => s.name)}
          projectRoot={projectRoot}
          onCancel={() => setCreateOpen(false)}
          onCreated={onCreated}
        />
      )}
    </div>
  );
}

type SkillListProps = {
  skills: SkillEntry[];
  loading: boolean;
  error: string | null;
  selected: string | null;
  onSelect: (name: string) => void;
  isDisabled: (name: string) => boolean;
  getPin: (name: string) => PinnedSkill | null;
  onToggle: (name: string) => void;
  onSetPin: (pin: PinnedSkill) => void;
  onClearPin: (name: string) => void;
  /**
   * Resolved per-user skills directory; the footer renders an "Open
   * folder" link that reveals it in the OS file manager. `null` when
   * we're not inside Tauri or the path couldn't be resolved — the
   * link is hidden in that case.
   */
  userSkillsDir: string | null;
  /** Opens the "+ New skill" modal — scaffolds an empty SKILL.md. */
  onOpenCreate: () => void;
};

function SkillList({
  skills,
  loading,
  error,
  selected,
  onSelect,
  isDisabled,
  getPin,
  onToggle,
  onSetPin,
  onClearPin,
  userSkillsDir,
  onOpenCreate,
}: SkillListProps) {
  const openSkillsFolder = () => {
    if (!userSkillsDir) return;
    revealItemInDir(userSkillsDir).catch((err) => {
      // eslint-disable-next-line no-console
      console.warn("revealItemInDir(skills) failed", err);
    });
  };
  const openAuthoringGuide = async () => {
    if (!isTauri()) return;
    try {
      const path = await invoke<string>("skills_authoring_guide_path");
      await openPath(path);
    } catch (err) {
      // Fall back to revealing the docs folder so the user can still
      // find it manually if the OS rejected the markdown handler.
      // eslint-disable-next-line no-console
      console.warn("open authoring guide failed", err);
    }
  };
  return (
    <section
      className="panel flex h-full min-h-0 flex-col overflow-hidden"
      aria-label="Skills"
    >
      <header className="flex shrink-0 items-center justify-between border-b border-[var(--color-border-subtle)] px-3 py-2">
        <div className="flex items-center gap-2 text-[var(--color-text-primary)]">
          <Sparkles className="h-3.5 w-3.5" strokeWidth={1.75} />
          <span className="text-[12px] font-semibold">Editorial Skills</span>
        </div>
        <span className="text-[11px] font-mono text-[var(--color-text-muted)]">
          {skills.length} {skills.length === 1 ? "skill" : "skills"}
        </span>
      </header>
      <div className="min-h-0 flex-1 overflow-y-auto">
        {error ? (
          <EmptyMessage tone="error">{error}</EmptyMessage>
        ) : loading ? (
          <EmptyMessage>Loading…</EmptyMessage>
        ) : skills.length === 0 ? (
          <EmptyMessage>
            No skills discovered. Add `SKILL.md` files under
            <code className="mx-1 rounded bg-[var(--color-surface-input)] px-1 font-mono">
              skills/
            </code>
            in your project or home directory.
          </EmptyMessage>
        ) : (
          <ul className="flex flex-col gap-1 p-2">
            {skills.map((skill) => (
              <li key={skill.name}>
                <SkillCard
                  skill={skill}
                  selected={selected === skill.name}
                  disabled={isDisabled(skill.name)}
                  pin={getPin(skill.name)}
                  onSelect={() => onSelect(skill.name)}
                  onToggle={() => onToggle(skill.name)}
                  onSetPin={onSetPin}
                  onClearPin={onClearPin}
                />
              </li>
            ))}
          </ul>
        )}
      </div>
      {userSkillsDir ? (
        <footer className="flex shrink-0 flex-col gap-1 border-t border-[var(--color-border-subtle)] px-3 py-2">
          <div className="flex items-center justify-between gap-2">
            <button
              type="button"
              onClick={openSkillsFolder}
              className="flex items-center gap-1.5 text-[11px] text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] transition-colors"
              title={userSkillsDir}
            >
              <FolderOpen className="h-3 w-3" strokeWidth={1.75} />
              <span>Skills folder</span>
            </button>
            <button
              type="button"
              onClick={onOpenCreate}
              className="flex items-center gap-1.5 rounded-[var(--radius-sm)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-input)] px-2 py-0.5 text-[11px] font-medium text-[var(--color-text-secondary)] hover:border-[var(--color-brand)] hover:text-[var(--color-brand)] transition-colors"
              title="Scaffold a new SKILL.md from the bundled template"
            >
              <Plus className="h-3 w-3" strokeWidth={2} />
              <span>New skill</span>
            </button>
          </div>
          <div className="flex items-center justify-between gap-2">
            <span
              className="truncate font-mono text-[10px] text-[var(--color-text-muted)]"
              title={userSkillsDir}
            >
              {userSkillsDir}
            </span>
            <button
              type="button"
              onClick={openAuthoringGuide}
              className="flex shrink-0 items-center gap-1 text-[10px] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] transition-colors"
              title="Open docs/skills-authoring.md"
            >
              <BookOpen className="h-2.5 w-2.5" strokeWidth={1.75} />
              <span>Authoring guide →</span>
            </button>
          </div>
        </footer>
      ) : null}
    </section>
  );
}

type CreateSkillModalProps = {
  /** Skills already in the catalog — used to flag duplicate names before
   *  the backend rejects them. Cheaper than a backend round-trip. */
  existingNames: string[];
  /** Active project root; the "Project" target is disabled when null. */
  projectRoot: string | null;
  onCancel: () => void;
  /** Called with the new skill's `name` after a successful write so the
   *  parent can refresh + select it. */
  onCreated: (name: string) => void;
};

/**
 * Compact modal for scaffolding a new skill from the bundled template.
 * The backend (`create_skill`) substitutes `{{name}}` / `{{description}}`
 * placeholders and writes the resulting SKILL.md to the chosen layer.
 */
function CreateSkillModal({
  existingNames,
  projectRoot,
  onCancel,
  onCreated,
}: CreateSkillModalProps) {
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  // Default to "user" — works on a fresh install with no project.
  // The "project" option is disabled in the dropdown when no project
  // is open, so we never start in a state we can't submit from.
  const [target, setTarget] = useState<"user" | "project">("user");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Esc cancels.
  useEffect(() => {
    function onKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape" && !busy) {
        event.preventDefault();
        onCancel();
      }
    }
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [busy, onCancel]);

  // Autofocus the name field.
  useEffect(() => {
    const el = document.getElementById("create-skill-name");
    if (el instanceof HTMLInputElement) el.focus();
  }, []);

  // Validation — surfaced inline so the Create button stays
  // accurately enabled/disabled.
  const nameValid = SKILL_NAME_PATTERN.test(name);
  const duplicate = existingNames.includes(name);
  const descriptionValid = description.trim().length > 0;
  const canSubmit =
    !busy &&
    nameValid &&
    !duplicate &&
    descriptionValid &&
    (target === "user" || projectRoot !== null);

  const submit = async () => {
    if (!canSubmit) return;
    setBusy(true);
    setError(null);
    try {
      await invoke<string>("create_skill", {
        target,
        name,
        description: description.trim(),
      });
      onCreated(name);
    } catch (e) {
      setError(typeof e === "string" ? e : String(e));
      setBusy(false);
    }
  };

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
      e.preventDefault();
      void submit();
    }
  };

  return (
    <div
      className="modal-backdrop"
      onClick={busy ? undefined : onCancel}
      role="presentation"
    >
      <div
        className="modal"
        onClick={(e) => e.stopPropagation()}
        onKeyDown={onKeyDown}
        role="dialog"
        aria-modal="true"
        aria-label="Create new skill"
        style={{ maxWidth: 480 }}
      >
        <header className="modal-header">
          <h2>New skill</h2>
          <button
            className="modal-close"
            onClick={onCancel}
            aria-label="Close"
            disabled={busy}
          >
            ×
          </button>
        </header>
        <div className="modal-body">
          <label className="field">
            <span>Name</span>
            <input
              id="create-skill-name"
              type="text"
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder="my-skill"
              autoComplete="off"
              spellCheck={false}
              disabled={busy}
            />
            <span
              className={cn(
                "text-[10px]",
                name && (!nameValid || duplicate)
                  ? "text-[var(--color-status-error)]"
                  : "text-[var(--color-text-muted)]",
              )}
            >
              {name && !nameValid
                ? "kebab-case only (lowercase letters, digits, dashes)"
                : duplicate
                  ? `A skill named "${name}" already exists`
                  : "kebab-case — matches the directory the file will live in"}
            </span>
          </label>
          <label className="field">
            <span>Description</span>
            <input
              type="text"
              value={description}
              onChange={(e) => setDescription(e.target.value)}
              placeholder="One line describing what this skill does."
              disabled={busy}
            />
          </label>
          <label className="field">
            <span>Where to create</span>
            <select
              value={target}
              onChange={(e) => setTarget(e.target.value as "user" | "project")}
              disabled={busy}
              className="h-[30px] w-full rounded-[var(--radius-sm)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-input)] px-2 text-[var(--text-body-sm)] text-[var(--color-text-primary)]"
            >
              <option value="user">
                User · ~/Library/Application Support/awidat/skills/
              </option>
              <option value="project" disabled={!projectRoot}>
                Project ·{" "}
                {projectRoot
                  ? `${projectRoot}/skills/`
                  : "(no project open)"}
              </option>
            </select>
          </label>
          {error && <div className="field-error">{error}</div>}
        </div>
        <footer className="modal-footer">
          <button onClick={onCancel} disabled={busy}>
            Cancel
          </button>
          <button className="primary" onClick={submit} disabled={!canSubmit}>
            {busy ? "Creating…" : "Create"}
          </button>
        </footer>
      </div>
    </div>
  );
}

type SkillCardProps = {
  skill: SkillEntry;
  selected: boolean;
  disabled: boolean;
  pin: PinnedSkill | null;
  onSelect: () => void;
  onToggle: () => void;
  onSetPin: (pin: PinnedSkill) => void;
  onClearPin: (name: string) => void;
};

/**
 * Versions above this string get a "Pin" affordance. Skills authored
 * at the default `0.1.0` floor get no pin link — they have nothing to
 * lock against. Anything above (1.0.0, 1.2.0, …) is fair game.
 */
const PIN_VERSION_FLOOR = "1.0.0";

function meetsPinFloor(version: string | null): boolean {
  if (!version) return false;
  // Lexicographic-safe compare via three-part split; mirrors the
  // backend's normalized comparison.
  const parts = version.split(".").map((p) => parseInt(p, 10));
  if (parts.some((p) => Number.isNaN(p))) return false;
  const floor = PIN_VERSION_FLOOR.split(".").map((p) => parseInt(p, 10));
  for (let i = 0; i < Math.max(parts.length, floor.length); i++) {
    const a = parts[i] ?? 0;
    const b = floor[i] ?? 0;
    if (a !== b) return a > b;
  }
  return true; // equal counts as "meets floor"
}

function SkillCard({
  skill,
  selected,
  disabled,
  pin,
  onSelect,
  onToggle,
  onSetPin,
  onClearPin,
}: SkillCardProps) {
  const canPin = meetsPinFloor(skill.version);
  return (
    <div
      className={cn(
        "group relative rounded-[var(--radius-sm)] border px-3 py-2.5 transition-colors",
        selected
          ? "border-[var(--color-brand)] bg-[var(--color-surface-selected)]"
          : "border-[var(--color-border-subtle)] bg-[var(--color-surface-app)] hover:border-[var(--color-border-strong)]",
        disabled && "opacity-60",
      )}
    >
      <button
        type="button"
        onClick={onSelect}
        aria-pressed={selected}
        className="block w-full text-left"
      >
        <div className="flex items-start justify-between gap-2">
          <div className="min-w-0 flex-1">
            <div className="flex items-center gap-2 flex-wrap">
              <span className="truncate text-[12px] font-semibold text-[var(--color-text-primary)]">
                {skill.display_name}
              </span>
              <ProvenanceChip provenance={skill.provenance} />
              {skill.version && (
                <span className="shrink-0 rounded-full bg-[var(--color-surface-input)] px-1.5 py-0.5 font-mono text-[10px] text-[var(--color-text-muted)]">
                  v{skill.version}
                </span>
              )}
              {pin && <PinChip pin={pin} />}
              {disabled && (
                <span className="shrink-0 rounded-full border border-[var(--color-border-subtle)] px-1.5 py-0.5 text-[10px] font-medium text-[var(--color-text-muted)]">
                  Disabled
                </span>
              )}
            </div>
            <p className="mt-1 line-clamp-2 text-[11px] leading-snug text-[var(--color-text-secondary)]">
              {skill.description}
            </p>
          </div>
        </div>
      </button>
      <div className="mt-2 flex items-center justify-between gap-2">
        {canPin ? (
          <PinControls
            skill={skill}
            pin={pin}
            onSetPin={onSetPin}
            onClearPin={onClearPin}
          />
        ) : (
          <span />
        )}
        <Switch
          checked={!disabled}
          onChange={onToggle}
          ariaLabel={`${disabled ? "Enable" : "Disable"} ${skill.display_name}`}
        />
      </div>
    </div>
  );
}

type PinControlsProps = {
  skill: SkillEntry;
  pin: PinnedSkill | null;
  onSetPin: (pin: PinnedSkill) => void;
  onClearPin: (name: string) => void;
};

/**
 * Pin / unpin link + provenance dropdown rendered inline under the
 * SkillCard's toggle row. Clicking "Pin v1.2.0" locks the project to
 * that exact version; the dropdown lets the editor further constrain
 * the discovery layer (bundled / user / project / any).
 */
function PinControls({ skill, pin, onSetPin, onClearPin }: PinControlsProps) {
  const version = skill.version ?? "";
  const handlePin = () => {
    if (!version) return;
    onSetPin({
      name: skill.name,
      version,
      // Default the pin's provenance to "any" — the editor can pick a
      // specific layer from the dropdown after pinning.
    });
  };
  const handleProvenanceChange = (
    next: PinnedSkill["provenance"] | undefined,
  ) => {
    if (!pin) return;
    onSetPin({ ...pin, provenance: next });
  };
  if (!pin) {
    return (
      <button
        type="button"
        onClick={handlePin}
        className="text-[11px] font-medium text-[var(--color-text-secondary)] hover:text-[var(--color-brand)] transition-colors"
        title={`Lock this project to v${version}`}
      >
        Pin v{version}
      </button>
    );
  }
  return (
    <div className="flex items-center gap-2">
      <button
        type="button"
        onClick={() => onClearPin(skill.name)}
        className="text-[11px] font-medium text-[var(--color-text-secondary)] hover:text-[var(--color-status-error)] transition-colors"
        title="Remove this version pin"
      >
        Unpin
      </button>
      <select
        value={pin.provenance ?? ""}
        onChange={(e) =>
          handleProvenanceChange(
            (e.target.value || undefined) as PinnedSkill["provenance"] | undefined,
          )
        }
        aria-label="Pinned provenance layer"
        title="Lock to a specific discovery layer"
        className="rounded border border-[var(--color-border-subtle)] bg-[var(--color-surface-input)] px-1 py-0.5 font-mono text-[10px] text-[var(--color-text-secondary)]"
      >
        <option value="">any layer</option>
        <option value="bundled">bundled</option>
        <option value="user">user</option>
        <option value="project">project</option>
      </select>
    </div>
  );
}

/**
 * Compact chip showing the active pin's constraints. Uses the unicode
 * pushpin glyph so we don't pull in another icon dependency for a
 * single 12px badge.
 */
function PinChip({ pin }: { pin: PinnedSkill }) {
  const parts: string[] = [];
  if (pin.version) parts.push(`v${pin.version}`);
  if (pin.provenance) parts.push(pin.provenance);
  const label = parts.length > 0 ? parts.join(" · ") : "pinned";
  return (
    <span
      title={`Pinned${pin.version ? ` to v${pin.version}` : ""}${
        pin.provenance ? ` · ${pin.provenance} layer` : ""
      }`}
      className="shrink-0 rounded-full border border-[color:var(--color-brand)]/40 bg-[color:var(--color-brand)]/15 px-1.5 py-0.5 font-mono text-[10px] text-[var(--color-brand)]"
    >
      <span aria-hidden className="mr-0.5">
        {"\u{1F4CC}"}
      </span>
      {label}
    </span>
  );
}

type SwitchProps = {
  checked: boolean;
  onChange: () => void;
  ariaLabel: string;
};

/**
 * Token-styled toggle switch. Keeps to the existing surface +
 * brand color tokens so it matches the rest of the chrome.
 */
function Switch({ checked, onChange, ariaLabel }: SwitchProps) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={ariaLabel}
      onClick={onChange}
      className={cn(
        "relative inline-flex h-4 w-7 shrink-0 items-center rounded-full transition-colors",
        checked
          ? "bg-[var(--color-brand)]"
          : "bg-[var(--color-surface-input)]",
      )}
    >
      <span
        aria-hidden
        className={cn(
          "block h-3 w-3 rounded-full bg-white shadow transition-transform",
          checked ? "translate-x-3.5" : "translate-x-0.5",
        )}
      />
    </button>
  );
}

type SkillDetailProps = {
  skill: SkillEntry | null;
  disabled: boolean;
  pin: PinnedSkill | null;
};

function SkillDetail({ skill, disabled, pin }: SkillDetailProps) {
  const { body, loading, error } = useSkillBody(skill?.name ?? null);

  if (!skill) {
    return (
      <section className="panel flex h-full min-h-0 items-center justify-center">
        <span className="text-[12px] text-[var(--color-text-muted)]">
          Select a skill to view its playbook.
        </span>
      </section>
    );
  }

  return (
    <section className="panel flex h-full min-h-0 flex-col overflow-hidden">
      <header className="flex shrink-0 items-start justify-between gap-3 border-b border-[var(--color-border-subtle)] px-4 py-3">
        <div className="min-w-0">
          <div className="flex items-center gap-2 flex-wrap">
            <h2 className="truncate text-[14px] font-semibold text-[var(--color-text-primary)]">
              {skill.display_name}
            </h2>
            <ProvenanceChip provenance={skill.provenance} />
            {skill.version && (
              <span className="shrink-0 rounded-full bg-[var(--color-surface-input)] px-1.5 py-0.5 font-mono text-[10px] text-[var(--color-text-muted)]">
                v{skill.version}
              </span>
            )}
            {pin && <PinChip pin={pin} />}
            {disabled && (
              <span className="shrink-0 rounded-full border border-[var(--color-border-subtle)] px-1.5 py-0.5 text-[10px] font-medium text-[var(--color-text-muted)]">
                Disabled
              </span>
            )}
          </div>
          <p className="mt-1 text-[12px] leading-snug text-[var(--color-text-secondary)]">
            {skill.description}
          </p>
          {skill.when_to_use && (
            <p className="mt-2 rounded-[var(--radius-sm)] border-l-2 border-[var(--color-brand-secondary)] bg-[var(--color-surface-app)] px-2 py-1 text-[11px] text-[var(--color-text-secondary)]">
              <span className="font-semibold text-[var(--color-text-primary)]">When to use: </span>
              {skill.when_to_use}
            </p>
          )}
        </div>
      </header>
      <div className="min-h-0 flex-1 overflow-y-auto px-4 py-3">
        {error ? (
          <EmptyMessage tone="error">{error}</EmptyMessage>
        ) : loading ? (
          <EmptyMessage>Loading playbook…</EmptyMessage>
        ) : body ? (
          <div className="markdown text-[12px] text-[var(--color-text-primary)]">
            <ReactMarkdown remarkPlugins={[remarkGfm]}>{body}</ReactMarkdown>
          </div>
        ) : (
          <EmptyMessage>No body content.</EmptyMessage>
        )}
      </div>
      <footer className="shrink-0 border-t border-[var(--color-border-subtle)] px-4 py-2">
        <span className="font-mono text-[10px] text-[var(--color-text-muted)]">{skill.path}</span>
      </footer>
    </section>
  );
}

/**
 * Tiny chip rendered next to a skill's name showing which discovery
 * layer it came from. Color convention (matches the broader brand
 * vocabulary):
 *   - bundled → neutral muted   (no special meaning — ships with app)
 *   - user    → cyan accent     (--color-brand-secondary, "yours")
 *   - project → orange accent   (--color-brand, "project-scoped")
 */
function ProvenanceChip({ provenance }: { provenance: SkillProvenance }) {
  const palette: Record<
    SkillProvenance,
    { label: string; className: string; title: string }
  > = {
    bundled: {
      label: "bundled",
      className:
        "border-[var(--color-border-subtle)] bg-[var(--color-surface-input)] text-[var(--color-text-muted)]",
      title: "Ships with Awidat",
    },
    user: {
      label: "user",
      className:
        "border-[color:var(--color-brand-secondary)]/40 bg-[color:var(--color-brand-secondary)]/15 text-[var(--color-brand-secondary)]",
      title: "Installed from your per-user skills folder",
    },
    project: {
      label: "project",
      className:
        "border-[color:var(--color-brand)]/40 bg-[color:var(--color-brand)]/15 text-[var(--color-brand)]",
      title: "Defined by this project — overrides user and bundled",
    },
  };
  const { label, className, title } = palette[provenance];
  return (
    <span
      title={title}
      className={cn(
        "shrink-0 rounded-full border px-1.5 py-0.5 font-mono text-[10px] uppercase tracking-wider",
        className,
      )}
    >
      {label}
    </span>
  );
}

function EmptyMessage({
  children,
  tone = "muted",
}: {
  children: React.ReactNode;
  tone?: "muted" | "error";
}) {
  return (
    <div
      className={cn(
        "flex h-full w-full items-center justify-center p-6 text-center text-[12px]",
        tone === "error"
          ? "text-[var(--color-status-error)]"
          : "text-[var(--color-text-muted)]",
      )}
    >
      <span>{children}</span>
    </div>
  );
}
