# Rendered-output acceptance

`awidat-eval --acceptance` is the opt-in behavioral video tier. It creates or
mounts media, applies an EDL, renders the timeline, and scores the MP4 with
deterministic media, timeline, artifact, and transcript gates. It stays out of
default evals and `--all` so PR checks remain cheap.

## Artifacts

Each run writes an artifact bundle under `target/awidat-eval/acceptance/` by
default:

- `project/` - generated or mounted Awidat project.
- `project/renders/*.mp4` - rendered timeline output.
- `artifacts/final_edl.edl` - EDL passed to the apply step.
- `artifacts/edit_manifest.json` - applied edits and kept source ranges.
- `artifacts/ffprobe.json` - stream and duration facts.
- `artifacts/silence.json` - `silencedetect` ranges at the fixture threshold.
- `artifacts/blackdetect.json` - black-frame/pathology detections.
- `artifacts/transcript.json` - optional transcript phrase evidence.
- `artifacts/edl_generator_stdout.txt` and `edl_generator_stderr.txt` -
  generator command output when a fixture uses `edl_generator.command`.
- `artifacts/artifact_bundle.json` - evidence inventory.
- `artifacts/scorecard.json` - pass/warn/fail verdict.

Useful overrides:

```bash
AWIDAT_EVAL_ARTIFACTS_DIR=/absolute/path cargo run -p awidat-eval -- --acceptance
AWIDAT_REAL_ACCEPTANCE_FIXTURE=/absolute/path/to/fixture-or-dir cargo run -p awidat-eval -- --acceptance
AWIDAT_ACCEPTANCE_CLI=/path/to/awidat cargo run -p awidat-eval -- --acceptance
```

`AWIDAT_ACCEPTANCE_CLI` forces the public product path:
`awidat apply-edl <project> <edl>` and `awidat render <project>`. Scorecards
record `edit_driver` and `render_driver` so each run says whether it used the
external CLI driver.

## Discovery

Discovery is read-only. It probes videos with `ffprobe`, samples each prefix
with FFmpeg `silencedetect`, scans existing Awidat Whisper sidecars under
`index/whisper/**/*.json`, and emits ranked candidate evidence.

```bash
mkdir -p target/awidat-eval/discovery
export AWIDAT_REAL_CORPUS=/absolute/path/to/local/video-corpus
cargo run -p awidat-eval -- \
  --acceptance-discover "$AWIDAT_REAL_CORPUS" \
  --acceptance-discover-write-drafts target/awidat-eval/local-fixtures/discovered-drafts/$(date -u +%Y%m%dT%H%M%SZ) \
  --json > target/awidat-eval/discovery/latest.json
```

Default discovery is bounded to 50 media files, depth 2, and 90 seconds per
file. Transcript-sidecar discovery is separately bounded to depth 6 and 100
sidecars. Shallow files are scanned before nested assets. Bare mid-phrase
`actually` markers are not promoted as false-start candidates because they are
often emphasis rather than restarts.

Media candidates with enough dead-air evidence include `fixture_draft`. The
optional `--acceptance-discover-write-drafts <dir>` flag writes one `.json`
file per draft plus `fixture_drafts_manifest.json`. Existing fixture files are
skipped instead of overwritten, while the manifest is refreshed for the latest
run. Those files are local authoring seeds because they contain absolute source
paths. Review the manifest, objective text, excerpt duration, and expected
removed ranges before promoting a draft to a durable local fixture.

The draft manifest is the first review surface in the real-corpus loop. Each
draft entry includes a `rank`, `scenario_type`, raw discovery `score`,
`ranking_signals`, and a human-readable `ranking_reason`. Ranking signals are
deterministic and currently include duration, sampled silence density, long
silence count and total, source-path confidence, transcript-cleanup evidence
availability, and expected usefulness. `source_path_confidence` is a local
review hint such as `absolute_existing` or `absolute_missing`; do not copy
machine-specific paths from draft manifests into checked-in fixtures.

Example manifest inspection:

```bash
jq '.drafts[] | {
  rank,
  id,
  scenario_type,
  expected_usefulness: .ranking_signals.expected_usefulness,
  silence_density: .ranking_signals.silence_density,
  source_path_confidence: .ranking_signals.source_path_confidence,
  reason: .ranking_reason
}' target/awidat-eval/local-fixtures/discovered-drafts/<timestamp>/fixture_drafts_manifest.json
```

Run a written draft like any other real fixture:

```bash
draft_dir=target/awidat-eval/local-fixtures/discovered-drafts/20260519T020151Z

AWIDAT_ACCEPTANCE_CLI=/path/to/awidat \
AWIDAT_REAL_ACCEPTANCE_FIXTURE=$draft_dir/candidate_short3_bullsemenmarketplace.json \
  cargo run -p awidat-eval -- --acceptance --json
```

## Real-Corpus Loop

Use this loop when you want to turn local videos into durable behavioral
fixtures without watching every full output.

1. Discover candidates and write local drafts:

```bash
export AWIDAT_REAL_CORPUS=/absolute/path/to/local/video-corpus
draft_dir=target/awidat-eval/local-fixtures/discovered-drafts/$(date -u +%Y%m%dT%H%M%SZ)
mkdir -p target/awidat-eval/discovery

cargo run -p awidat-eval -- \
  --acceptance-discover "$AWIDAT_REAL_CORPUS" \
  --acceptance-discover-write-drafts "$draft_dir" \
  --json > target/awidat-eval/discovery/latest.json
```

2. Inspect the ranked draft manifest:

```bash
jq '.drafts[] | {
  rank,
  id,
  scenario_type,
  usefulness: .ranking_signals.expected_usefulness,
  silence_density: .ranking_signals.silence_density,
  reason: .ranking_reason,
  fixture_path
}' "$draft_dir/fixture_drafts_manifest.json"
```

3. Run one selected draft through the public product path:

```bash
cargo build -p awidat-cli

AWIDAT_ACCEPTANCE_CLI=$PWD/target/debug/awidat \
AWIDAT_REAL_ACCEPTANCE_FIXTURE="$draft_dir/<fixture>.json" \
  cargo run -p awidat-eval -- --acceptance --json
```

4. Run a draft directory and write batch summaries:

```bash
mkdir -p target/awidat-eval/acceptance-batches
AWIDAT_ACCEPTANCE_CLI=$PWD/target/debug/awidat \
  cargo run -p awidat-eval -- \
  --acceptance-batch-summary "$draft_dir" \
  --json > target/awidat-eval/acceptance-batches/latest.json
```

The batch runner preserves the normal per-scenario acceptance artifact layout
under `target/awidat-eval/acceptance/` and also writes:

- `target/awidat-eval/acceptance-batches/<timestamp>/batch_summary.json`
- `target/awidat-eval/acceptance-batches/<timestamp>/batch_summary.md`

Each batch row includes fixture id, objective, scenario type, status, score,
scorecard path, rendered output path, artifact bundle path, and top failing
gates.

5. Review scorecards and package failures:

```bash
cargo run -p awidat-eval -- \
  --acceptance-package-failure target/awidat-eval/acceptance/<scenario>/<run>/artifacts/scorecard.json \
  --json
```

Failure packets are written under
`target/awidat-eval/failure-packets/<scenario-id>/<timestamp>/`. The packet
copies compact diagnostic artifacts such as `scorecard.json`,
`artifact_bundle.json`, `final_edl.edl`, `edit_manifest.json`, `ffprobe.json`,
`silence.json`, `blackdetect.json`, transcript evidence when present, and EDL
generator logs when present. Rendered MP4s are referenced by path by default
instead of copied, which keeps packets compact and avoids moving large media.

6. Promote a useful draft into a durable local fixture directory:

```bash
cargo run -p awidat-eval -- \
  --acceptance-promote-fixture "$draft_dir/<fixture>.json" \
  --to target/awidat-eval/local-fixtures/promoted-real-behavioral \
  --review-note "Reviewed scorecard and flagged timestamp clips." \
  --promotion-reason "Useful recurring dead-air cleanup fixture." \
  --acceptance-discovery-manifest "$draft_dir/fixture_drafts_manifest.json" \
  --json
```

Promotion refuses to overwrite an existing promoted fixture or review metadata
unless `--force` is explicit. It copies the fixture as-is and writes a
`*.review.json` sidecar with `reviewed_at`, source draft path, promoted fixture
path, scenario type, reviewer note, promotion reason, and original discovery
manifest path.

7. Rerun the promoted corpus:

```bash
AWIDAT_ACCEPTANCE_CLI=$PWD/target/debug/awidat \
AWIDAT_REAL_ACCEPTANCE_FIXTURE=target/awidat-eval/local-fixtures/promoted-real-behavioral \
  cargo run -p awidat-eval -- --acceptance --json
```

`pass` means all hard and soft gates passed. `warn` means deterministic hard
gates passed but at least one soft gate failed. `fail` means at least one hard
gate failed, such as render failure, missing audio/video, broken artifact
bundle, duration bounds, source-range checks, transcript checks, silence, or
black-frame pathology.

Hard gates are deterministic requirements that should block a fixture from
being trusted. Soft gates are semantic or subjective checks that can guide
review without overriding deterministic media and timeline evidence. Keep
semantic judges warn-only by default until the scenario factory has enough
known-good real-corpus history to calibrate them.

Humans should still review candidates before promotion, but the review should
focus on ranked manifests, scorecards, failing gates, and flagged timestamps or
short clips. Draft fixtures, batch artifacts, failure packets, promoted local
fixtures, and any machine-specific source paths should stay under
`target/awidat-eval/` or another ignored local artifact directory, not in git.

## Fixture Manifests

Use `--acceptance-fixture-manifest` when you want to inspect a real fixture
file or directory without rendering it:

```bash
cargo run -p awidat-eval -- \
  --acceptance-fixture-manifest target/awidat-eval/local-fixtures/real-behavioral \
  --json > target/awidat-eval/fixture-manifests/real-behavioral.json
```

The manifest uses the same runnable-fixture filtering as
`AWIDAT_REAL_ACCEPTANCE_FIXTURE`, so `*.template.json` and `*.sample.json` files
are ignored. It reports fixture ids, objectives, source paths, source duration,
whether the EDL is literal or generated, transcript phrase-check counts,
source-range expectation counts, and expected duration bounds. This is a cheap
handoff artifact for deciding which local real fixtures to render or promote.

## Fixture Shape

Checked-in examples live under `fixtures/acceptance/`. Real fixtures with
machine-specific `project.source_path` values should live outside git, for
example:

```bash
target/awidat-eval/local-fixtures/real-behavioral/
```

`AWIDAT_REAL_ACCEPTANCE_FIXTURE` accepts one JSON fixture or a directory of
JSON fixtures. Directory discovery ignores `*.template.json` and
`*.sample.json` examples, plus `*.review.json` promotion metadata sidecars.

Real fixture timing is excerpt-local. If `project.source_start_s` is `300.0`,
then `final_edl`, generated EDL output, `removed_source_ranges`,
`kept_source_ranges`, and transcript segments all start at `0.0` for the
transcoded excerpt.

Fixtures can embed `final_edl` or use `edl_generator.command`. Generator
commands run after project creation, receive `AWIDAT_ACCEPTANCE_PROJECT_ROOT`,
`AWIDAT_ACCEPTANCE_OBJECTIVE`, `AWIDAT_ACCEPTANCE_SOURCE_ASSET`, and
`AWIDAT_ACCEPTANCE_SOURCE_DURATION_S`, and must write the final EDL envelope to
stdout.

Common deterministic planners:

```bash
awidat plan-dead-air-edl "$AWIDAT_ACCEPTANCE_PROJECT_ROOT" \
  --min-duration-s 0.8 \
  --silence-threshold-db -40.0

awidat plan-transcript-setup-edl "$AWIDAT_ACCEPTANCE_PROJECT_ROOT"

awidat plan-transcript-remove-edl "$AWIDAT_ACCEPTANCE_PROJECT_ROOT" \
  --remove-phrase "awkward mistaken aside"

awidat plan-transcript-cleanup-edl "$AWIDAT_ACCEPTANCE_PROJECT_ROOT"

awidat plan-false-start-edl "$AWIDAT_ACCEPTANCE_PROJECT_ROOT"
```

When a fixture has `expect.transcript`, the runner materializes that evidence
into `project/index/whisper/` before invoking an EDL generator. Segment timings
are fixture evidence; generated word timings are evenly distributed inside each
segment only to make deterministic planner commands usable.

Useful expectation fields:

```json
{
  "removed_source_ranges": [
    {
      "start_s": 1.213,
      "end_s": 2.509,
      "reason": "detected dead air",
      "tolerance_s": 0.12
    }
  ],
  "kept_source_ranges": [
    {
      "start_s": 2.6,
      "end_s": 4.5,
      "reason": "spoken content that must survive",
      "tolerance_s": 0.12
    }
  ],
  "transcript": {
    "segments": [
      {
        "start_s": 13.58,
        "end_s": 18.58,
        "text": "biggest thing to all the founders out there..."
      }
    ],
    "must_preserve": ["get those papers"],
    "must_remove": ["weren't too passionate"]
  }
}
```

`removed_source_ranges` pass when final kept source ranges overlap the expected
removed span by no more than `tolerance_s`. `kept_source_ranges` pass when the
expected kept span is covered except for at most `tolerance_s`. Transcript
phrase checks use only transcript segments that overlap final kept source
ranges.

## Scorecards

Scorecard gates carry `severity`. Current render, media, timeline, artifact,
source-range, and transcript checks are hard gates. A hard gate failure sets the
scorecard status to `fail`. Future semantic judges can be soft gates; a failed
soft gate yields `warn` when hard gates still pass. A run with all gates passing
is `pass`.
