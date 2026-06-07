# Indexing Performance Review

Montage has a dedicated CLI path for indexing performance review:

```bash
cargo run -p montage-cli --bin montage -- index-perf /path/to/project
```

The command runs configured indexers, captures per-pair dispatcher telemetry in
milliseconds, compares each pair to initial targets, and writes:

```text
<project>/reports/indexing-performance/indexing-performance.json
<project>/reports/indexing-performance/indexing-performance.md
```

## Default Scope

`index-perf` excludes `whisper` by default. Transcription is model-heavy and
should be measured separately from the faster context indexers unless the review
explicitly asks for it.

Use `--include-whisper` when transcription is part of the review:

```bash
cargo run -p montage-cli --bin montage -- index-perf /path/to/project --include-whisper
```

Use indexer filters to focus a run:

```bash
cargo run -p montage-cli --bin montage -- index-perf /path/to/project \
  --indexer audio-energy \
  --indexer scenedetect \
  --indexer frame-quality
```

Use exclusions to keep one expensive or irrelevant indexer out of a run:

```bash
cargo run -p montage-cli --bin montage -- index-perf /path/to/project \
  --exclude-indexer clip
```

## Real Video Review Set

For local performance runs, a useful real-video corpus exists at:

```text
/Users/explicit/Projects/video-editor/VideoEditor/Tools/eval_corpus/public_seed/
```

Suggested first pass:

```bash
cargo run -p montage-cli --bin montage -- index-perf /path/to/project \
  --asset /Users/explicit/Projects/video-editor/VideoEditor/Tools/eval_corpus/public_seed/varied_720p_24fps_30s.mp4 \
  --asset /Users/explicit/Projects/video-editor/VideoEditor/Tools/eval_corpus/public_seed/varied_1080p_60fps_20s.mp4 \
  --asset /Users/explicit/Projects/video-editor/VideoEditor/Tools/eval_corpus/public_seed/varied_4k_30fps_10s.mp4 \
  --output /path/to/project/reports/indexing-performance
```

The CLI accepts explicit assets outside `<project>/raw/`; their report IDs are
derived from the path relative to the project when possible, otherwise from the
absolute path.

## Report Fields

Each pair records:

- `queued_ms`: scheduler wait after hashing.
- `launch_init_ms`: child process launch plus MCP initialize.
- `tool_ms`: `index_asset` runtime.
- `write_ms`: JSON serialization and sidecar write.
- `total_ms`: complete pair wall time.
- `peak_rss_bytes`: best-effort child RSS when available.

The report also records command metadata, selected indexers, excluded indexers,
asset IDs, operating system, architecture, available parallelism, and an
`average` or `powerful` machine profile.

## Targets

Targets are review guidance, not CI gates. A budget violation marks the pair as
`review` in Markdown and increments `budget_violations` in JSON, but the command
only exits non-zero when indexing itself fails.

Initial targets are intentionally conservative. Tighten them after several real
runs on both average and powerful machines.
