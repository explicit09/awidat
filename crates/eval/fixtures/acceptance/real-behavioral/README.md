# Real-Behavioral Acceptance Fixtures

This directory documents the durable layout for real-video rendered-output
acceptance cases. Do not commit large media or machine-specific source paths
here. Keep runnable real fixtures in a local or mounted directory and point the
acceptance runner at that directory:

```bash
mkdir -p target/awidat-eval/local-fixtures/real-behavioral
AWIDAT_REAL_ACCEPTANCE_FIXTURE=$PWD/target/awidat-eval/local-fixtures/real-behavioral \
  cargo run -p awidat-eval -- --acceptance --json
```

Runnable fixture files must end in `.json`. Committed examples should end in
`.template.json` or `.sample.json`; directory discovery intentionally ignores
those names so examples can live beside runnable fixtures without being
executed.

Use `scenario.template.json` as the starting contract. The most important
fields are:

- `project.source_path`: absolute path to a local source video.
- `project.source_start_s` and `project.source_duration_s`: the excerpt to
  transcode into the generated project.
- `final_edl`: the edit to apply before rendering, or `edl_generator.command`
  when the scenario should get the EDL from an external command.
- `removed_source_ranges`: excerpt-local source spans that should be absent
  from the final kept timeline.
- `kept_source_ranges`: excerpt-local source spans that must survive.
- `transcript`: optional excerpt-local transcript evidence for required and
  removed phrases.

All source-range and transcript timings are local to the transcoded excerpt.
If the excerpt starts at `project.source_start_s = 300.0`, fixture time `0.0`
means original-source time `300.0`.

When using `edl_generator.command`, the command must write an EDL envelope to
stdout. The runner provides `AWIDAT_ACCEPTANCE_PROJECT_ROOT`,
`AWIDAT_ACCEPTANCE_OBJECTIVE`, `AWIDAT_ACCEPTANCE_SOURCE_ASSET`, and
`AWIDAT_ACCEPTANCE_SOURCE_DURATION_S` in the environment. Generator stdout and
stderr are saved into the artifact bundle.

For dead-air cleanup scenarios, the generator can be Awidat itself:

```json
{
  "edl_generator": {
    "command": [
      "/bin/sh",
      "-c",
      "exec \"${AWIDAT_ACCEPTANCE_CLI:-/path/to/awidat}\" plan-dead-air-edl \"$AWIDAT_ACCEPTANCE_PROJECT_ROOT\" --min-duration-s 0.8 --silence-threshold-db -40.0"
    ]
  }
}
```

That command emits the final EDL to stdout; the same `AWIDAT_ACCEPTANCE_CLI`
value can then drive `awidat apply-edl` and `awidat render`.

Fixtures with `expect.transcript` also get a generated Whisper-style sidecar
inside the run project. Product commands can use that transcript path directly.
For example, a setup-removal scenario can keep the first segment containing a
known advice phrase:

```json
{
  "edl_generator": {
    "command": [
      "/bin/sh",
      "-c",
      "exec \"${AWIDAT_ACCEPTANCE_CLI:-/path/to/awidat}\" plan-transcript-trim-edl \"$AWIDAT_ACCEPTANCE_PROJECT_ROOT\" --keep-from-phrase \"get those papers\""
    ]
  }
}
```

For a more autonomous deterministic setup-removal scenario, use the planner
that scans transcript segments for actionable advice cues:

```json
{
  "edl_generator": {
    "command": [
      "/bin/sh",
      "-c",
      "exec \"${AWIDAT_ACCEPTANCE_CLI:-/path/to/awidat}\" plan-transcript-setup-edl \"$AWIDAT_ACCEPTANCE_PROJECT_ROOT\""
    ]
  }
}
```

For an internal transcript-removal scenario, remove the segment span containing
one or more unwanted phrases while preserving material before and after it:

```json
{
  "edl_generator": {
    "command": [
      "/bin/sh",
      "-c",
      "exec \"${AWIDAT_ACCEPTANCE_CLI:-/path/to/awidat}\" plan-transcript-remove-edl \"$AWIDAT_ACCEPTANCE_PROJECT_ROOT\" --remove-phrase \"awkward mistaken aside\""
    ]
  }
}
```

This planner is segment-based. Keep `removed_source_ranges` aligned to the
whole transcript segment span that should disappear, not just the exact words
inside the phrase.

For autonomous filler cleanup, let Awidat choose filler-heavy transcript
segments itself:

```json
{
  "edl_generator": {
    "command": [
      "/bin/sh",
      "-c",
      "exec \"${AWIDAT_ACCEPTANCE_CLI:-/path/to/awidat}\" plan-transcript-cleanup-edl \"$AWIDAT_ACCEPTANCE_PROJECT_ROOT\""
    ]
  }
}
```

The default threshold is `--min-filler-ratio 0.35 --min-filler-tokens 2`.
This is also segment-based. Use it for fixtures where the unwanted material is
a transcript segment dominated by filler or discourse markers, and set the
source-range expectations to that whole segment.

For false-start cleanup, let Awidat detect restart markers from Whisper-style
word timings:

```json
{
  "edl_generator": {
    "command": [
      "/bin/sh",
      "-c",
      "exec \"${AWIDAT_ACCEPTANCE_CLI:-/path/to/awidat}\" plan-false-start-edl \"$AWIDAT_ACCEPTANCE_PROJECT_ROOT\""
    ]
  }
}
```

The planner shares detection with `find_false_starts` and currently removes the
fragment before `wait`, `actually`, or `let me` restart markers. Acceptance
fixtures materialize `expect.transcript` as generated word timings, so keep the
transcript segment boundaries tight around the false-start fragment when using
this planner.

The artifact bundle for each run lands under:

```text
target/awidat-eval/acceptance/<scenario-id>/<timestamp>-p<PID>-<counter>/
```

The main handoff files are `artifacts/scorecard.json` and
`artifacts/artifact_bundle.json`. The scorecard records `edit_driver` and
`render_driver`; set `AWIDAT_ACCEPTANCE_CLI=/path/to/awidat` when you want the
fixture EDL to go through `awidat apply-edl` and the export to go through
`awidat render` instead of the in-process eval driver.
