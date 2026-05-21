# Collaboration Gap Finish Design

## Purpose

Category 11 identified Awidat's strongest collaboration story as vedit: content-addressed timeline commits, semantic OTIO diffs, auto-commit on accepted edits, and agent reasoning in commit bodies. The missing pieces are user-facing references, commit deep dives, blame projection, richer authored notes, review handoff, comment ingest, and later branch/merge workflows.

This design finishes the gap incrementally without destabilizing existing editing behavior. The first implementation slice stays local-first: it adds named checkpoint/read tools and per-clip attribution over the existing vedit log. It also fixes stale version-control docs so agents stop treating auto-commit as future work.

## Scope

In scope:

- Correct `skills/version-control/SKILL.md` to reflect current auto-commit behavior.
- Add `vedit_tag` for named local checkpoints under `.vedit/refs/tags/`.
- Add `vedit_show` for a single commit plus `parent..commit` semantic diff.
- Add `vedit_blame` for projecting recent commit history onto a clip name/media reference.
- Register the new tools in CLI, TUI, and desktop agent registries.
- Expose the same read/checkpoint primitives through desktop Tauri commands for UI consumption.
- Keep all changes inside vedit/version-control/docs for the first slice.

Deferred but tracked:

- User-authored notes and comment threading.
- Desktop diff overlays.
- Branch/switch/merge tool surface.
- Review proxy manifest/import.
- Frame.io/Vimeo/Wipster ingest, which needs credentials and product-specific payload decisions.

## Architecture

The core boundary remains `crates/core/src/vc/mod.rs`. Other crates must not reach into `vedit_core::*` directly. New tool modules call small wrapper functions and return JSON that is easy for agents and the desktop to render.

Tags are flat files in `.vedit/refs/tags/<name>` pointing at resolved commit hashes. They are deliberately cheaper than branches: no checkout behavior, no HEAD movement, no merge semantics.

`vedit_show` resolves one ref, reads the commit metadata, and computes the semantic diff from its first parent to itself. Initial commits diff against an empty timeline.

`vedit_blame` walks the first-parent log newest to oldest, computes each commit's local diff, and returns the first matching changes for a clip. Matching is conservative: clip name, media reference, and animation target strings. It does not claim line-by-line OTIO authorship.

## Data Flow

`tool call or desktop command -> vc wrapper -> vedit repo -> JSON response`

The wrapper is responsible for ref resolution, tag validation, commit reads, diff computation, and blame matching. Tool modules only parse arguments, enforce simple user-facing validation, and serialize responses.

## Error Handling

- Empty or invalid tag names fail before writing.
- Unknown refs return `VcError::UnknownRef`.
- No matching blame entries returns an empty `matches` array plus a clear note.
- Branch/merge requests remain outside this slice. Agents should offer tags/checkpoints instead.

## Verification

Targeted verification for this slice:

- `cargo fmt --all -- --check`
- Targeted core tests for `vc::tag_ref`, `vc::show_commit`, and `vc::blame_clip`.
- Tool tests for `vedit_tag`, `vedit_show`, and `vedit_blame`.
- Full `make check` once disk space allows Rust artifacts to be built.

Current machine state may block Rust compilation because `/System/Volumes/Data` is full. That blocker must be reported rather than treated as a passing check.
