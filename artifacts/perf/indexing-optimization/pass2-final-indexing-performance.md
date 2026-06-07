# Indexing Performance Report

- Label: `pass2-final`
- Project: `/Volumes/Explicit's Hard Drive/montage-index-perf-work/projects/montage-index-perf-pass2-final-66020-1780805088992698000`
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
- Slowest total: 136177 ms
- Slowest tool runtime: 73730 ms

## Pair Timings

| Indexer | Outcome | Total ms | Tool ms | Queue ms | Launch ms | Write ms | Frames | Output bytes | Perf phases |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| beats | wrote | 7943 | 7411 | 0 | 459 | 3 |  | 60027 |  |
| audio-energy | wrote | 12044 | 10134 | 0 | 1603 | 24 |  | 945560 |  |
| clip | wrote | 56059 | 40490 | 12048 | 2892 | 21 | 537 | 740874 | decode_read_ms=23055<br>frames_processed=537<br>inference_ms=14981<br>model_load_ms=2349<br>probe_ms=71 |
| face | wrote | 84085 | 73730 | 7944 | 2167 | 8 | 268 | 147407 | cluster_ms=7<br>decode_read_ms=3314<br>frames_processed=268<br>inference_ms=70227<br>probe_ms=80<br>setup_ms=68<br>speaker_map_ms=0<br>stitch_ms=0<br>summary_ms=0 |
| color-analysis | wrote | 93140 | 36060 | 56059 | 911 | 11 | 268 | 545527 | aggregate_ms=24<br>analysis_ms=782<br>decode_read_ms=35127<br>frames_processed=268<br>setup_ms=68 |
| frame-quality | wrote | 128772 | 44142 | 84086 | 437 | 7 | 1073 | 231152 | analysis_ms=2024<br>decode_read_ms=42119<br>frames_processed=1073<br>setup_ms=58 |
| scenedetect | wrote | 134205 | 40359 | 93143 | 609 | 1 | 1073 | 951 | analysis_ms=7<br>decode_read_ms=40012<br>frames_processed=1073<br>setup_ms=69 |
| gaze | wrote | 135163 | 16 | 134205 | 839 | 4 | 268 | 131166 |  |
| shot | wrote | 136177 | 581 | 135164 | 356 | 1 |  | 4802 |  |
