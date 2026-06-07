# Indexing Optimization Summary

## Source

- Asset: `/Volumes/Explicit's Hard Drive/Episode3_AI_Regulation_IPOs_Elons_Chip_Play.mp4`
- Duration: 1073.000s
- Resolution: 1280x720
- Codec/FPS: h264, 30/1
- Size: 1007682712 bytes
- Included indexes: audio-energy, beats, scenedetect, clip, face, shot, gaze, frame-quality, color-analysis

## Timing Semantics

- `total_ms` is queue-inclusive pair wall time from dispatcher enqueue.
- `tool_ms` is dispatcher-measured `index_asset` runtime and is the current compute proxy.
- Decode/read/model sub-phases are still inside `tool_ms`; the current indexers do not emit those finer metrics yet.
- The old `shot = total time` shape is a queue artifact: shot compute is sub-second, but it waits for scenedetect, face, gaze, and clip.

## Results

| Run | Concurrency | Wall time ms | Wall time s | Wrote | Failed | Notes |
| --- | ---: | ---: | ---: | ---: | ---: | --- |
| Old benchmark | 2 | 150495 | 150.495 | 9 | 0 | Historical 18-minute run |
| Fresh baseline | 2 | 232506 | 232.506 | 9 | 0 | Current checkout before optimization |
| Final | 2 | 136883 | 136.883 | 9 | 0 | Face sampling restored to 0.25fps; CLIP resource class changed to embedding |
| Concurrency 1 | 1 | 247591 | 247.591 | 9 | 0 | Serial upper bound |
| Concurrency 4 | 4 | 200360 | 200.360 | 9 | 0 | More contention; slower than concurrency 2 |

Final is 95623ms faster than the fresh baseline and 13612ms faster than the old 150.495s benchmark. It did not reach the <=120s target.

## Main Changes

- Restored `face-mcp` default sampling from 0.5fps to 0.25fps, matching the historical long-form benchmark density. Face/gaze sidecars now contain 268 frames on this asset instead of 537.
- Changed bundled CLIP resource class from `exclusive` to `embedding`, allowing CLIP to overlap with scenedetect/face under the existing scheduler.
- Added `montage-index-perf` for repeatable real-video reports with media metadata, queue/tool/write timings, sidecar sizes, sampled frame counts, and explicit temp/cache work directories.

## Remaining Bottlenecks

1. Shared frame extraction/proxy cache for scenedetect, frame-quality, color-analysis, face, gaze, and CLIP. Current final still decodes the same 17:53 H.264 asset multiple times.
2. Finer indexer instrumentation inside `tool_ms`: decode/read time, frame preprocessing, model inference, and JSON assembly/write need separate timers before deeper changes.
3. Adaptive scheduling: concurrency 2 is best in this test, while concurrency 4 inflated tool runtimes. The scheduler should prioritize the critical path while avoiding simultaneous decode-heavy passes.
