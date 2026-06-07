# Indexing Performance Report

- Label: `pass2-baseline`
- Project: `/Volumes/Explicit's Hard Drive/montage-index-perf-work/projects/montage-index-perf-pass2-baseline-31045-1780804056006883000`
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
- Slowest total: 261520 ms
- Slowest tool runtime: 126763 ms

## Pair Timings

| Indexer | Outcome | Total ms | Tool ms | Queue ms | Launch ms | Write ms | Frames | Output bytes | Perf phases |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| beats | wrote | 24690 | 23693 | 3 | 818 | 8 |  | 60027 |  |
| audio-energy | wrote | 29792 | 26712 | 1 | 2369 | 38 |  | 945560 |  |
| scenedetect | wrote | 121300 | 95000 | 24692 | 1180 | 3 | 1073 | 952 | analysis_ms=14<br>decode_read_ms=94593<br>frames_processed=1073<br>setup_ms=76 |
| clip | wrote | 147006 | 107679 | 29814 | 7248 | 52 | 537 | 740874 | decode_read_ms=58641<br>frames_processed=537<br>inference_ms=42642<br>model_load_ms=6195<br>probe_ms=87 |
| frame-quality | wrote | 206381 | 57853 | 147011 | 1411 | 10 | 1073 | 231152 | analysis_ms=1902<br>decode_read_ms=55837<br>frames_processed=1073<br>setup_ms=85 |
| face | wrote | 256174 | 126763 | 121300 | 7571 | 4 | 268 | 147411 | cluster_ms=16<br>decode_read_ms=6748<br>frames_processed=268<br>inference_ms=119616<br>probe_ms=153<br>setup_ms=64<br>speaker_map_ms=16<br>stitch_ms=0<br>summary_ms=0 |
| gaze | wrote | 259115 | 92 | 256174 | 2465 | 6 | 268 | 131166 |  |
| color-analysis | wrote | 260724 | 53378 | 206385 | 604 | 21 | 268 | 545527 | aggregate_ms=27<br>analysis_ms=766<br>decode_read_ms=52468<br>frames_processed=268<br>setup_ms=64 |
| shot | wrote | 261520 | 794 | 259116 | 1408 | 0 |  | 4804 |  |
