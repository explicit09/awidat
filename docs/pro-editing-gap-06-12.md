# Pro editing gap closure: 06 Color and 12 Export/Delivery

This note reconciles the color and export/delivery gap review with the current
implementation state.

## 06 Color

### Closed or narrowed

- **Masked look-LUT render path:** `montage.color_pipeline.mask_source` now has
  a renderable v1 path for a single `look_lut` plus static image mask. The
  render graph inserts the mask input, scales it to the clip, alphamerges it
  into the LUT branch, and overlays the masked grade over the original clip.
- **Effect metadata:** `montage.color_pipeline` is marked
  `FfmpegNative` because the FFmpeg render path supports the color-management
  chain and the v1 masked look-LUT subset. Unsupported mask combinations still
  surface explicit limitations.
- **Scopes:** the CLI/TUI tool registry exposes `color_scopes`. It samples a
  frame and returns machine-readable luma histogram, RGB histogram, luma
  waveform, RGB parade, and Cb/Cr vectorscope data. This is not a desktop
  real-time scope dock yet, but it gives the agent objective per-frame scope
  evidence.
- **`.3dl` policy drift:** `.3dl` support is intentional for now. `.cube` and
  `.3dl` are parser-validated before graph application; `.dat`, `.m3d`, and
  `.csp` remain extension-accepted render/pass-through paths rather than
  in-tree parsers.

### Still open

- Real-time desktop scope panels.
- OCIO or a formally documented long-term alternative to the hand-rolled
  `zscale` mapping.
- Curves, color wheels, tracked power windows, GPU LUTs, and GPU scopes.
- Visual scope tiles in `review_look_regions` contact sheets.

## 12 Export/Delivery

### Closed or narrowed

- **Preset lowering is production-reachable:** `export_package` now builds an
  `ExportPreset` for rendered package formats and applies
  `apply_export_preset_to_spec` before starting the render job.
- **Hardware policy is reachable:** `export_package` accepts
  `hardware_acceleration: "off" | "auto" | "require"` and forwards it through
  `ExportPreset.output.hardware_acceleration`.
- **Hardware codec selection:** `apply_export_preset_to_spec` honors
  `HardwareAccelerationPolicy`. `Off` preserves the preset codec, `Auto` maps
  to native hardware where a mapping exists, and `Require` fails loudly if no
  native mapping is available. macOS maps `libx264` to `h264_videotoolbox` and
  `libx265`/`libx265_hevc` to `hevc_videotoolbox`.
- **MP4 faststart:** MP4 package presets lower to `-movflags +faststart`.
- **Turnover EDL frame rate:** CMX 3600 turnover timecode no longer hard-codes
  24 fps. It derives an EDL frame rate from `timeline.global_start_time`,
  track/stack source ranges, clip/gap source ranges, or transition offsets,
  then falls back to 24 only when no project rate is available.

### Still open

- `start_render` preview/full/timeline scopes still use their existing
  built-in render defaults unless a caller goes through `export_package`.
- Broader preset catalog: ProRes, DNxHR, AV1, H.265 delivery variants, image
  sequence variants, broadcast, archive, HDR, and platform-specific outputs.
- FIFO render queue, pause/resume/remove, and output auto-suffixing.
- AAF/OMF/Pro Tools turnover.
- Embedded captions and embedded chapters.
- A first-class stream-copy/remux tool over `StreamExportContract`.
