# Pass 2 Indexing Optimization Summary

## Source

- Asset: `/Volumes/Explicit's Hard Drive/Episode3_AI_Regulation_IPOs_Elons_Chip_Play.mp4`
- Duration: 1073.000s
- Resolution: 1280x720
- Codec/FPS: h264, 30/1
- Size: 1007682712 bytes
- Included indexes: audio-energy, beats, scenedetect, clip, face, shot, gaze, frame-quality, color-analysis

## Baseline And Final

| Run | Concurrency | Wall time ms | Wall time s | Wrote | Failed | Notes |
| --- | ---: | ---: | ---: | ---: | ---: | --- |
| Old benchmark | 2 | 150495 | 150.495 | 9 | 0 | Historical 18-minute reference |
| Previous best | 2 | 136883 | 136.883 | 9 | 0 | End of pass 1 |
| Pass 2 baseline | 2 | 261520 | 261.520 | 9 | 0 | Instrumented current-order run |
| Pass 2 face-first experiment | 2 | 204642 | 204.642 | 9 | 0 | Face and CLIP moved earlier |
| Pass 2 face/color-first experiment | 2 | 204034 | 204.034 | 9 | 0 | Color moved before frame-quality/scenedetect |
| Pass 2 final | 2 | 136177 | 136.177 | 9 | 0 | Default bundled order updated |

Pass 2 final is 125343ms faster than the pass 2 baseline, 706ms faster than the previous best, and 14318ms faster than the old 150.495s benchmark.

## What Changed

- Added additive `data.perf` phase timings to visual sidecars where practical.
- Updated the Rust performance report parser to extract and render `data.perf`.
- Reordered bundled non-transcription defaults to prioritize the measured critical path:
  audio-energy, beats, face, clip, color-analysis, frame-quality, scenedetect, gaze, shot.
- Updated the perf harness default order to match the bundled defaults so default benchmark runs are not hand-overridden.

## Output Shape Check

The pass 2 final preserved sampled work counts versus pass 2 baseline:

| Indexer | Baseline frames/items | Final frames/items |
| --- | ---: | ---: |
| audio-energy | 10730 | 10730 |
| beats | 679 | 679 |
| scenedetect | 1073 | 1073 |
| clip | 537 | 537 |
| frame-quality | 1073 | 1073 |
| face | 268 | 268 |
| gaze | 268 | 268 |
| color-analysis | 268 | 268 |
| shot | 2 | 2 |

## Phase Timing Evidence

Pass 2 final visual phase timings:

| Indexer | Main phase evidence |
| --- | --- |
| face | inference_ms=70227, decode_read_ms=3314 |
| clip | decode_read_ms=23055, inference_ms=14981, model_load_ms=2349 |
| color-analysis | decode_read_ms=35127, analysis_ms=782 |
| frame-quality | decode_read_ms=42119, analysis_ms=2024 |
| scenedetect | decode_read_ms=40012, analysis_ms=7 |

The remaining safe high-leverage work is shared/proxy frame extraction for decode-bound visual passes. Face is now mostly dlib inference, so shared decode alone will not remove the face cost.

## Stop Condition

No further code optimization was made in this pass because the remaining obvious gain requires a shared frame/proxy cache across independent MCP indexers. That is larger architecture work and needs a separate design to preserve quality, schema compatibility, and cache invalidation correctness.
