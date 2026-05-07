---
name: version-control
description: Commit, diff, and audit timeline edits using vedit. Loaded when the user asks about history, change tracking, "what did you do?", "save this version", or wants to audit what's changed.
version: 0.1.0
tier: editorial
tools_allowlist:
  - vedit_commit
  - vedit_diff
  - vedit_log
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
- "What did the agent do at <time>?"
- "Why did we cut this clip?"

## What's already happening (Phase A)

- A vedit repo lives at `<project>/.vedit/`. It's created
  automatically when needed.
- Every session, awidat stamps a `session-start` branch at
  whatever HEAD is — so the question "what did you change THIS
  session?" has a stable answer: `vedit_diff` (default args).
- Commits are user-triggered today. Phase B will auto-commit on
  every accepted apply_edl envelope; until then, you commit when
  the user asks (or when you've made a substantial change worth
  saving).

## The 3-tool surface

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

### `vedit_log(limit?)` — list recent commits

Default 30, hard cap 200. Each entry: header, full message, timestamp,
hashes, parents. Use this for "what's been going on lately" / "show me
history."

## Editorial conventions

- **Commit cadence (Phase A)**: per substantial change. A typo fix in
  the agent reasoning text is NOT a commit. A trimmed clip + bundled
  b-roll cover IS. When unsure: ask the user "should I commit this as
  a checkpoint?"
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
3. Tell the user the commit hash so they can refer back
```

### "Show me the history"

```
1. vedit_log(limit=10) for a quick overview
2. Render entries as: "<short hash> · <timestamp> · <header>"
3. If a specific commit is interesting, the user can ask about it by
   short hash and you fetch the full message
```

### "What's different between commit A and now?"

```
1. vedit_diff(from=<commit A's short hash>)
2. Same prose-rendering as session diff
```

## What NOT to do

- **Don't commit metadata-only changes** (e.g. you updated agent
  reasoning text but no clip moved) without telling the user. The
  structural diff will be empty even though the hash differs;
  `vedit_diff` flags this. Either skip the commit or call out
  "metadata-only checkpoint" in the header.
- **Don't pull every commit's full body**. Default `vedit_log` returns
  enough; deep-dive only when the user asks about a specific commit.
- **Don't try to merge or branch in Phase A.** Those tools aren't
  exposed yet. If the user asks "let's try this on a branch", tell
  them branching lands in Phase B and offer to commit the current
  state as a checkpoint they can return to manually.
- **Don't auto-commit on every turn.** Phase A is user-triggered. The
  apply pipeline doesn't auto-commit yet (that's Phase B), so each
  `vedit_commit` call is a deliberate save-point.

## You are done when...

- [ ] The user's question about history was answered with
      structured `vedit_log` / `vedit_diff` data, not vague
      "I think I trimmed something."
- [ ] If the user asked you to commit, the commit landed and you
      returned the short hash so they can refer to it later.
- [ ] If the diff was empty but a commit exists, you said so
      clearly ("the structural diff is empty — only metadata
      changed, e.g. agent reasoning text").
- [ ] Hashes are quoted in their short form (first 7 hex chars) for
      readability, unless the user asks for full.

## Phase B preview (not yet active)

- **Auto-commit on every accepted envelope.** Every apply_edl that
  the user accepts becomes a commit. The reasoning body comes from
  the turn's reasoning verbatim. You won't need to call
  `vedit_commit` manually then — it'll just happen.
- **Branch + switch tools.** The agent will be able to propose
  alternatives on a branch ("let me try a tighter cut on `alt-tight`
  and show you both"). User reviews, picks, optionally merges.
- **Diff view in the desktop UI.** No CLI roundtrip needed for the
  user; the diff renders in the timeline pane.

Phase A is the substrate; Phase B is when this becomes load-bearing.
