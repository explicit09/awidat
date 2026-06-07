# Auto-Indexing Performance Design

## Problem

Montage should make imported media useful to the agent as soon as it enters the
media bin. Today the desktop backend already starts a post-import chain after
`import_local`, `import_locals`, and `import_url`, but indexing is still shaped
as a coarse project-wide run. Users can perceive indexing as tied to "Add to
timeline" because the visible agent context and readiness surfaces do not make
bin-only assets feel immediately available.

The target behavior is agent-first: any media-bin import should enqueue
indexing immediately after the raw asset exists. Timeline placement should
consume indexed context, not start it.

## Goals

- Start auto-indexing after every user import path that adds source media to the
  bin: import button, drag/drop into media bin, and URL import.
- Index newly imported asset IDs first, with a safe whole-project fallback when
  scoped asset resolution fails.
- Stage indexers so agent-critical and user-visible context arrives first:
  transcript, audio/waveform/silence, scenes, then topics/moments, then heavier
  visual intelligence.
- Keep generated B-roll/media out of the expensive default source-media index
  path when Montage already knows what it generated. The generation pipeline
  should write a compact descriptive sidecar for the agent immediately.
- Adapt scheduling to average machines and powerful machines without requiring
  users to tune environment variables.
- Preserve the existing dispatcher, sidecar schema, dependency handling,
  resource classes, and idempotency model.

## Non-Goals

- Rewriting all Python indexers.
- Replacing the MCP dispatcher.
- Making full visual intelligence block import completion or timeline editing.
- Treating generated media as raw unknown footage by default.
- Building a cloud render/index farm.

## Current State

The desktop import backend already runs a background post-import chain after
local and URL imports. That chain processes proxy/thumbnail/waveform/silence
and motion work, then calls `index_project_at_root`.

The Rust dispatcher already provides important foundations:

- metadata-fingerprint idempotency before launching indexers;
- topo-aware scheduling through `depends_on`;
- sidecar-based dependency satisfaction;
- resource classes for `exclusive`, `vision`, `light`, `network`, and
  `embedding` work;
- progress events for started and completed `(indexer, asset)` pairs.

The main gaps are orchestration and prioritization, not basic indexing
capability.

## Proposed Architecture

### 1. Import Event Contract

Every import path that creates a source asset under `raw/` returns or records
the project-relative asset IDs it added. The backend then enqueues an indexing
request with those IDs.

Required trigger paths:

- desktop Import files button;
- drag/drop into the media bin, if present or added;
- URL import;
- new-project creation that imports selected local files or a URL.

`insert_media_on_timeline` must not be an indexing trigger. It may refresh
readiness after placement, but it should not be required for agent context.

### 2. Scoped Indexing API

Add a scoped desktop backend entrypoint that accepts asset IDs:

```text
index_project_assets(project_root, asset_ids, mode)
```

`mode` controls tier selection, not correctness:

- `fast_context`: first-pass auto-index after import.
- `full_context`: complete configured index set.
- `manual`: current Run indexers behavior.

Resolution rules:

- Resolve each asset ID to a file under `raw/`.
- Reject unsafe or missing asset IDs clearly.
- If any scoped resolution error is ambiguous, log it and fall back to the
  current whole-project `index_project_at_root` behavior.
- Keep `index_project_at_root` as the manual and fallback path.

This avoids scanning and scheduling stale project assets on every import while
retaining the current robust behavior when scoped input is unreliable.

### 3. Indexing Tiers

The planner orders work by agent usefulness and user-visible payoff.

Tier 0: import readiness

- source exists;
- proxy/transcode for preview when needed;
- thumbnail and waveform;
- built-in silence/motion sidecars when cheap enough for the selected profile.

Tier 1: fast agent context

- `audio-energy`;
- `beats`;
- `scenedetect`;
- `whisper`.

Tier 2: semantic context

- `topic`;
- `editorial-moments`.

Tier 3: visual intelligence

- `frame-quality`;
- `color-analysis`;
- `face`;
- `gaze`;
- `clip`;
- `shot`;
- `composition` when present in config.

Tier 1 should start as soon as the raw asset exists. Tier 2 should follow
dependency completion. Tier 3 should run opportunistically and should never
make the user think import is blocked.

### 4. Machine Profiles

Add an internal machine profile decision that is conservative by default:

- Average profile: default for fewer than 8 logical cores, high load average,
  low memory headroom, or unknown telemetry. Run one heavy model at a time,
  prefer Tier 1 and Tier 2, and defer Tier 3 visual work.
- Powerful profile: 8 or more logical cores, low load, and enough memory
  headroom. Allow light/audio/scene passes to overlap with one visual pass,
  but do not overlap `whisper` and `clip` until telemetry proves it is safe.

The profile feeds planner decisions. It should not leak into indexer code as
ad hoc conditionals.

### 5. Generated Media and B-Roll

Generated B-roll/media should not enter the default raw-source indexing queue
unless explicitly requested. The generator already knows the prompt, provider,
duration, intended use, and output path. It should write a compact sidecar such
as:

```text
index/generated-description/<asset-id>.json
```

Minimum fields:

- `asset_id`;
- `source`: generated provider/job id;
- `prompt` or normalized creative brief;
- `duration_s`;
- `visual_summary`;
- `intended_use`;
- `created_at`;
- `confidence` or `provenance` metadata.

Agents can use this immediately for timeline planning. Full visual indexing
can remain a manual or background enhancement.

### 6. Progress and Readiness

The UI should present readiness by layer, not as one binary "indexed" state.
The agent-facing tools should read the same layer state.

Important layer labels:

- source;
- proxy;
- waveform;
- transcript;
- scenes;
- audio/silence;
- topics;
- moments;
- visual quality;
- faces/gaze;
- visual search;
- generated description.

This makes bin-only media visible to the agent and human before timeline
placement.

## Error Handling

- Import succeeds even if post-import indexing fails.
- Scoped indexing failures are asset-scoped and indexer-scoped.
- Dependency failures should keep using the existing dep-skipped outcome.
- Generated-description sidecar failures should be surfaced as generator
  warnings, not as raw indexer failures.
- Manual Run indexers remains the repair path for stale or failed sidecars.

## Testing Strategy

Backend tests:

- importing local files enqueues scoped asset IDs;
- URL import enqueues the downloaded asset ID;
- scoped index resolves only requested raw assets;
- missing/unsafe scoped IDs fall back or error according to the contract;
- `insert_media_on_timeline` does not start indexing;
- generated media writes a descriptive sidecar without invoking full indexing.

Dispatcher/planner tests:

- tier ordering runs `whisper` before `topic` and `editorial-moments`;
- average profile defers heavy visual work;
- powerful profile allows more parallel light/vision work without overlapping
  exclusive jobs unsafely;
- disabled indexer overlays still apply.

Frontend tests:

- Import button and URL import show background indexing job cards for the new
  asset.
- Drag/drop into media bin routes through the same import command and receives
  the same auto-index behavior.
- Add to timeline does not show a new indexing job unless indexing is already
  running for that asset.

## Implementation Slices

1. Add scoped indexing entrypoint around the existing dispatcher.
2. Change post-import chain to pass imported asset IDs to scoped indexing.
3. Add tier planner and machine profile.
4. Route drag/drop-to-bin through import if it is not already present.
5. Add generated-media descriptive sidecars.
6. Improve layer readiness reporting for agent and UI surfaces.
7. Measure repeated FFmpeg decode cost and decide whether to add a shared
   sparse-frame/audio cache as the next performance project.

## Acceptance Criteria

- Importing media into the bin starts indexing without adding media to the
  timeline.
- New imports index their own asset IDs first.
- Whole-project indexing remains available and is used as fallback.
- Transcript and other fast context appear before heavy visual sidecars.
- Generated B-roll has immediate semantic context without full indexing.
- Average machines stay responsive; powerful machines use more parallelism.
- Existing indexer sidecars, dependency semantics, disabled-indexer overlays,
  and manual Run indexers behavior continue to work.
