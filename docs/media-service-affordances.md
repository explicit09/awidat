# Media Service Affordances

This note captures media-service affordances that should stay in existing
Montage project caches and sidecars unless a user-facing workflow proves a
separate service is needed.

## Concept Map

- Proxy status: Montage already writes proxy mp4 files under `.montage/proxies/` and exposes a lifecycle report with fresh, stale, missing, orphan, and pending states through the desktop transcode command.
- Waveform extraction: Montage already writes per-asset waveform sidecars under `.montage/waveforms/` and the desktop timeline cache lazily reads them.
- Loudness measurement: `audio-energy-mcp` already emits integrated LUFS and short-term loudness. It now also emits `true_peak_dbfs` so delivery checks can distinguish quiet masters from clipped or near-clipped material without a new analyzer.
- Thumbnail candidate scoring: `frame-quality-mcp` already samples sharpness, brightness, and contrast. It now emits per-frame `thumbnail_score` plus ranked `thumbnail_candidates`, using those existing signals instead of adding model inference.
- Export/package presets: Montage already has durable `DeliveryProfile` and `ExportPreset` contracts in `crates/proto` and lowering/preflight helpers in `crates/render`.
- Composition verification: Montage already generates composition regions and render-side composition graph diagnostics. `composition-mcp` now includes a lightweight `verification` report for generated regions so malformed ranges, missing sources, and bad confidence values are visible in the existing sidecar.

## Deferred

- Post-composition thumbnail rendering like the reference `ThumbnailCandidates` should wait until Montage has a stable composition preview artifact to score.
- Rendered-output verification like the reference `CompositionVerifier` should extend existing render preflight or smoke fixtures, not bypass `crates/render` job APIs.
- Platform-specific preset expansion should be additive to the existing `ExportPreset` registry and tests.
