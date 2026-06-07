# Indexing Performance Report

- Label: `baseline`
- Project: `/Volumes/Explicit's Hard Drive/awidat-index-perf-work/projects/awidat-index-perf-baseline-90536-1780802007288758000`
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
- Slowest total: 232506 ms
- Slowest tool runtime: 97234 ms

## Pair Timings

| Indexer | Outcome | Total ms | Tool ms | Queue ms | Launch ms | Write ms | Frames | Output bytes |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| audio-energy | wrote | 14785 | 12589 | 0 | 1998 | 19 |  | 945560 |
| beats | wrote | 17881 | 17108 | 0 | 673 | 4 |  | 60027 |
| scenedetect | wrote | 56319 | 40417 | 14793 | 1037 | 0 | 1073 | 822 |
| frame-quality | wrote | 59184 | 40772 | 17881 | 442 | 5 | 1073 | 231020 |
| color-analysis | wrote | 94423 | 34596 | 59185 | 509 | 14 | 268 | 545371 |
| face | wrote | 156093 | 97234 | 56319 | 2332 | 6 | 537 | 296490 |
| clip | wrote | 210102 | 46883 | 156094 | 5997 | 211 | 537 | 740712 |
| gaze | wrote | 225552 | 233 | 210104 | 14815 | 4 | 537 | 264298 |
| shot | wrote | 232506 | 631 | 225554 | 6220 | 0 |  | 4821 |
