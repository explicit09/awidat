# Indexing Performance Review Design

## Goal

Build a repeatable way to measure Awidat indexing speed in milliseconds, compare
results against explicit targets, and produce a report the team can use for
performance reviews on average and powerful machines.

## Scope

- Measure non-transcription indexing by default.
- Keep whisper/transcription out of the default run because it is model-heavy
  and currently not the target of this review.
- Report per-indexer and per-asset durations in milliseconds.
- Include end-to-end dispatcher duration, write time, launch/init time, queue
  time, and tool runtime from existing `PairTelemetry`.
- Include built-in pass timings when they are part of the indexing path, notably
  motion and silence.
- Record machine profile metadata so results can be compared across average and
  powerful machines.
- Output both JSON and Markdown so reports are machine-readable and easy to
  review.

## Non-Goals

- No desktop UI in this pass.
- No hard CI gate from real media timings yet. Real video timing varies too much
  by hardware and local model/cache state.
- No bundled private media fixtures. Local videos can be used for manual report
  runs, but committed tests should use synthetic data or constructed reports.
- No transcription performance target in the default profile.

## Proposed Workflow

Add a CLI performance-review path that runs indexing over selected project
assets, filters out `whisper` unless explicitly requested, and writes:

- `<output>/indexing-performance.json`
- `<output>/indexing-performance.md`

The command should be explicit, for example:

```bash
cargo run -p awidat-cli -- index-perf /path/to/project \
  --asset /path/to/project/raw/varied_720p_24fps_30s.mp4 \
  --output /path/to/project/reports/indexing-perf \
  --exclude-indexer whisper
```

The report compares measurements to a small target table. Initial targets are
conservative and can be revised after the first real runs:

- `queue_ms`: budget for scheduler delay after hashing.
- `launch_init_ms`: budget for child process spawn and MCP initialization.
- `tool_ms`: budget for the indexer's main work.
- `write_ms`: budget for sidecar serialization and disk write.
- `total_ms`: budget for the complete pair.

Targets are informational by default. The command exits non-zero only when
indexing itself fails. A future `--fail-on-budget` can turn targets into CI
gates once the team has enough baseline data.

## Architecture

- Keep `awidat_index` responsible for telemetry aggregation and report model
  helpers because it already owns `PairTelemetry` and `IndexReport`.
- Keep `awidat-cli` responsible for command-line parsing, config loading, asset
  selection, filesystem output, and human-facing Markdown.
- Add a small reusable formatter instead of pushing timing output into the
  normal `awidat index` command. Normal indexing should stay concise.
- Time built-in passes through a small report type if they are included in the
  command path. MCP pair timings remain sourced from `PairTelemetry`.

## Real-Asset Review Set

Local candidate videos were found at:

```text
/Users/explicit/Projects/video-editor/VideoEditor/Tools/eval_corpus/public_seed/
```

That corpus has useful short samples, including 720p, 1080p, 4k, vertical,
square, noisy-audio, silence-gap, and scene-heavy clips. It is a better manual
default than random Downloads files because the names encode duration and
format.

## Acceptance Criteria

- A focused unit test can construct pair telemetry and verify target evaluation.
- A focused unit test can verify Markdown and JSON report content.
- The CLI exposes an explicit performance-review command.
- The default command excludes `whisper`.
- The report records milliseconds, target milliseconds, pass/fail per timing
  field, machine profile, asset list, indexer list, and command metadata.
- Verification includes Rust formatting and targeted CLI/index tests.
