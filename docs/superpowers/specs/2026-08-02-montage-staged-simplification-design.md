# Montage Staged Simplification Design

**Status:** Approved on 2026-08-02

## Problem

Montage grew from a small editing harness into a 150-member Rust workspace with
multiple product-scale subsystems. The first-party tree now contains roughly
261k Rust lines, a 63k-line desktop frontend, 118 MCP tool modules, and several
large speculative surfaces. Size alone is not the defect: the defect is that
historical scaffolding, dormant product concepts, and stale control-plane
configuration make the live editing path harder to understand, build, and
change.

The public product contract remains the path described by `README.md` and
`ARCHITECTURE.md`:

```text
import -> index -> inspect/reason -> apply EDL -> render -> verify
```

Social publishing is a real user-facing subsystem, but it is not part of the
documented core path. It remains intact during Wave 1 and will receive a
separate keep/isolate/remove decision after its runtime boundary is measured.

## Chosen Approach

Use staged subtraction. Each wave must leave working, testable software and
must reduce actual ownership cost rather than merely move code between files.

1. Repair the repository control plane and remove source-proven dead weight.
2. Delete dormant professional orchestration while retaining the three live
   workflows and their contracts.
3. Measure product boundaries before isolating publishing or the vendored
   Codex workspace.
4. Simplify tool, desktop, and render internals only where characterization
   tests expose a smaller equivalent design.

A hygiene-only pass was rejected as insufficient. A rewrite was rejected
because it would discard working editing and safety behavior before equivalent
proof existed.

## Wave 1 Changes

### Repository control plane

- Remove deleted packages from `MONTAGE_APP_PACKAGES` so the documented
  `make check-app` lane resolves again.
- Remove the empty `legacy_local_publishing` feature. It has no `cfg` users and
  therefore controls no behavior.
- Ignore local agent/worktree state without deleting it.
- Refresh comments that still describe the current MCP server as a one-tool
  migration stub.

### Dependency surface

Remove direct dependencies whose crate identifiers do not occur in their
package source:

- `montage-core`: `async-stream`, `eventsource-stream`, `rusqlite`
- `montage-render-gpu`: `tracing`
- `montage-index`: `tokio-util`
- `montage-cli`: `tracing`
- `montage-social`: `rand`
- `montage-social-server`: `thiserror`
- `montage-desktop-protocol`: `chrono`
- `montage-desktop`: `anyhow`

Compilation is the authoritative proof that no macro, generated target, or
feature-only path needs one of these declarations.

### Dormant professional orchestration

`crates/core/src/professional.rs` was expanded by about 4,080 lines in one
speculative commit. Current production callers use only:

- `build_workflow_lens_snapshots`
- `inspect_pre_autonomy_readiness`
- `derive_audio_finishing_state`

Wave 1 retains those functions, the types they return, their private helper
closure, and focused tests. It removes the 23 public types and 13 public
functions with no production or integration-test callers, along with unit
tests that only preserve those dormant concepts.

## Safeguards That Stay

- OTIO and project-schema backward/forward compatibility.
- EDL validation, approval, rationale, history, diff, rollback, and picture
  lock behavior.
- Render preflight, timeout, cancellation, polling, and rendered-output
  verification.
- Skill loading and per-skill tool allowlists.
- Direct MCP tool exposure. Its implementation records a live A/B regression
  where deferred tools caused the agent to bypass Montage's edit flow.
- Secret storage, credential redaction, OAuth state validation, publishing
  authorization, and provider-side safety checks.
- All unrelated working-tree edits.

## Verification

Wave 1 is complete only if:

1. `make fmt-app` resolves every listed package and passes.
2. All touched Rust packages compile and their focused tests pass.
3. Live professional workflow tests pass after dormant code removal.
4. Desktop TypeScript typechecking passes.
5. Safe Python indexer smoke tests pass.
6. Static searches confirm removed features, dependencies, and dormant public
   symbols are absent.
7. A final diff audit confirms the user's pre-existing changes were not
   modified or staged.

## Deferred Decisions

- Publishing: keep integrated, isolate as an optional product, or remove.
- Vendored Codex: keep one workspace, create a reproducible nested workspace,
  or consume a pinned external sidecar artifact.
- MCP tools: retain current direct exposure until a usage/eval harness proves a
  smaller catalog or deferred discovery behaves correctly.
- Desktop and CSS: split only along live responsibility boundaries; do not
  create component shards just to reduce line counts.
- Render/EDL: simplify last, behind fixture and rendered-output parity because
  these modules encode the product's core behavior.
