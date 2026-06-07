# Indexing Performance Report

- Label: `final`
- Project: `/Volumes/Explicit's Hard Drive/awidat-index-perf-work/projects/awidat-index-perf-final-42584-1780802643422912000`
- Source: `/Volumes/Explicit's Hard Drive/Episode3_AI_Regulation_IPOs_Elons_Chip_Play.mp4`
- Duration: 1073.000s
- Resolution: 1280x720
- Video codec: `h264`
- FPS: `30/1`
- File size: 1007682712 bytes
- Concurrency: 2
- Indexers: audio-energy, beats, scenedetect, clip, face, shot, gaze, frame-quality, color-analysis

## Timing Semantics

- `total_ms` is pair wall time from scheduler enqueue and includes `queued_ms`.
- `tool_ms` is the dispatcher-measured `index_asset` runtime and is the closest current proxy for exclusive indexer compute time.
- Decode/read/model phases are inside `tool_ms` unless an indexer emits finer-grained sidecar metrics.

## Summary

- Pairs: 9
- Wrote: 9
- Skipped: 0
- Failed: 0
- Blocked by dependency: 0
- Slowest total: 136883 ms
- Slowest tool runtime: 80310 ms

## Pair Timings

| Indexer | Outcome | Total ms | Tool ms | Queue ms | Launch ms | Write ms | Frames | Output bytes |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| beats | wrote | 8394 | 7916 | 0 | 371 | 23 |  | 60027 |
| audio-energy | wrote | 13756 | 12290 | 0 | 1208 | 18 |  | 945560 |
| scenedetect | wrote | 49151 | 39955 | 8394 | 698 | 1 | 1073 | 822 |
| clip | wrote | 63727 | 46801 | 13761 | 2611 | 24 | 537 | 740712 |
| frame-quality | wrote | 101401 | 37047 | 63730 | 507 | 12 | 1073 | 231020 |
| face | wrote | 132312 | 80310 | 49151 | 2530 | 7 | 268 | 147158 |
| gaze | wrote | 133690 | 14 | 132313 | 1213 | 51 | 268 | 131166 |
| shot | wrote | 134869 | 602 | 133690 | 507 | 1 |  | 4803 |
| color-analysis | wrote | 136883 | 34884 | 101403 | 440 | 62 | 268 | 545371 |
