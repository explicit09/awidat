# Render Execution Manifest Design

## Goal

Revise the Awidat harness roadmap around the current codebase and define
the first implementation phase: a shared Render Execution Manifest and
FFmpeg replay spine. Stream-copy/remux remains the first follow-up feature
that should consume this spine, not part of the first implementation slice.

## Corrected Roadmap

The earlier harness improvement report was directionally right but stale in
several places. Awidat already has a capability manifest, a professional
capability registry, meaningful render verification gates, package export,
stream export contracts, stream-copy segment rendering, raw-stream GPU
transition rendering, proxy preview state, and acceptance artifacts.

The current bottleneck is therefore not a missing subsystem. The real gap is
that render planning, package export, raw-stream rendering, stream-export
lowering, verification, and eval artifacts do not share one durable
execution record.

The corrected roadmap is:

1. Add a Render Execution Manifest and replay spine.
2. Add backend classification for every render/export path.
3. Add a first-class stream-copy/remux tool over `StreamExportContract`.
4. Extend `verify_render` into a broader quality-gate suite.
5. Add benchmark wrappers that emit the same manifest and artifact bundle.
6. Build a durable proxy/preview cache model after manifest hashing exists.

This order keeps existing architecture intact and prevents new features from
becoming one-off render paths.

## Phase One Scope

Phase one builds a reusable manifest model and replay support for FFmpeg
argv-based renders. It does not add a stream-copy/remux tool, a benchmark
CLI, or full raw-stream replay.

In scope:

- A `crates/render/src/manifest.rs` module with serializable manifest types.
- Public exports from `crates/render/src/lib.rs`.
- Manifest writing from `start_render`.
- Manifest writing from `export_package`.
- Manifest writing or adoption in rendered-output acceptance runs.
- A replay command that runs FFmpeg argv manifests.
- Clear non-support errors for manifests whose backend cannot replay in
  phase one.
- Focused Rust tests for manifest creation, hashing, writing, replay
  classification, and call-site integration.

Out of scope:

- A new stream-copy/remux agent tool.
- Raw-stream GPU replay.
- Package export replay that regenerates sidecars.
- Cache invalidation.
- Full benchmark dashboarding.
- Quality gates beyond wiring existing verification artifacts into the
  manifest where practical.

## Architecture

The manifest lives in `awidat-render` because it describes render execution
evidence, not agent tool behavior. Tool-specific code in `awidat-core` should
only assemble context and call render crate helpers.

Add these core types:

```rust
pub struct RenderExecutionManifest {
    pub schema_version: u32,
    pub manifest_id: String,
    pub created_at: String,
    pub awidat_version: String,
    pub project_root: String,
    pub project_hash: Option<String>,
    pub timeline_hash: Option<String>,
    pub backend: RenderBackendKind,
    pub replay: RenderReplayPlan,
    pub inputs: Vec<RenderInputFingerprint>,
    pub outputs: Vec<RenderOutputArtifact>,
    pub sidecars: Vec<RenderSidecarFingerprint>,
    pub limitations: Vec<RenderManifestLimitation>,
    pub verification: Option<RenderVerificationSummary>,
}
```

The concrete field list can be adjusted during implementation, but the
responsibilities should not drift. The manifest captures what was planned,
what inputs were used, what command or backend was selected, what outputs are
expected, and what limitations are known.

`RenderBackendKind` should include:

- `AssetPreview`
- `AssetSegmentStreamCopy`
- `AssetFullReencode`
- `TimelineFfmpegReencode`
- `TimelineRawStreamGpu`
- `PackageExport`
- `StreamExportRemux`

`RenderReplayPlan` should include:

- `FfmpegArgv { argv: Vec<String>, cwd: Option<String> }`
- `Unsupported { reason: String }`

Do not model raw-stream replay as an executable plan in phase one. Record it
as `Unsupported` with enough metadata for later implementation.

## Data Flow

`start_render` flow:

1. Build the existing `RenderJobSpec`.
2. Classify the backend from the requested scope and render limitations.
3. Build a manifest using the spec args, output path, input fingerprints,
   project/timeline hashes, and limitations.
4. Write the manifest beside the output under `renders/`.
5. Return `manifest_path` in the tool response.
6. Start the existing `JobManager` job unchanged.

`export_package` flow:

1. Build the current package artifacts and `RenderJobSpec`.
2. Apply the existing `ExportPreset`.
3. Write package metadata as it does today.
4. Write a render execution manifest into `renders/package/`.
5. Return `manifest_path` in the tool response.
6. Start the existing `JobManager` job unchanged.

Acceptance flow:

1. Keep existing scorecard and artifact bundle outputs.
2. Either write a manifest through the shared helper or include a manifest
   path as another required artifact once the render path emits it.
3. Do not duplicate manifest schema inside `crates/eval`.

Replay flow:

1. Read a manifest JSON path.
2. Validate `schema_version`.
3. Reject `Unsupported` replay plans with the stored reason.
4. Validate output paths with existing output safety logic.
5. Run the stored FFmpeg argv in the stored cwd.
6. Return the output path and exit status.

## Hashing and Fingerprints

Phase one should hash only files that are already necessary to prove replay
integrity:

- `project.otio.json`
- Direct media inputs referenced by the render spec when discoverable.
- Sidecar files that are already read for package subtitles or verification.
- Output files only after a synchronous replay command finishes.

Do not hash all project media or every index sidecar recursively in phase
one. That would make normal render startup unexpectedly expensive.

Use SHA-256 and stream file reads. Missing optional inputs should be recorded
as limitations or omitted depending on whether the render actually consumed
them.

## Error Handling

Manifest writing should fail loudly before starting a render if the manifest
cannot be written. A render without its manifest is not acceptable for the
new spine.

Replay should fail before invoking FFmpeg when:

- The manifest schema version is unsupported.
- The replay plan is unsupported.
- The FFmpeg argv is empty.
- The output path would overwrite an existing file.
- The output path is unsafe by existing render output policy.
- The cwd does not exist.

The error messages should name the manifest path and the failed field where
possible.

## Testing Strategy

Unit tests in `crates/render`:

- Manifest IDs are deterministic for the same planned content excluding
  `created_at`.
- File hashing streams bytes and reports missing files clearly.
- Backend classification maps known scopes to stable enum values.
- Unsupported replay plans fail before spawning a process.
- FFmpeg argv replay validates cwd and output policy.

Tool tests in `crates/core`:

- `start_render` responses include `manifest_path`.
- `start_render` writes a manifest with `FfmpegArgv` replay for `preview`,
  `segment`, and `full` scopes.
- `export_package` responses include `manifest_path`.
- Package manifests record `PackageExport` backend and the applied preset id.

Eval tests:

- Acceptance artifact bundle includes the render manifest once the shared
  helper is wired in.

Run focused tests first:

```bash
cargo test -p awidat-render manifest
cargo test -p awidat-core start_render export_package
cargo test -p awidat-eval acceptance
```

Then run broader checks if local disk and time allow:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## Phase Two Handoff

After phase one lands, implement the stream-copy/remux tool as the first
consumer:

- Add an agent-facing tool in `crates/core/src/tools/`.
- Accept a validated `StreamExportContract` or a constrained simplified
  request that lowers to one.
- Use `awidat_render::professional::plan_stream_export_args`.
- Start the job through `JobManager`.
- Emit `RenderBackendKind::StreamExportRemux`.
- Emit the same manifest shape and return `manifest_path`.

This keeps the new remux path aligned with the manifest and replay contract
from its first commit.

## Acceptance Evidence

The design is complete when:

- The roadmap explicitly corrects stale report claims.
- Phase one has a clear scope and excludes remux implementation.
- The manifest schema has clear responsibilities.
- Every render call-site change has a named data flow.
- Replay support is limited and explicit.
- Testing and follow-up scope are concrete enough to write an
  implementation plan without rediscovering the architecture.
