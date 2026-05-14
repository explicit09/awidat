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
  JSON-defined golden edit/cut cases under `fixtures/golden/`.
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
  messages, and forbidden operations.

They intentionally avoid large media and downloads. Add large corpora as
live fixtures instead of committing them.

## Real corpus fixtures

Use a local indexed project for live evaluation:

```bash
export AWIDAT_REAL_PROJECT=/absolute/path/to/indexed/project
cargo run -p awidat-eval -- --live
```

`AWIDAT_REAL_CORPUS` is accepted as an alias for scheduled runners that
mount a fixture corpus. Missing or invalid paths produce explicit skips,
not silent passes.

The dedicated `.github/workflows/evals.yml` workflow keeps these checks
out of PR CI:

- Weekly scheduled run: CI + product + golden + stress + Python safe smoke.
- Manual `run_audio_energy`: real Python `audio-energy-mcp` sidecar smoke
  through the Rust index dispatcher.
- Manual `run_live_agent`: ignored live/API Rust tests with
  `ANTHROPIC_API_KEY`.
- Manual `run_real_corpus`: `awidat-eval --live --fail-on-skip` on a
  self-hosted runner labeled `awidat-real-corpus`.

## Python smoke boundary

The default eval tier validates sidecar parsing through Rust fixtures.
The Python metadata/schema smoke is:

```bash
python3 python/scripts/smoke_indexers.py --safe
```

It checks workspace membership, package layout, indexer schema markers,
and synthetic sidecar keys without importing heavy indexer modules or
downloading models.

The safe real-indexer smoke is:

```bash
python3 python/scripts/smoke_indexers.py --safe --audio-energy
```

That runs the ignored `awidat-index` end-to-end test for `audio-energy-mcp`
against a tiny WAV fixture. Full model-backed indexer execution remains
guarded and manual; see `python/SMOKE.md`.
