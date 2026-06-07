# Indexing Performance Report

- Label: `pass2-face-first`
- Project: `/Volumes/Explicit's Hard Drive/montage-index-perf-work/projects/montage-index-perf-pass2-face-first-77501-1780804365175127000`
- Source: `/Volumes/Explicit's Hard Drive/Episode3_AI_Regulation_IPOs_Elons_Chip_Play.mp4`
- Duration: 1073.000s
- Resolution: 1280x720
- Video codec: `h264`
- FPS: `30/1`
- File size: 1007682712 bytes
- Concurrency: 2
- Indexers: audio-energy, beats, face, clip, scenedetect, frame-quality, color-analysis, gaze, shot

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
- Slowest total: 204642 ms
- Slowest tool runtime: 90611 ms

## Pair Timings

| Indexer | Outcome | Total ms | Tool ms | Queue ms | Launch ms | Write ms | Frames | Output bytes | Perf phases |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| beats | wrote | 16141 | 15362 | 0 | 696 | 6 |  | 60027 |  |
| audio-energy | wrote | 21612 | 19799 | 0 | 1404 | 77 |  | 945560 |  |
| clip | wrote | 74542 | 48549 | 21625 | 3530 | 102 | 537 | 740874 | decode_read_ms=28066<br>frames_processed=537<br>inference_ms=17781<br>model_load_ms=2556<br>probe_ms=75 |
| face | wrote | 109224 | 90611 | 16142 | 2036 | 6 | 268 | 147408 | cluster_ms=14<br>decode_read_ms=4422<br>frames_processed=268<br>inference_ms=85941<br>probe_ms=90<br>setup_ms=68<br>speaker_map_ms=0<br>stitch_ms=0<br>summary_ms=0 |
| frame-quality | wrote | 122113 | 46296 | 74542 | 1017 | 10 | 1073 | 231152 | analysis_ms=1921<br>decode_read_ms=44324<br>frames_processed=1073<br>setup_ms=80 |
| scenedetect | wrote | 191866 | 81730 | 109225 | 621 | 2 | 1073 | 952 | analysis_ms=13<br>decode_read_ms=81355<br>frames_processed=1073<br>setup_ms=66 |
| gaze | wrote | 195565 | 75 | 191866 | 3226 | 5 | 268 | 131166 |  |
| shot | wrote | 199132 | 1787 | 195566 | 1359 | 8 |  | 4804 |  |
| color-analysis | wrote | 204642 | 81585 | 122114 | 560 | 15 | 268 | 545527 | aggregate_ms=28<br>analysis_ms=780<br>decode_read_ms=80613<br>frames_processed=268<br>setup_ms=73 |
