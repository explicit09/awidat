# Skills authoring guide

Skills are named, prompt-engineered editorial workflows the agent loads
on demand. This guide tells you how to write one.

## What a skill is

A **skill** is a folder containing a `SKILL.md` file:

```
skills/
  auto-cutter/
    SKILL.md           # required — frontmatter + playbook body
    scripts/           # optional — helpers invoked via `bash`
```

`SKILL.md` has two halves: a **YAML frontmatter block** (metadata) and a
**Markdown body** (the prompt the agent reads at L2 when this skill is
loaded).

The frontmatter is the L1 catalog entry — the one-line summary the
agent sees on every turn. The body is the L2 playbook — only loaded
when the agent calls `load_skill(name)`.

Folders prefixed with `_` (e.g. `_template/`) are ignored by discovery
on both the agent loader and the Skills tab. Use the prefix for
scaffolds, archives, and work-in-progress.

## Frontmatter reference

```yaml
---
name: my-skill                 # required, kebab-case, must match dir name
description: One line describing what this skill does.  # required
version: "0.1.0"               # recommended; bumped on behavior changes
montage_min_version: "0.1.0"    # optional compat gate; skipped if too old
tools_allowlist:               # optional — restricts agent's tools
  - apply_edl
  - view_timeline
tier: editorial                # optional grouping tag (free-form)
when_to_use: |                 # optional — when should the agent reach
  Activate when the user...     # for this skill? Surfaced in Skills tab.
---
```

- **`name`** — directory name and L1 id. Loader rejects mismatches.
- **`description`** — one sentence; lands in the per-turn skills
  fragment as `- <name>: <description>`.
- **`version`** — semver string. Defaults to `"0.1.0"` if omitted.
  Bump it whenever the agent's behavior would notably change for users
  pinned to an older version.
- **`montage_min_version`** — minimum Montage core required. Older hosts
  skip the skill with a warning rather than loading a broken playbook.
- **`tools_allowlist`** — when present, tools outside this list are
  **hard-rejected** after `load_skill` (state in
  `.montage/active_skill.json`). Meta tools `load_skill`,
  `load_project_instructions`, `attempt_completion`, `update_plan`,
  `request_user_input`, and `set_picture_lock` always remain available.
  Empty or missing = no restriction. Escape hatch:
  `MONTAGE_DISABLE_SKILL_ALLOWLIST=1`.
- **`when_to_use`** — free-text trigger hint shown in the Skills tab
  detail pane. Use it to disambiguate skills with overlapping scope.

## The rationale contract

Every proposal the agent emits via `apply_edl` (and friends) **MUST**
carry a one-sentence `rationale` (passed as the `reasoning:` parameter).
Empty rationales fail editorial review. The contract is hard-wired into
the always-on L1 fragment — see
[`crates/core/src/context.rs::RATIONALE_CONTRACT`](../crates/core/src/context.rs)
for the exact text the agent reads every turn.

Example proposal payload:

```jsonc
{
  "tool": "apply_edl",
  "ops": [
    { "kind": "Trim Clip", "clip_uuid": "…", "trim_in_ms": 420 }
  ],
  "reasoning":
    "Trimmed 0.42s — silence > 300ms exceeded the podcast-cleanup threshold from AGENTS.md."
}
```

Rules to encode in your skill body:

- Reference the threshold or principle that justified the edit
  (AGENTS.md, an indexer signal, an editorial rule from your skill).
- Keep it under ~120 characters — the user sees it on one line in the
  Brief, History, Inspector, and on timeline ghost-clips.
- Don't restate the action ("Trimmed 0.42s" is the title; rationale is
  *why*, not *what*).
- For B-roll insertions, name the prompt + provider for disclosure.
- For color/audio, name the audited measurement (LUFS, ΔE, etc.).

## Tool access

Skills don't carry their own tool implementations — they call the same
tools every other agent turn does. Common ones:

| Skill goal                       | Tool                  |
| -------------------------------- | --------------------- |
| Apply cut / trim / insert        | `apply_edl`           |
| Propose edits the user must OK   | `propose_user_edit`   |
| Insert generated B-roll          | `use_generated_media` |
| Inspect transcript / audio       | `view_episode`, `view_timeline`, `inspect_clip` |
| Detect cleanup candidates        | `find_dead_air`, `find_filler_words`, `find_false_starts` |
| Quality gate before render       | `assess_edit_quality`, `vedit_diff` |
| Render                            | `start_render`, `poll_render` |
| Shell-out to bundled scripts     | `bash`                |

Use `tools_allowlist` in frontmatter to lock the agent to just the
tools your playbook actually needs.

## Layered discovery

Skills are discovered from three layers, lowest priority first:

1. **Bundled** — `<install>/share/montage/skills/` or, in dev,
   `<repo>/skills/`. Ships with Montage.
2. **User** — `~/Library/Application Support/montage/skills/` (macOS),
   `~/.config/montage/skills/` (Linux), `%APPDATA%\montage\skills\`
   (Windows). Personal overrides + additions.
3. **Project** — `<project>/skills/`. Per-project workflows; trumps
   user + bundled on name conflict.

A skill at a higher layer **replaces** the lower-layer entry of the
same `name` wholesale (not a field-merge). The Skills tab shows the
resolved provenance via a chip on each row.

## Version pinning

Projects can lock to a specific skill version (and optionally a
specific provenance layer) by setting a pin in
`<project>/.montage/skills.json`. The Skills tab exposes this through
the "Pin v1.0.0" affordance on any skill at version ≥ 1.0.0. Pinned
skills resolve deterministically across teammates and CI.

A pin shape:

```json
{
  "pinned": {
    "<project>": [
      { "name": "auto-cutter", "version": "1.2.0", "provenance": "user" }
    ]
  }
}
```

`provenance` is optional — omit to accept the version from any layer.

## A complete example

The bundled `auto-cutter` skill ships at
[`skills/auto-cutter/SKILL.md`](../skills/auto-cutter/SKILL.md).
Frontmatter declares the L1 entry and locks the tool surface; the body
walks the agent through a four-step workflow with hard rules and a
"done when" checklist:

```yaml
---
name: auto-cutter                              # matches directory
description: Extract the real episode...       # one-line catalog blurb
version: 0.1.0
tier: editorial                                # grouping tag
tools_allowlist:                               # narrowed tool surface
  - view_episode
  - find_episode_start
  - apply_edl
  - bash
  # ... see file for full list
---

# Auto-cutter                                  # H1 = playbook title

Use this skill when the user wants a one-pass cleanup ...

## Workflow                                    # numbered steps the
### 1. Identify the publishable episode        # agent follows in order
...

## Rules                                       # hard constraints
- Never use energy alone ...

## Done when                                   # exit checklist
- The episode start was chosen with ...
```

Copy this shape. Replace the name + description, narrow the
`tools_allowlist` to what you actually call, and write the body as
prose the agent will execute.

## Distribution

Today: share skills by copying the folder. Drop it under your project's
`skills/` to scope it to that project, or under the user skills dir to
make it available everywhere on your machine.

A registry / package format is on the roadmap — not yet shipped. Until
then, version your skills with `version:` and treat the folder as the
unit of distribution.

## Scaffolding a new skill

The Skills tab in Montage has a **+ New skill** button that copies the
`skills/_template/SKILL.md` skeleton into the chosen location (user or
project) with the name + description substituted. Use it for the
boilerplate; edit the body to taste.
