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

import { useEffect, useMemo, useState } from "react";
import { invoke, isTauri } from "@tauri-apps/api/core";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { Sparkles } from "lucide-react";

import { cn } from "../ui";
import { useProjectStore } from "../app/state";
import { useSkillsStore } from "../state";

/** Mirror of the Rust `SkillEntry` returned by `list_skills`. */
export type SkillEntry = {
  name: string;
  display_name: string;
  description: string;
  when_to_use: string | null;
  version: string | null;
  path: string;
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
} {
  const [skills, setSkills] = useState<SkillEntry[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
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
    // ship different skill folders.
  }, [projectRoot]);

  return { skills, loading, error };
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

export function SkillsSurface() {
  const { skills, loading, error } = useSkills();
  const projectRoot = useProjectStore((s) => s.current);
  const disabledByProject = useSkillsStore((s) => s.disabledByProject);
  const toggle = useSkillsStore((s) => s.toggle);

  // Keep selection alive across rerenders. Default to the first
  // skill on first load so the right pane is never empty when there
  // are skills to show.
  const [selected, setSelected] = useState<string | null>(null);
  useEffect(() => {
    if (selected && skills.some((s) => s.name === selected)) return;
    setSelected(skills[0]?.name ?? null);
  }, [skills, selected]);

  // Reactive disable lookup. We pull from the map directly so the
  // component re-renders when the user toggles a skill.
  const isDisabled = useMemo(() => {
    const set = disabledByProject.get(projectRoot ?? "__global__");
    return (name: string) => set?.has(name) ?? false;
  }, [disabledByProject, projectRoot]);

  return (
    <div className="grid h-full min-h-0 grid-cols-[2fr_3fr] gap-2 p-3">
      <SkillList
        skills={skills}
        loading={loading}
        error={error}
        selected={selected}
        onSelect={setSelected}
        isDisabled={isDisabled}
        onToggle={(name) => toggle(projectRoot, name)}
      />
      <SkillDetail
        skill={skills.find((s) => s.name === selected) ?? null}
        disabled={selected ? isDisabled(selected) : false}
      />
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
  onToggle: (name: string) => void;
};

function SkillList({
  skills,
  loading,
  error,
  selected,
  onSelect,
  isDisabled,
  onToggle,
}: SkillListProps) {
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
                  onSelect={() => onSelect(skill.name)}
                  onToggle={() => onToggle(skill.name)}
                />
              </li>
            ))}
          </ul>
        )}
      </div>
    </section>
  );
}

type SkillCardProps = {
  skill: SkillEntry;
  selected: boolean;
  disabled: boolean;
  onSelect: () => void;
  onToggle: () => void;
};

function SkillCard({
  skill,
  selected,
  disabled,
  onSelect,
  onToggle,
}: SkillCardProps) {
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
            <div className="flex items-center gap-2">
              <span className="truncate text-[12px] font-semibold text-[var(--color-text-primary)]">
                {skill.display_name}
              </span>
              {skill.version && (
                <span className="shrink-0 rounded-full bg-[var(--color-surface-input)] px-1.5 py-0.5 font-mono text-[10px] text-[var(--color-text-muted)]">
                  v{skill.version}
                </span>
              )}
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
      <div className="mt-2 flex items-center justify-end">
        <Switch
          checked={!disabled}
          onChange={onToggle}
          ariaLabel={`${disabled ? "Enable" : "Disable"} ${skill.display_name}`}
        />
      </div>
    </div>
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
};

function SkillDetail({ skill, disabled }: SkillDetailProps) {
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
          <div className="flex items-center gap-2">
            <h2 className="truncate text-[14px] font-semibold text-[var(--color-text-primary)]">
              {skill.display_name}
            </h2>
            {skill.version && (
              <span className="shrink-0 rounded-full bg-[var(--color-surface-input)] px-1.5 py-0.5 font-mono text-[10px] text-[var(--color-text-muted)]">
                v{skill.version}
              </span>
            )}
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
