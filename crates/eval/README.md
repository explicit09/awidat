# Awidat evals

`awidat-eval` is the product regression harness for agent-native video
editing. It separates fast CI checks from slower/local validation so PRs
stay cheap while the product loop still measures edit quality.

## Tiers

- `cargo run -p awidat-eval -- --ci`
  Fast, deterministic, offline scenarios. Covers EDL parse/apply,
  structural proposal-diff surfaces, b-roll anchor handoff, same-position
  moves, vedit diff/recovery safety, approval/sandbox behavior, and a tiny
  sidecar parsing fixture.
- `cargo run -p awidat-eval -- --product`
  Offline product scenarios using synthetic timelines and sidecars. Covers
  dead-air quality, transcript-aware b-roll opportunity quality, concrete
  b-roll note anchors, and vedit recovery flows.
- `cargo run -p awidat-eval -- --golden`
  JSON-defined golden edit/cut cases under `fixtures/golden/`, including
  baseline grammar examples, genre variants, and rejection/failure repair
  cases.
- `cargo run -p awidat-eval -- --stress`
  Slow stress checks for large transcripts, malformed sidecars, recorder
  scale, and high-volume state.
- `cargo run -p awidat-eval -- --live`
  Real corpus/API-gated checks. These skip unless `AWIDAT_REAL_PROJECT` or
  `AWIDAT_REAL_CORPUS` points at an indexed project.

Default with no tier flags runs `--ci --product --golden`. `--all` runs
CI, product, golden, and stress; live remains opt-in.

Useful reporting flags:

```bash
cargo run -p awidat-eval -- --list
cargo run -p awidat-eval -- --ci --json
cargo run -p awidat-eval -- --live --fail-on-skip
```

Make aliases:

```bash
make eval
make eval-ci
make eval-stress
```

## Golden cases

Golden cases are small JSON fixtures:

- `project`: synthetic clips and placeholder assets.
- `objective`: the editorial goal.
- `edl`: expected edit envelope.
- `expect`: structural assertions, timing tolerance, required applied-op
  messages, forbidden operations, optional `cut_boundaries` assertions
  for semantic cut metadata, and optional `output_format` assertions for
  delivery constraints. Fixtures can also assert `package_metadata`
  platform/title fields and description/tag contains checks. Longer
  rough-assembly cases can include `proposal_history` entries so a
  rejected recommendation and accepted follow-up are explicit in the
  fixture and checked against the final EDL.
  Invalid-edit fixtures can use `error_contains` to assert the expected
  parser/apply rejection.

They intentionally avoid large media and downloads. Add large corpora as
live fixtures instead of committing them.

The checked-in default suite covers baseline editorial grammar plus
podcast dialogue, short-form hook, tutorial insert-repair, documentary
no-transition, multi-step dialogue cleanup, invalid metadata rejection,
rough short-form assembly delivery, rough assembly rejected/follow-up
history, and transition-overuse rejection variants.

## Real corpus fixtures

Use a local indexed project for live evaluation:

```bash
export AWIDAT_REAL_PROJECT=/absolute/path/to/indexed/project
cargo run -p awidat-eval -- --live
```

`AWIDAT_REAL_CORPUS` is accepted as an alias for scheduled runners that
mount a fixture corpus. Missing or invalid paths produce explicit skips,
not silent passes.

The live tier checks real transcript/search wiring and now includes an
`assess_edit_quality` visual-context scenario. When a real corpus is
configured, that scenario measures `index/shot` visual metadata coverage
before probing the assessor. Defaults require at least 3 shots with
composition or match metadata, at least 25% shot coverage, and at least
1 shot with generated match candidates. It also reports how many shots
carry `composition_source` values beginning with `model:`; set a
non-zero model-composition threshold when validating a corpus produced by
the real composition classifier. Override those gates with
`AWIDAT_REAL_VISUAL_MIN_METADATA_SHOTS`,
`AWIDAT_REAL_VISUAL_MIN_METADATA_RATIO`, and
`AWIDAT_REAL_VISUAL_MIN_MATCH_CANDIDATE_SHOTS`, plus
`AWIDAT_REAL_VISUAL_MIN_MODEL_COMPOSITION_SHOTS` for true model-backed
composition labels propagated into shot sidecars and
`AWIDAT_REAL_VISUAL_MIN_COMPOSITION_MODEL_REGIONS` for valid actual
`index/composition-model` region sidecars. Invalid model-region tolerance
defaults to zero and can be relaxed only through
`AWIDAT_REAL_VISUAL_MAX_INVALID_COMPOSITION_MODEL_REGIONS`. Numeric live
threshold environment variables must parse cleanly; invalid integers or
metadata ratios outside `[0, 1]` fail the eval instead of silently
falling back to defaults. The region count uses the same contract as
Python safe smoke: model source, valid time range, bounded confidence,
and controlled subject/depth/framing labels. The coverage summary
reports valid and invalid
composition-model region counts separately, so a corpus run can
distinguish missing model output from model output that failed the
sidecar contract. The Python safe-smoke preflight uses the same
distinction and includes sample path/reason diagnostics before the Rust
live eval runs. When project-tree thresholds are configured, the
preflight requires `AWIDAT_COMPOSITION_MODEL_PROJECT` or
`AWIDAT_REAL_CORPUS` instead of silently skipping the configured gate.
The workflow maps the same minimum-region and max-invalid settings into
`AWIDAT_COMPOSITION_MODEL_MIN_REGIONS` and
`AWIDAT_COMPOSITION_MODEL_MAX_INVALID_REGIONS`, and Python safe smoke
also accepts the real-corpus variable names as fallbacks. A real-corpus
minimum-region value of `0` keeps the Python preflight disabled, matching
the workflow condition. Otherwise the Python preflight and Rust live gate
honor the same rollout thresholds.

The live tier can also validate assessor-generated proposal lifecycle
fixtures against the mounted real project. Set
`AWIDAT_REAL_ASSESSOR_PROPOSAL_FIXTURE` to a JSON file or directory, or
place a single file at `.awidat/eval/assessor-proposal-flow.json` and
additional files under `.awidat/eval/assessor-proposals/*.json` in the
real project. Each fixture contains a final EDL plus `proposal_history`
entries and must include at least one rejected recommendation plus one
accepted follow-up. The eval applies every discovered final EDL to the
real timeline and checks that rejected proposal snippets are absent
while accepted follow-up snippets are present. In `proposal_history`,
`edl_contains` is status-aware: accepted entries must appear in the
final EDL, while rejected entries must not, and each history entry must
include at least one `edl_contains` snippet. Optional
`final_edl_must_contain` and `final_edl_must_not_contain` snippets must
also be non-empty, so fixture assertions cannot pass vacuously. A
checked-in sample lives at
`crates/eval/fixtures/real/assessor-proposal-flow.sample.json`; a
directory-layout sample also lives under
`crates/eval/fixtures/real/assessor-proposals/`. Unit coverage mounts
both shapes so the live scenario discovery path stays exercised. Set
`AWIDAT_REAL_MIN_ASSESSOR_PROPOSAL_FIXTURES` to require a minimum number
of discovered assessor proposal fixtures in a real-corpus run.

Transition decision fixtures promote the `transition_context` ->
`plan_transition` product flow into mounted real-project checks. Set
`AWIDAT_REAL_TRANSITION_PLANNER_FIXTURE` to a JSON file or directory, or
place a single file at `.awidat/eval/transition-planner-flow.json` and
additional files under `.awidat/eval/transition-planners/*.json` in the
real project. Each fixture names an adjacent clip boundary, optional
planner objective/direction, and expected recommendation fields plus EDL
fragment positive/negative snippets. Use `edl_contains` for required
planner EDL lines and `edl_must_not_contain` to prove hard-cut fixtures
did not smuggle in a visible transition. Hard-cut fixtures must prove
`*** Set Cut Intent`, `+ cut_type: hard_cut`, and forbidden
`*** Insert Transition` snippets. Visible-transition fixtures must name
`transition_id` and prove both `*** Insert Transition` and that
transition id in `edl_contains`. The eval runs
`transition_context`, feeds that packet to `plan_transition`, parses the
returned EDL fragment, and applies it against the mounted timeline. A
checked-in sample lives at
`crates/eval/fixtures/real/transition-planner-flow.sample.json`; a
directory-layout sample also lives under
`crates/eval/fixtures/real/transition-planners/`, covering both
hard-cut-default and visible motion-cover cases. Set
`AWIDAT_REAL_MIN_TRANSITION_PLANNER_FIXTURES` to require a minimum number
of discovered transition-planner fixtures in a real-corpus run.

Rough assembly flows can be promoted from golden fixtures into mounted
real-project checks the same way. Set `AWIDAT_REAL_ROUGH_ASSEMBLY_FIXTURE`
to a JSON file or directory, or place a single file at
`.awidat/eval/rough-assembly-flow.json` and additional files under
`.awidat/eval/rough-assemblies/*.json` in the real project. Each fixture
supplies a final EDL and expected clip ranges, cut boundaries, output
format, package metadata, applied-op snippets, forbidden ops, and
optional proposal history. Proposal-history final-EDL assertion snippets
must be non-empty just like assessor proposal fixtures. A checked-in
sample lives at
`crates/eval/fixtures/real/rough-assembly-flow.sample.json`; a
directory-layout sample also lives under
`crates/eval/fixtures/real/rough-assemblies/`. Set
`AWIDAT_REAL_MIN_ROUGH_ASSEMBLY_FIXTURES` to require a minimum number of
discovered rough assembly fixtures in a real-corpus run.

The dedicated `.github/workflows/evals.yml` workflow keeps these checks
out of PR CI:

- Weekly scheduled run: CI + product + golden + stress + Python safe smoke.
- Manual `run_audio_energy`: real Python `audio-energy-mcp` sidecar smoke
  through the Rust index dispatcher.
- Manual `run_live_agent`: ignored live/API Rust tests with
  `ANTHROPIC_API_KEY`.
- Manual `run_real_corpus`: `awidat-eval --live --fail-on-skip` on a
  self-hosted runner labeled `awidat-real-corpus`. The workflow forwards
  repository variables for the live visual gates (`AWIDAT_REAL_VISUAL_MIN_*`)
  and fixture gates (`AWIDAT_REAL_MIN_ASSESSOR_PROPOSAL_FIXTURES`,
  `AWIDAT_REAL_MIN_TRANSITION_PLANNER_FIXTURES`,
  `AWIDAT_REAL_MIN_ROUGH_ASSEMBLY_FIXTURES`). It refuses an empty
  `AWIDAT_REAL_CORPUS` path, and also requires that path to be an
  existing directory with `project.otio.json` on the self-hosted runner,
  before running optional sidecar preflight or live evals. Python safe
  smoke also counts mounted fixture files when any fixture minimum is
  non-zero, so undercovered real corpora fail before Rust parses and
  applies fixture contents. When
  `AWIDAT_REAL_VISUAL_MIN_COMPOSITION_MODEL_REGIONS` is non-zero, it also
  runs Python safe smoke against `AWIDAT_REAL_CORPUS` before the Rust live
  eval.

## Python smoke boundary

The default eval tier validates sidecar parsing through Rust fixtures.
The Python metadata/schema smoke is:

```bash
python3 python/scripts/smoke_indexers.py --safe
```

It checks workspace membership, package layout, indexer schema markers,
synthetic sidecar keys, composition-model sidecar contract fixtures, and
the eval workflow's real-corpus gate contract without importing heavy
indexer modules or downloading models.

The safe real-indexer smoke is:

```bash
python3 python/scripts/smoke_indexers.py --safe --audio-energy
```

That runs the ignored `awidat-index` end-to-end test for `audio-energy-mcp`
against a tiny WAV fixture. Full model-backed indexer execution remains
guarded and manual; see `python/SMOKE.md`.
