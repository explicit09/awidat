# Caption frame-pixel scorer — design

Status: approved 2026-05-22, ready for implementation planning.

## Goal

Replace the metadata/sidecar-derived caption rendered-output evidence in `verify_render` with real, measured evidence from decoded frames of the render output. Keep the existing sidecar derivation as an explicit, named fallback path when frame decoding is unavailable (no ffmpeg, no output file, parse failure).

This closes the "frame-pixel caption occlusion scoring" gap recorded in the Awidat improvement living plan.

## Non-goals

- Multi-frame sampling per caption event in this slice. A single midpoint probe per event is sufficient for first-cut measured evidence.
- OCR-based caption readability. The occlusion signal in this slice is a luminance-variance heuristic, not character recognition.
- A trait abstraction over the whole verification flow. The trait surface is scoped to frame sampling.
- Changes to the preview worker pool. That gap is the next slice and gets its own design.

## Architecture

### New module: `crates/core/src/caption_rendered_output_scorer.rs`

Public surface:

```rust
pub async fn score_caption_rendered_output(
    render_output: &Path,
    layout_sidecars: &[PathBuf],
    video_dims: (u32, u32),
    safe_area_profile: &str,
    sampler: &dyn CaptionFrameSampler,
) -> Result<CaptionRenderedOutputEvidence, ScorerError>;

pub struct CaptionRenderedOutputEvidence {
    pub probe_count: usize,
    pub safe_area_pass_count: usize,
    pub occlusion_fail_count: usize,
    pub per_event_findings: Vec<CaptionEventFinding>,
    pub fallback_reason: Option<&'static str>,
}

pub trait CaptionFrameSampler: Send + Sync {
    async fn sample(&self, t_s: f64) -> Result<DecodedFrame, ScorerError>;
}

pub struct DecodedFrame {
    pub width: u32,
    pub height: u32,
    pub luma: Vec<u8>, // grayscale plane, row-major
}
```

Production implementation `FfmpegFrameSampler`:

- Wraps a new sibling helper in `awidat_render::ffmpeg`, `extract_frame_raw_gray`, that invokes ffmpeg with `-f rawvideo -pix_fmt gray -frames:v 1` and returns `(width, height, Vec<u8>)`. This avoids a PNG/JPEG encode + decode roundtrip and avoids adding a new image-decoding workspace dep — the `image` crate is not currently in the workspace, and pulling it in for one decode is unnecessary.
- The returned grayscale plane is the `luma` field of `DecodedFrame` directly. No further decoding step.
- Output dimensions are bounded to the render's resolution (no scale filter) so bbox coords compute against the real render extent.

Test implementation `InMemoryFrameSampler`:

- Stores `(t_s_rounded, DecodedFrame)` pairs.
- Lookup tolerance: ±25 ms (single midpoint probes round consistently).
- Used by all focused unit tests in the scorer module and by the verify_render integration test.

### Algorithm

For each caption event extracted from the layout sidecars:

1. Parse `Dialogue:` lines from the ASS sidecar to get `(start_s, end_s, text, style_name)`. Read the `Style:` line referenced by each Dialogue line for `MarginL`, `MarginR`, `MarginV`, `Alignment`, and `Fontsize`.
2. Probe time `t_s = (start_s + end_s) / 2.0`.
3. Compute caption bounding box in output pixel coordinates:
   - Read `PlayResX`, `PlayResY` from the ASS header. Default to `video_dims` if absent.
   - Scale style margins from PlayRes coords into output pixel coords.
   - Compute bbox center using `Alignment` (ASS numpad layout: 1/4/7 = left, 2/5/8 = center, 3/6/9 = right; 1/2/3 = bottom, 4/5/6 = mid, 7/8/9 = top).
   - Bbox height = `Fontsize` scaled into output pixels times line count (parsed from `\N` in text). Bbox width = `output_width − MarginL − MarginR` scaled into output pixels.
4. Safe-area check: bbox ⊆ (frame inset by the safe-area profile's margins). Initial profiles are `default` (5% inset on all sides) and `mobile` (10% inset top/bottom, 5% inset left/right). Unknown profile → safe-area pass count not incremented, recorded as `unknown_safe_area_profile` in `per_event_findings`.
5. Occlusion check: sample the sampler at `t_s`, extract the luma plane, compute:
   - `inside_variance` = population variance of luma values inside the bbox.
   - `halo_variance` = population variance of luma values in an 8-pixel halo around the bbox, clipped to the frame.
   - Pass iff `inside_variance >= halo_variance + epsilon`, with `epsilon = 4.0` (variance units on 0..255 luma — captions on natural footage typically clear this by an order of magnitude; this is the floor we accept).
   - On fail, `occlusion_fail_count` is incremented.

Probe count = number of caption events for which sampling and bbox computation succeeded. Events that fail to parse or fail to sample are recorded in `per_event_findings` with a `reason`, do not count toward probe count, and set `fallback_reason = Some("partial_scorer_evidence")` so the gate can decide how to react.

### Wiring change in `crates/core/src/tools/verify_render.rs`

- `verify_render_output` is already async. Before the sync gate-builder runs:
  - When `caption_summary.has_exportable_captions` is true AND the manifest has an output path AND layout sidecar paths are surface-able, attempt `score_caption_rendered_output`.
  - Layout sidecar paths: today the manifest summary stores only counts. Adding `libass_layout_sidecar_paths` (comma-separated, in stable order) to the manifest metadata produced by `crates/render/src/manifest.rs` is part of this slice. The render path already enumerates the sidecars to produce the counts; the change is to also record their paths.
  - On scorer success, inject into the metadata view used by the gate:
    - `caption_rendered_output_status = "passed"` (or `"failed"`)
    - `caption_rendered_output_probe_count = N`
    - `caption_rendered_output_safe_area_pass_count = N`
    - `caption_rendered_output_occlusion_fail_count = N`
    - `caption_rendered_output_source = "frame_pixel_scorer"`
  - On scorer failure, set `caption_rendered_output_fallback_reason` (e.g. `"ffmpeg_unavailable"`, `"render_output_missing"`, `"sidecar_parse_failed"`) and leave the existing `libass_layout_supports_caption_rendered_output` path to apply.
- `add_caption_rendered_output_gate` stays the same shape. The `reason` set expands to:
  - `frame_pixel_scorer_passed`
  - `frame_pixel_scorer_failed`
  - `frame_pixel_scorer_unavailable_fell_back_to_libass_layout`
  - existing `derived_from_libass_layout_evidence`, `missing_caption_rendered_output_evidence`, `caption_rendered_output_evidence_failed`.

### Capability metadata

`crates/core/src/capabilities.rs` and `crates/core/src/capability_metadata.rs`:

- The `caption_safe_area_verification` capability gains a note that rendered-output evidence is now frame-pixel measured when a render output and ffmpeg are available, with libass-layout sidecar derivation retained as a named fallback.
- The verification-limit note removes "still inferred from metadata and sidecars" for the rendered-output case; libass-layout fallback is mentioned separately.

### Manifest surface change

`crates/render/src/manifest.rs`:

- Add `libass_layout_sidecar_paths` to the metadata BTreeMap when sidecars are present, alongside the existing count fields. Comma-joined, stable order.
- Update the existing manifest test fixtures to expect the new key when sidecars exist.

## Data flow

```
RenderManifestEvidence
    output_path
    libass_layout_sidecar_paths   ←  new metadata key
        │
        ▼
async verify_render_output
        │
        ▼
score_caption_rendered_output  ←  FfmpegFrameSampler (prod) | InMemoryFrameSampler (tests)
        │
        ▼
inject caption_rendered_output_*  +  caption_rendered_output_source = "frame_pixel_scorer"
        │
        ▼
add_caption_rendered_output_gate (unchanged shape)
        │
        ▼
verify_render report
```

## Error handling

- ffmpeg not on PATH → `ScorerError::SamplerUnavailable("ffmpeg_unavailable")` → fallback path.
- Render output file missing or zero-sized → `ScorerError::RenderOutputMissing` → fallback.
- Sidecar parse failure on every sidecar → `ScorerError::SidecarParseFailed` → fallback.
- Partial parse (some events readable) → return `CaptionRenderedOutputEvidence` with `fallback_reason = Some("partial_scorer_evidence")`. Gate prefers measured evidence but the partial flag is surfaced in gate `details`.
- Frame sampling error for an individual event → that event does not count toward probe count and is recorded as a `per_event_findings` entry with `reason = "sample_failed"`.

## Testing strategy

Focused unit tests in `caption_rendered_output_scorer.rs`:

1. `scorer_passes_when_event_within_safe_area_and_variance_high` — single event inside default safe area, in-memory frame with synthetic checkerboard pattern in bbox → safe-area pass + occlusion pass.
2. `scorer_fails_safe_area_when_event_outside_margin` — event with PlayRes large enough that the bbox extends past the safe-area inset → safe-area pass count not incremented for that event.
3. `scorer_fails_occlusion_when_inside_variance_low` — flat-luma frame where bbox interior matches halo → occlusion fail count incremented.
4. `scorer_returns_empty_evidence_when_no_events` — sidecar without Dialogue lines → `probe_count = 0`, `fallback_reason = Some("no_caption_events")`.
5. `scorer_reports_partial_when_some_events_unparseable` — mixed parseable/unparseable Dialogue lines → partial flag set, only parseable events scored.

Integration tests in `verify_render.rs` (extend the existing test module):

6. `verify_render_uses_frame_pixel_scorer_when_render_output_present` — synthetic manifest with output path + sidecar paths + in-memory sampler returns passing scorer evidence → gate passes with `reason = "frame_pixel_scorer_passed"`.
7. `verify_render_falls_back_to_libass_layout_when_scorer_unavailable` — scorer reports `SamplerUnavailable` → gate passes via existing libass-layout derivation with `reason = "frame_pixel_scorer_unavailable_fell_back_to_libass_layout"`.
8. `verify_render_reports_frame_pixel_scorer_failed_on_failed_evidence` — scorer reports occlusion failures → gate fails with `reason = "frame_pixel_scorer_failed"`.

Manifest test:

9. `manifest_records_libass_layout_sidecar_paths` — adding sidecars to a manifest builds metadata that includes the new comma-joined paths key in stable order.

Capability test update:

10. Update `capability_manifest_adds_explicit_known_tool_metadata` (or its caption-specific cousin) to reflect the new note language.

No heavyweight media fixtures are required. The scorer's only ffmpeg path is in `FfmpegFrameSampler`, which is exercised only by the production `verify_render` flow when a real render output exists.

## Components

| Component | Responsibility | Depends on |
|---|---|---|
| `caption_rendered_output_scorer` (new) | Parse sidecars, compute bbox, run sampler, compute counters | `awidat_render::ffmpeg` |
| `CaptionFrameSampler` trait (new) | Single-method async sampling at a timestamp | `tokio` |
| `FfmpegFrameSampler` (new) | Production sampler over `extract_frame_raw_gray` | `awidat_render::ffmpeg` |
| `extract_frame_raw_gray` (new, in `awidat_render::ffmpeg`) | Decode a single grayscale frame at `t_s` to raw bytes | `tokio::process` |
| `InMemoryFrameSampler` (new, test-only) | Lookup by rounded timestamp | std only |
| `verify_render_output` (changed) | Run scorer, inject metadata, fall back to sidecar derivation | scorer, manifest |
| `crates/render/src/manifest.rs` (changed) | Add `libass_layout_sidecar_paths` key | std |
| `capabilities.rs` / `capability_metadata.rs` (changed) | Updated rendered-output verification note | std |

## Build sequence

1. Add `libass_layout_sidecar_paths` to manifest with focused test.
2. Add `extract_frame_raw_gray` to `awidat_render::ffmpeg`. No focused unit test (real ffmpeg invocation); exercised indirectly via the production scorer when a real render exists.
3. Add `caption_rendered_output_scorer` module with `InMemoryFrameSampler`-driven focused tests (no ffmpeg yet).
4. Add `FfmpegFrameSampler` (production wrapper around `extract_frame_raw_gray`).
5. Wire scorer into `verify_render_output` with the three integration tests above.
6. Update capability metadata notes + capability test fixture.
7. Run focused tests, format, package clippy.
8. Run broad `cargo test --workspace` + workspace clippy + fmt before claiming the slice complete.
