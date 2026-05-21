---
name: version-control
description: Commit, diff, and audit timeline edits using vedit. Loaded when the user asks about history, change tracking, "what did you do?", "save this version", or wants to audit what's changed.
version: 0.1.0
tier: editorial
tools_allowlist:
  - vedit_commit
  - vedit_blame
  - vedit_branch
  - vedit_changed_clip_ids
  - vedit_checkout
  - vedit_diff
  - vedit_log
  - vedit_merge_preflight
  - vedit_show
  - vedit_tag
  - view_timeline
---

# Version control (vedit)

awidat persists timeline history via vedit — a content-addressed
commit graph for OTIO timelines, alongside the project at
`<project>/.vedit/`. Every commit captures the timeline plus the
agent's reasoning at that point in time. This is the audit trail
that lets the user trust the agent's editorial decisions over time.

## When to use this skill

Load this skill when the user asks any of:

- "What did you change in this session?"
- "Save this version" / "commit this" / "snapshot"
- "Show me the edit history"
- "Roll back to ..."
- "Try this on an alternate branch"
- "What did the agent do at <time>?"
- "Why did we cut this clip?"

## What's already happening (Phase A + Phase B)

- A vedit repo lives at `<project>/.vedit/`. It's created
  automatically when needed.
- Every session, awidat stamps a `session-start` branch at
  whatever HEAD is — so the question "what did you change THIS
  session?" has a stable answer: `vedit_diff` (default args).
- **Every accepted `apply_edl` envelope auto-commits.** The header
  is auto-generated from the structured op descriptions; one op
  becomes the verbatim description, two ops join with `;`, three+
  ops become "X; Y; …and N more" capped at 120 chars.
- **Pass `reasoning: "..."` to `apply_edl` whenever you have context.**
  It lands in the auto-commit body as `Agent reasoning: <text>`. One
  short sentence is enough — reference rules ("per rhythm-preservation
  rule"), user requests ("user asked for tighter pacing"), or trigger
  findings ("matched find_broll_opportunities at 12.4s"). The header
  is the *what*; the reasoning is the *why*. Together they're the
  audit trail for "why did we cut this?" reads on the commit log.
- `vedit_commit` remains available for **explicit save points** —
  e.g. when the user says "snapshot this" without making a structural
  edit, or when you want to land a commit message richer than what
  the auto-header produces. Use it sparingly.

## The tool surface

### `vedit_commit(header, reasoning?)` — save the current state

```
vedit_commit(
  header="Trim drone_shot_04 -1.8s; insert b-roll cover at skyline",
  reasoning="User asked for tighter pacing. Trimmed the drone hold per
   rhythm-preservation rule. Bundled a 3.0s Pexels skyline cover at
   12.4s because the speaker referenced 'imagine a city skyline' — the
   cutaway hides the otherwise-dirty mid-motion trim point."
)
```

The header is canonical: short, imperative, no trailing period. Same
convention as good git commit messages. The reasoning body is your
audit trail — it's what the user will read in 3 weeks when they ask
"why did we cut this?"

### `vedit_diff(from?, to?)` — see structured changes

Default: `from=session-start`, `to=HEAD`. Returns the structured list
of OTIO operations between the two refs:

- `TrackAdded` / `TrackRemoved`
- `Added` / `Removed` / `Moved` (clips)
- `Trimmed` (range narrowed/shifted)
- `EffectsChanged` (effect count delta)
- `Replaced` (same slot, different media)
- `TransitionAdded` / `TransitionRemoved`

Render as English prose for the user: "Trimmed drone_shot_04 by 1.8s
(in)", "Inserted skyline_dusk.mp4 between interview-take-2 and
b_roll_03", etc.

### `vedit_changed_clip_ids(from?, to?)` — list touched clip/media ids

Default: `from=session-start`, `to=HEAD`. Returns sorted clip names,
media references, and clip animation targets touched by the diff, plus
structural and animation change counts.

Use this for read-only review or preflight overlap checks between refs.
It does not checkout, merge, or mutate any ref. Until bounded merge is
approved, use it only to explain potential overlap; do not perform an
automated merge.

### `vedit_merge_preflight(source, target?)` — check bounded merge safety

Default target: `HEAD`. Returns the source/target commits, their common
ancestor, each side's changed clip/media ids, overlap ids, change
counts, and `is_mergeable`.

Use this before discussing a branch merge. It enforces the proposed
bounded rule as a read-only report: if overlap ids are present, the
merge would require human/manual resolution. If no overlap ids are
present, the refs are compatible with a future non-overlapping merge
path, but this tool still does not merge or move refs.

### `vedit_log(limit?)` — list recent commits

Default 30, hard cap 200. Each entry: header, full message, timestamp,
hashes, parents. Use this for "what's been going on lately" / "show me
history."

### `vedit_tag(name?, refstr?, list?)` — name a checkpoint

Use tags for human-stable checkpoint names: `client-review-v1`,
`before-tightening-pass`, `shown-to-sarah`. Tags point at commits and
live under `.vedit/refs/tags/`. They do not switch HEAD, create a
branch, or merge anything.

Call with `list=true` to list existing tags. Call with `name` and
optional `refstr` to create/update a tag; `refstr` defaults to `HEAD`.

### `vedit_show(refstr)` — deep-dive one commit

Use this after `vedit_log` when the user asks "what exactly happened
in that commit?" It returns the commit hashes, full message, parents,
and the semantic diff from the first parent to that commit. Initial
commits diff against an empty timeline.

### `vedit_blame(clip_id, start_ref?, limit?)` — why did this clip change?

Use this when the user asks "who/why touched this clip?" or "when did
this clip get trimmed?" It walks first-parent history from `HEAD` (or
`start_ref`) and returns commits whose semantic diff touches the clip
name, media reference, or animation target string.

This is attribution, not full git-style line blame. It projects commit
reasoning and semantic OTIO ops onto a clip so the user can see the
most relevant history without reading the whole log.

### `vedit_branch(name?, start_ref?, list?)` — create/list alternates

Use branches for alternate local cuts: `alt-tight`,
`client-version-b`, `try-cold-open`. Branches point at commits and
live under `.vedit/refs/heads/`. Creating a branch does not switch the
working timeline; it just creates the alternate ref.

Call with `list=true` to list existing branches and see which one is
current. Call with `name` and optional `start_ref` to create a branch;
`start_ref` defaults to `HEAD`.

### `vedit_checkout(branch)` — switch to an alternate

Use this only when the user explicitly wants to work on an existing
branch. It switches HEAD to the branch and restores `project.otio.json`
to that branch's committed timeline snapshot. It does not merge and it
does not create an audit commit by itself.

## Editorial conventions

- **Commit cadence**: accepted timeline mutations auto-commit. Use
  `vedit_commit` only for explicit save points or metadata-only
  checkpoints the user asked for.
- **Header format**: imperative, present tense. "Trim X by 1.8s",
  not "Trimmed X by 1.8s" or "X was trimmed by 1.8s." Same rule as
  good git.
- **Reasoning is mandatory for non-trivial commits**. The header
  alone tells you WHAT changed; the reasoning tells you WHY. The
  WHY is what the audit trail is for.
- **Reference rules and sources in the reasoning**: "per
  rhythm-preservation rule", "user explicitly asked for", "matched
  find_broll_opportunities trigger". The agent's editorial choices
  should be defensible from the commit log alone.

## Common patterns

### "What did you change this session?"

```
1. vedit_diff() with no args
2. Render the structured changes as English prose
3. Reference specific clips by name + media
```

### "Save this version"

```
1. Confirm WHAT to call this checkpoint
2. vedit_commit(header=<chosen header>, reasoning=<context from this session>)
3. Optionally vedit_tag(name=<stable name>) if the user gave a
   human-friendly label
4. Tell the user the short commit hash and tag, if created
```

### "Show me the history"

```
1. vedit_log(limit=10) for a quick overview
2. Render entries as: "<short hash> · <timestamp> · <header>"
3. If a specific commit is interesting, call vedit_show(refstr=<hash>)
   and render the parent..commit diff
```

### "Why did we cut this clip?"

```
1. vedit_blame(clip_id=<clip name from view_timeline>)
2. Render matching commits newest-first:
   "<short hash> · <header> · <reasoning summary>"
3. Include the structural change that matched, e.g. Trimmed / Moved /
   EffectsChanged
```

### "What's different between commit A and now?"

```
1. vedit_diff(from=<commit A's short hash>)
2. vedit_changed_clip_ids(from=<commit A's short hash>) if the user
   asks which clips/media were touched
3. Same prose-rendering as session diff
```

### "Try a tighter cut on a branch"

```
1. vedit_branch(name="alt-tight")
2. vedit_checkout(branch="alt-tight")
3. Make the requested edit through apply_edl so it auto-commits
4. vedit_diff(from=<original branch/tag>, to="HEAD") to explain the alternate
5. vedit_changed_clip_ids(from=<original branch/tag>, to="HEAD") if you need
   overlap preflight data for a later human merge decision
6. vedit_merge_preflight(source="alt-tight", target=<original branch/tag>)
   to get the common ancestor and overlap report without merging
```

## What NOT to do

- **Don't commit metadata-only changes** (e.g. you updated agent
  reasoning text but no clip moved) without telling the user. The
  structural diff will be empty even though the hash differs;
  `vedit_diff` flags this. Either skip the commit or call out
  "metadata-only checkpoint" in the header.
- **Don't pull every commit's full body**. Default `vedit_log` returns
  enough; deep-dive only when the user asks about a specific commit.
- **Don't merge branches yet.** Branch and checkout tools are local
  alternates only, and `vedit_merge_preflight` is read-only. Bounded
  merge execution is still roadmap work until the product rule is
  accepted: merge only when changed clip ids do not overlap; otherwise
  return a conflict for human choice.
- **Don't call `vedit_commit` for every turn.** The apply pipeline
  already auto-commits accepted edit envelopes. Manual commits are
  deliberate save points.

## You are done when...

- [ ] The user's question about history was answered with structured
      `vedit_log` / `vedit_show` / `vedit_diff` / `vedit_changed_clip_ids` /
      `vedit_blame` data, not vague "I think I trimmed something."
- [ ] If the user asked you to commit, the commit landed and you
      returned the short hash so they can refer to it later.
- [ ] If the diff was empty but a commit exists, you said so
      clearly ("the structural diff is empty — only metadata
      changed, e.g. agent reasoning text").
- [ ] Hashes are quoted in their short form (first 7 hex chars) for
      readability, unless the user asks for full.

## Still on the roadmap

- **Bounded merge execution.** Branches and read-only merge preflight
  exist, but automated merge is still deferred. The safe rule is
  non-overlapping changed clip ids only; overlapping edits must prompt
  the user.
- **Diff view in the desktop UI.** No CLI roundtrip needed for the
  user; the diff renders in the timeline pane.

The auto-commit substrate is load-bearing now; the rest is polish on
top of it.
