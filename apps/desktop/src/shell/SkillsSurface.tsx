/** Skills available to the agent for this project. */

import { useEffect, useMemo, useState } from "react";
import { invoke, isTauri } from "@tauri-apps/api/core";
import { Sparkles } from "lucide-react";

import { deferNonCriticalHydration } from "../app/startupHydration";
import { cn } from "../ui";
import { useProjectStore } from "../app/state";
import { useSkillsStore } from "../state";

/**
 * Mirror of the Rust `SkillEntry` returned by `list_skills`.
 *
 * `provenance` identifies which discovery layer the skill came from:
 *   - "bundled" → ships with Montage (lowest priority)
 *   - "user"    → ~/Library/Application Support/montage/skills (etc.)
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
    const loadSkills = () => invoke<SkillEntry[]>("list_skills")
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
    const cancelDeferredLoad = deferNonCriticalHydration(() => {
      void loadSkills();
    });
    return () => {
      cancelled = true;
      cancelDeferredLoad();
    };
  }, [projectRoot]);

  return { skills, loading, error };
}

export function SkillsSurface() {
  const { skills, loading, error } = useSkills();
  const projectRoot = useProjectStore((s) => s.current);
  const disabledByProject = useSkillsStore((s) => s.disabledByProject);
  const toggle = useSkillsStore((s) => s.toggle);
  const disabled = disabledByProject.get(projectRoot ?? "__global__");
  return <SkillsSheet skills={skills} loading={loading} error={error}
    isDisabled={(name) => disabled?.has(name) ?? false}
    onToggle={(name) => toggle(projectRoot, name)} />;
}


type SkillsSheetProps = {
  skills: SkillEntry[];
  loading: boolean;
  error: string | null;
  isDisabled: (name: string) => boolean;
  onToggle: (name: string) => void;
};

function SkillsSheet({
  skills,
  loading,
  error,
  isDisabled,
  onToggle,
}: SkillsSheetProps) {
  const [expanded, setExpanded] = useState<string | null>(null);
  const enabledCount = useMemo(
    () => skills.filter((s) => !isDisabled(s.name)).length,
    [skills, isDisabled],
  );

  return (
    <div className="flex h-full min-h-0 flex-col gap-4 overflow-y-auto p-6">
      <header className="flex shrink-0 items-baseline justify-between gap-3">
        <div className="flex items-center gap-2.5">
          <Sparkles
            className="h-5 w-5 text-[#FCA5A5]"
            strokeWidth={1.75}
            aria-hidden
          />
          <h1 className="text-[18px] font-semibold text-[var(--color-text-primary)]">
            Skills{" "}
            <span className="text-[var(--color-text-muted)]">
              · the agent&apos;s loadout
            </span>
          </h1>
        </div>
        <span className="shrink-0 font-mono text-[12px] text-[var(--color-text-secondary)]">
          {error || loading ? (
            <span className="text-[var(--color-text-muted)]">
              {error ? "—" : "…"}
            </span>
          ) : (
            <>
              {enabledCount}
              <span className="text-[var(--color-text-muted)]">
                /{skills.length} on
              </span>
            </>
          )}
        </span>
      </header>

      {error ? (
        <div className="flex flex-1 items-center justify-center text-center text-[13px] text-[var(--color-status-error)]">
          <span>{error}</span>
        </div>
      ) : loading ? (
        <div className="flex flex-1 items-center justify-center text-[13px] text-[var(--color-text-muted)]">
          <span>Loading skills…</span>
        </div>
      ) : skills.length === 0 ? (
        <div className="flex flex-1 items-center justify-center px-8 text-center text-[13px] leading-relaxed text-[var(--color-text-muted)]">
          <span>
            No skills discovered yet. Add{" "}
            <code className="rounded bg-[var(--color-surface-input)] px-1 font-mono">
              SKILL.md
            </code>{" "}
            files under a{" "}
            <code className="rounded bg-[var(--color-surface-input)] px-1 font-mono">
              skills/
            </code>{" "}
            folder in your project or home directory.
          </span>
        </div>
      ) : (
        <div className="grid grid-cols-1 gap-3 md:grid-cols-2">
          {skills.map((skill) => (
            <SkillSheetCard
              key={skill.name}
              skill={skill}
              disabled={isDisabled(skill.name)}
              expanded={expanded === skill.name}
              onToggleExpand={() =>
                setExpanded((cur) => (cur === skill.name ? null : skill.name))
              }
              onToggle={() => onToggle(skill.name)}
            />
          ))}
        </div>
      )}
    </div>
  );
}

type SkillSheetCardProps = {
  skill: SkillEntry;
  disabled: boolean;
  expanded: boolean;
  onToggleExpand: () => void;
  onToggle: () => void;
};

function SkillSheetCard({
  skill,
  disabled,
  expanded,
  onToggleExpand,
  onToggle,
}: SkillSheetCardProps) {
  return (
    <div
      className={cn(
        "glass-content flex flex-col gap-2.5 p-4 transition-opacity",
        disabled && "opacity-55",
      )}
    >
      <div className="flex items-start justify-between gap-3">
        <div className="flex min-w-0 flex-1 flex-col gap-1.5">
          <span className="truncate text-[14px] font-bold text-[var(--color-text-primary)]">
            {skill.display_name}
          </span>
          <div className="flex flex-wrap items-center gap-1.5">
            <SheetProvenanceChip provenance={skill.provenance} />
            {skill.version && (
              <span className="shrink-0 rounded-full bg-[var(--color-surface-input)] px-1.5 py-0.5 font-mono text-[10px] text-[var(--color-text-muted)]">
                v{skill.version}
              </span>
            )}
          </div>
        </div>
        {/* enable/disable pip — orange when on */}
        <button
          type="button"
          role="switch"
          aria-checked={!disabled}
          aria-label={`${disabled ? "Enable" : "Disable"} ${skill.display_name}`}
          onClick={onToggle}
          title={disabled ? "Enable skill" : "Disable skill"}
          className={cn(
            "mt-0.5 h-3 w-3 shrink-0 rounded-full border transition-all",
            disabled
              ? "border-[var(--glass-border)] bg-transparent hover:border-[var(--glass-border-strong)]"
              : "border-[var(--color-brand)] bg-[var(--color-brand)] shadow-[0_0_10px_rgba(239,68,68,0.55)]",
          )}
        />
      </div>

      <p className="line-clamp-2 text-[12px] leading-snug text-[var(--color-text-secondary)]">
        {skill.description}
      </p>

      {skill.when_to_use && (
        <>
          <button
            type="button"
            onClick={onToggleExpand}
            aria-expanded={expanded}
            className="self-start text-[11px] font-medium text-[#FCA5A5] transition-opacity hover:opacity-80"
          >
            {expanded ? "Hide details" : "When to use →"}
          </button>
          {expanded && (
            <p className="rounded-[10px] border-l-2 border-[#FCA5A5] bg-[var(--color-surface-input)] px-2.5 py-2 text-[11px] leading-relaxed text-[var(--color-text-secondary)]">
              {skill.when_to_use}
            </p>
          )}
        </>
      )}
    </div>
  );
}

/**
 * Provenance chip using the workspace color vocabulary
 * (user = cyan accent, project = orange brand, bundled = neutral frost).
 */
function SheetProvenanceChip({
  provenance,
}: {
  provenance: SkillProvenance;
}) {
  const palette: Record<
    SkillProvenance,
    { label: string; className: string; title: string }
  > = {
    bundled: {
      label: "bundled",
      className:
        "border-[var(--glass-border)] bg-[var(--color-surface-input)] text-[var(--color-text-muted)]",
      title: "Ships with Montage",
    },
    user: {
      label: "user",
      className:
        "border-[#FCA5A5]/40 bg-[#FCA5A5]/12 text-[#FCA5A5]",
      title: "Installed from your per-user skills folder",
    },
    project: {
      label: "project",
      className:
        "border-[color:var(--color-brand)]/45 bg-[color:var(--color-brand)]/15 text-[var(--color-brand)]",
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
