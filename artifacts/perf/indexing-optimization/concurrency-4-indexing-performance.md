# Indexing Performance Report

- Label: `concurrency-4`
- Project: `/Volumes/Explicit's Hard Drive/awidat-index-perf-work/projects/awidat-index-perf-concurrency-4-59040-1780802806954015000`
- Source: `/Volumes/Explicit's Hard Drive/Episode3_AI_Regulation_IPOs_Elons_Chip_Play.mp4`
- Duration: 1073.000s
- Resolution: 1280x720
- Video codec: `h264`
- FPS: `30/1`
- File size: 1007682712 bytes
- Concurrency: 4
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
- Slowest total: 200360 ms
- Slowest tool runtime: 102361 ms

## Pair Timings

| Indexer | Outcome | Total ms | Tool ms | Queue ms | Launch ms | Write ms | Frames | Output bytes |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| beats | wrote | 14490 | 13680 | 0 | 450 | 79 |  | 60027 |
| audio-energy | wrote | 32870 | 29922 | 0 | 1432 | 75 |  | 945560 |
| scenedetect | wrote | 86580 | 85474 | 0 | 666 | 1 | 1073 | 822 |
| clip | wrote | 103284 | 97440 | 0 | 2822 | 13 | 537 | 740712 |
| frame-quality | wrote | 115426 | 99967 | 14491 | 722 | 11 | 1073 | 231020 |
| color-analysis | wrote | 129357 | 95174 | 32885 | 1087 | 16 | 268 | 545371 |
| face | wrote | 196790 | 102361 | 86580 | 7603 | 2 | 268 | 147158 |
| gaze | wrote | 198589 | 19 | 196790 | 1621 | 3 | 268 | 131166 |
| shot | wrote | 200360 | 999 | 198590 | 567 | 122 |  | 4803 |
