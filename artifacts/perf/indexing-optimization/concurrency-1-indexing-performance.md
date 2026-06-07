# Indexing Performance Report

- Label: `concurrency-1`
- Project: `/Volumes/Explicit's Hard Drive/awidat-index-perf-work/projects/awidat-index-perf-concurrency-1-97650-1780803084183434000`
- Source: `/Volumes/Explicit's Hard Drive/Episode3_AI_Regulation_IPOs_Elons_Chip_Play.mp4`
- Duration: 1073.000s
- Resolution: 1280x720
- Video codec: `h264`
- FPS: `30/1`
- File size: 1007682712 bytes
- Concurrency: 1
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
- Slowest total: 247591 ms
- Slowest tool runtime: 61850 ms

## Pair Timings

| Indexer | Outcome | Total ms | Tool ms | Queue ms | Launch ms | Write ms | Frames | Output bytes |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| audio-energy | wrote | 63546 | 61850 | 0 | 1481 | 22 |  | 945560 |
| beats | wrote | 81883 | 17843 | 63551 | 418 | 2 |  | 60027 |
| scenedetect | wrote | 109044 | 26427 | 81884 | 644 | 0 | 1073 | 822 |
| clip | wrote | 142460 | 30565 | 109044 | 2369 | 16 | 537 | 740712 |
| face | wrote | 191761 | 47045 | 142460 | 2059 | 11 | 268 | 147158 |
| gaze | wrote | 192793 | 11 | 191762 | 938 | 10 | 268 | 131166 |
| shot | wrote | 193981 | 647 | 192794 | 467 | 0 |  | 4804 |
| frame-quality | wrote | 220844 | 26374 | 193981 | 357 | 9 | 1073 | 231020 |
| color-analysis | wrote | 247591 | 26272 | 220845 | 356 | 16 | 268 | 545371 |
