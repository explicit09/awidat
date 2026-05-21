# Category 11 Collaboration Backlog

## Source

This backlog is derived from:

- `/Users/explicit/Projects/awidat/.reference-research/pro-editing-gap-analysis/11-collaboration.md`
- `/Users/explicit/Projects/awidat/.reference-research/pro-editing-gap-analysis/11-collaboration.html`
- Current Awidat code in this worktree.

The gap analysis splits collaboration into versioning, frame-specific
notes, review/share surfaces, multi-user awareness, and cloud sync. The
current worktree addresses the first four suggested next moves in the
local-first versioning slice: named checkpoints, single-commit show,
per-clip blame projection, and branch alternates with checkout. Full
workspace verification passes in this worktree. It also includes the
changed-clip-id extraction primitive needed by future bounded merge
planning and exposes it through a read-only agent tool plus a desktop
command, desktop diff responses, and a read-only merge preflight that
reports overlap/conflict state without merging. Merge execution itself
remains blocked on product approval.

## Updated Category 11 Row

`Collaboration | vedit substrate is ahead of sampled OSS NLE peers:
content-addressed timeline commits, semantic OTIO diff, auto-commit on
apply, session-start audit, named checkpoint tags, local branch
alternates with checkout, single-commit show, per-clip blame
projection, read-only bounded merge preflight, and agent reasoning in
commit bodies. Missing bounded merge execution, bisect, authored
marker/comment model, threaded comments, review links, third-party
comment ingest, multi-user awareness, and cloud sync.`

## Phase Map

### Phase 1: Local vedit attribution

Status: implemented and verified in this worktree.

Scope:

- Add `vedit_tag` and desktop tag commands.
- Add `vedit_show` and desktop commit-show command.
- Add `vedit_blame` and desktop blame command.
- Correct version-control docs so auto-commit on accepted applies is
  described as current behavior, not future work.

Evidence gates:

- `cargo fmt --all -- --check`
- `cargo test -p awidat-core vc::`
- `cargo test -p awidat-core vedit_`
- `git diff --check`
- `make check`

### Phase 2: Branches and bounded merge

Status: branch/create/list/checkout and changed-clip-id extraction are
implemented in this worktree. Changed-clip ids are also available
through a read-only agent tool, desktop command, and desktop diff
responses. Read-only merge preflight reports whether two refs overlap
under the proposed bounded rule. Bounded merge execution remains
blocked on product approval and missing upstream merge semantics.

Purpose:

- Let an agent create an alternate cut without overwriting the current
  timeline.
- Let users compare refs and merge only when changed clip identities do
  not overlap.
- Refuse or escalate conflicting merges instead of pretending full OTIO
  three-way merge is solved.

Implemented tool/API surface:

- `vedit_branch(list?, name?, start_ref?)`
- `vedit_checkout(refstr)`
- `vedit_changed_clip_ids(from?, to?)`
- `vedit_merge_preflight(source, target?)`
- `changed_vedit_clip_ids(from_ref?, to_ref?)` desktop command
- `preflight_vedit_merge(source_ref, target_ref?)` desktop command
- `diff_vedit_refs(from_ref?, to_ref?)` desktop response includes
  `changedClipIds` and `changedClipCount`
- `vc::changed_clip_ids(&CommittedDiff)` as shared conflict-detection
  input for future merge planning.
- `vc::merge_preflight(repo, source_ref, target_ref?)` as shared
  common-ancestor and overlap reporting input for future merge
  execution.

Remaining proposed tool/API surface:

- `vedit_merge(source_ref, target_ref?, strategy="non_overlapping_clips")`

Non-goals:

- No full semantic three-way OTIO merge.
- No automatic conflict resolution on overlapping clip ids.
- No remote branch sync.

Evidence gates:

- Unit tests for branch create/list/checkout wrappers.
- Unit tests for changed clip id extraction from structural and
  animation diffs.
- Tool tests for read-only changed clip id reporting.
- Desktop command response tests for changed clip id reporting.
- Desktop diff response tests for changed clip id reporting.
- Core tests for non-overlapping merge preflight.
- Tool and desktop response tests for overlapping merge preflight.
- Tool tests for approval keys and refusal output.

### Phase 3: User-authored notes and richer anchors

Status: proposed, requires product confirmation before schema migration.

Purpose:

- Convert notes from agent-only findings into a durable review-note
  substrate that can later receive human and third-party comments.
- Preserve existing `EditorialNote` behavior for agent-generated notes.

Minimal model surface:

- Add an author shape with stable type and display name.
- Add an anchor shape:
  - point timecode
  - time range
  - clip id
  - clip-relative range
- Keep the current `anchor_at_s` compatibility path during migration.
- Keep dismissal-pattern memory agent-only.

Open product decision:

- Whether agent-authored editorial findings and user-authored review
  comments share one `notes.json` schema or split into separate stores.

Evidence gates:

- Backward-compatibility tests loading current `notes.json`.
- Migration tests preserving existing open/resolved/dismissed notes.
- Desktop store tests for old and new note shapes.
- UI smoke check that existing NotesPanel behavior survives.

### Phase 4: Desktop diff/review view

Status: proposed.

Purpose:

- Present `vedit_diff`, `vedit_show`, and `vedit_blame` as editor-facing
  change views instead of chat JSON.

Minimal surface:

- Diff list grouped by structural changes and animation changes.
- Commit detail view using `show_vedit_commit`.
- Clip history drawer using `blame_vedit_clip`.
- Tag affordance for "shown to reviewer" checkpoints.

Non-goals:

- No remote review page.
- No live multi-user presence.

Evidence gates:

- Desktop type checks.
- UI harness updates for new command responses.
- Browser screenshot or harness verification once the local app can run.

### Phase 5: Render-to-review handoff

Status: proposed, can land before third-party integrations.

Purpose:

- Create a local review package that ties a render artifact back to the
  vedit commit, agent reasoning, and current tags.

Minimal surface:

- Review proxy profile for timeline render.
- Manifest under `renders/` with:
  - render artifact path
  - vedit commit hash
  - timeline hash
  - selected tag names
  - commit header and reasoning body
  - generated-at timestamp
- Optional desktop command to reveal/copy the package path.

Non-goals:

- No hosted URL.
- No Frame.io/Vimeo/Wipster API call.
- No credentials stored by Awidat.

Evidence gates:

- Manifest serialization tests.
- Render command tests that do not require full media rendering where
  possible.
- Manual ffmpeg-backed smoke only when disk and fixtures allow it.

### Phase 6: Third-party comment ingest

Status: deferred until Phase 3 note schema exists and credentials are
available.

Purpose:

- Import Frame.io, Vimeo Review, Wipster, or iconik comments as authored
  review notes anchored to timecode.

Blockers:

- Provider choice.
- API credentials or webhook payload fixtures.
- Finalized internal authored-note schema.

Evidence gates:

- Fixture-driven parser tests per provider.
- No live network dependency in default tests.
- Explicit opt-in for credentialed integration smoke.

### Phase 7: Multi-user awareness and cloud sync

Status: out of current local-first implementation scope.

Reason:

- The gap analysis identifies presence, locks, conflict prompts, and
  cloud sync as architectural work that likely requires a sync server or
  a repository-level coordination model. It should not be bolted onto
  local `.vedit` files during this slice.

Future decision:

- Decide whether Awidat remains solo-editor-with-agent or becomes
  collaborator-aware.
