# Indexing Performance Report

- Label: `pass2-face-color-first`
- Project: `/Volumes/Explicit's Hard Drive/montage-index-perf-work/projects/montage-index-perf-pass2-face-color-first-15648-1780804603584106000`
- Source: `/Volumes/Explicit's Hard Drive/Episode3_AI_Regulation_IPOs_Elons_Chip_Play.mp4`
- Duration: 1073.000s
- Resolution: 1280x720
- Video codec: `h264`
- FPS: `30/1`
- File size: 1007682712 bytes
- Concurrency: 2
- Indexers: audio-energy, beats, face, clip, color-analysis, frame-quality, scenedetect, gaze, shot

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
- Slowest total: 204034 ms
- Slowest tool runtime: 84300 ms

## Pair Timings

| Indexer | Outcome | Total ms | Tool ms | Queue ms | Launch ms | Write ms | Frames | Output bytes | Perf phases |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| beats | wrote | 30694 | 29735 | 0 | 878 | 1 |  | 60027 |  |
| audio-energy | wrote | 37466 | 35671 | 0 | 1609 | 17 |  | 945560 |  |
| clip | wrote | 108984 | 67072 | 37471 | 3276 | 184 | 537 | 740874 | decode_read_ms=44991<br>frames_processed=537<br>inference_ms=19026<br>model_load_ms=2681<br>probe_ms=77 |
| face | wrote | 117214 | 84300 | 30695 | 1967 | 4 | 268 | 147407 | cluster_ms=9<br>decode_read_ms=6025<br>frames_processed=268<br>inference_ms=78115<br>probe_ms=83<br>setup_ms=61<br>speaker_map_ms=0<br>stitch_ms=0<br>summary_ms=0 |
| color-analysis | wrote | 166312 | 55876 | 109186 | 1036 | 12 | 268 | 545528 | aggregate_ms=29<br>analysis_ms=754<br>decode_read_ms=54748<br>frames_processed=268<br>setup_ms=242 |
| frame-quality | wrote | 172686 | 54716 | 117215 | 465 | 5 | 1073 | 231152 | analysis_ms=1940<br>decode_read_ms=52754<br>frames_processed=1073<br>setup_ms=62 |
| scenedetect | wrote | 200675 | 33495 | 166317 | 713 | 1 | 1073 | 952 | analysis_ms=11<br>decode_read_ms=33156<br>frames_processed=1073<br>setup_ms=64 |
| gaze | wrote | 202744 | 16 | 200676 | 1941 | 2 | 268 | 131166 |  |
| shot | wrote | 204034 | 712 | 202745 | 467 | 1 |  | 4803 |  |
