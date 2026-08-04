# Montage

Montage is a terminal-first, agent-native video editing harness: a Rust CLI/TUI, a Tauri desktop app, Python MCP indexers, and bundled editorial skills. An agent can inspect footage, reason about edits, propose timeline changes, and render previews.

This is a developer-preview source release for contributors who build from source. Signed macOS (Apple Silicon) installers are built by CI on `v*` tags; other platforms, Homebrew publishing, and auto-update are future work.

## Layout

- `crates/` — Rust workspace: CLI, TUI, core agent loop, config, MCP client, project protocol, rendering, indexing, desktop protocol.
- `apps/desktop/` — Tauri 2 desktop app with a React/Vite frontend.
- `python/` — `uv` workspace of MCP indexers: Whisper transcription, scene detection, audio energy, face/gaze detection, CLIP frame search, shot classification, color analysis.
- `skills/` — bundled editorial workflows exposed through `montage skills`.
- `docs/` — design notes and research.

Process and policy docs: `CONTRIBUTING.md`, `ARCHITECTURE.md`, `PRIVACY.md`, `SECURITY.md`, `CODE_OF_CONDUCT.md`, `THIRD_PARTY_NOTICES.md`, `CHANGELOG.md`.

## Prerequisites

- Rust toolchain with Cargo.
- Node.js and `pnpm` for the desktop frontend.
- Python 3.11 and `uv` for the Python indexers.
- `ffmpeg` on `PATH`.
- Tauri system dependencies for desktop development. On Linux, install the WebKit/AppIndicator packages shown in `.github/workflows/ci.yml`.
- `ANTHROPIC_API_KEY` for agent-backed commands and indexers that call Claude.
- `HF_TOKEN` for Whisper diarization workflows that use gated Hugging Face models.

## Quick Start

```bash
# build the Rust workspace
cargo check --workspace --all-targets

# create a project and import a source
cargo run -p montage-cli --bin montage -- new my-episode --import /path/to/video.mp4

# open the TUI agent on the project
cargo run -p montage-cli --bin montage -- tui my-episode

# store your Anthropic key in the OS keychain
printf '%s' "$ANTHROPIC_API_KEY" | cargo run -p montage-cli --bin montage -- secrets-set
```

## CLI

```bash
montage init <path>
montage new <name> --import <url-or-path>
montage validate <project>
montage index <project>
montage index-perf <project>
montage chat <project>
montage tui <project>
montage apply-edl <project> <edl>
montage render <project>
montage skills list
montage skills run <skill-name> <project>
montage lessons learn
montage lessons show
montage resume
montage version
```

During development, prefix commands with `cargo run -p montage-cli --bin montage --`.

## Desktop App

```bash
make desktop       # installs deps, fetches the yt-dlp and codex sidecars, runs pnpm tauri dev
make desktop-stop  # free the fixed Tauri dev port if it is busy
```

## Python Indexers

```bash
cd python
uv sync --all-packages
```

Montage resolves the Python workspace from `MONTAGE_PYTHON_ROOT`, by walking up from the binary/current directory in development, or from packaged install locations. Some indexers download large model weights on first use; see `python/SMOKE.md` for low-cost smoke testing.

## Development Checks

Run the narrowest lane that matches your change:

```bash
make check-app                # Montage-only Rust lane (normal app/core iteration)
make check-agent              # Codex auth / bridge / agent runner
make check-desktop-rust       # Tauri backend
cargo test -p <crate>         # targeted Rust tests
pnpm --dir apps/desktop test  # desktop frontend
```

Full workspace coverage, including the vendored Codex workspace:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo check --workspace --all-targets
```

`make check` runs the historical full gate (`cargo test --workspace`, vendored Codex tests included); reserve it for broad integration or release changes.

## Existing-sidecar Skip Benchmark

Measure the public index dispatcher when every indexer/asset pair already has a matching sidecar:

```bash
make perf-index-skip
```

For a controlled 12 assets × 3 indexers × 8 MiB sidecar run on the external drive, capture CPU, maximum RSS, page-fault, and filesystem-I/O evidence with macOS `time`:

```bash
CARGO_TARGET_DIR="/Volumes/My Passport for Mac/awidat-build/main-target" \
MONTAGE_INDEX_SKIP_WORK_DIR="/Volumes/My Passport for Mac/awidat-build/index-skip-perf" \
MONTAGE_INDEX_SKIP_OUTPUT="/Volumes/My Passport for Mac/awidat-build/index-skip-perf.json" \
MONTAGE_INDEX_SKIP_ARGS="--label controlled-12x3x8 --assets 12 --indexers 3 --sidecar-mib 8 --warmups 3 --samples 15" \
/usr/bin/time -l make perf-index-skip
```

The JSON report records the fixture configuration, machine facts, exact dispatch correctness counts, raw samples in milliseconds, and median/p95/MAD statistics. Before any warmup or timed sample, every new or reused fixture is checked against the current asset fingerprints, requested sidecar size, complete JSON shape, and indexer/asset header identity; stale or substituted fixture data fails closed instead of being timed. The `time -l` output supplies the CPU, peak-memory, page-fault, and filesystem-I/O evidence alongside that report.

## `Project::read` A/B Benchmark

`make perf-project-read` is a manual macOS/APFS controller, never a CI lane. Build and preserve a literal `release` `montage-project-read-perf` binary from each clean source tree; the harness source, build script, workspace and crate manifests, `.cargo/config.toml`, `Cargo.lock`, compiler identity, target, exact Cargo target-directory profile, and captured build-setting hash (Rust flags, wrappers, profile overrides, target settings, and native-tool flags) must match. A qualifying pair's tracked source snapshots must differ only at `crates/proto/src/project.rs`. The target deliberately builds nothing and canonicalizes both absolute executable paths before starting the candidate controller:

```bash
MONTAGE_PROJECT_READ_BASELINE=/absolute/baseline/montage-project-read-perf \
MONTAGE_PROJECT_READ_CANDIDATE=/absolute/candidate/montage-project-read-perf \
MONTAGE_PROJECT_READ_BASELINE_SOURCE=/absolute/baseline/awidat \
MONTAGE_PROJECT_READ_CANDIDATE_SOURCE=/absolute/candidate/awidat \
MONTAGE_PROJECT_READ_ARGS="--label read-a --work-dir /private/tmp/montage-project-read-a" \
make perf-project-read
```

It writes deterministic production `Project::write` fixtures at 100, 1,000, and 5,000 clips outside measurement, including a nonempty edit-plan item and index-manifest entry with fixed assets and timestamp. It then runs 3 warmups and 15 alternating, fresh-helper samples per arm. Helpers run with `LC_ALL=C` and `TZ=UTC`; each report records controller-observed end-to-end wrapper wall time and `/usr/bin/time -l` peak-RSS samples, input re-hashes, typed compact witnesses, unmeasured full typed signatures for every measured fixture, and canonical contracts for valid input, `Clip.99`, malformed marker recovery, and unknown-schema failure.

Run the full command twice independently. Each report has only a `single_report_gate_passed` field and can never itself assert program acceptance. A report must show no p95 latency regression at 100/1k/5k, plus either a 5k median-latency gain of at least 10% and 10 ms with non-worse 5k p95, or a 5k median-RSS gain of at least 25% and 10 MiB with non-worse 5k median/p95 latency. The 1k result is corroboration only. Verify the two absolute report paths together. The verifier re-hashes the preserved binaries and invokes their embedded identity protocol, re-hashes relevant source files, revalidates contracts/full typed witnesses, recomputes sample summaries and the frozen gate from raw evidence, enforces the alternating schedule, rejects identical raw measurement sets even if cosmetic report fields change, and requires distinct report/session IDs, generation times, project-source hashes, and matching methodology provenance, including OS, architecture, available parallelism, and work/report filesystem identities:

```bash
/absolute/candidate/montage-project-read-perf --verify-reports \
  /absolute/reports/first-project-read-ab.json \
  /absolute/reports/second-project-read-ab.json
```

For a non-qualifying baseline-vs-itself smoke on reduced inputs, make that intent explicit:

```bash
MONTAGE_PROJECT_READ_BASELINE=/absolute/candidate/montage-project-read-perf \
MONTAGE_PROJECT_READ_CANDIDATE=/absolute/candidate/montage-project-read-perf \
MONTAGE_PROJECT_READ_BASELINE_SOURCE=/absolute/candidate/awidat \
MONTAGE_PROJECT_READ_CANDIDATE_SOURCE=/absolute/candidate/awidat \
MONTAGE_PROJECT_READ_ARGS="--smoke --clips 10 --warmups 1 --samples 2" \
make perf-project-read
```

`--allow-dirty-source` is available only with `--smoke`; qualifying reports fail closed on any source-tree dirt.

## Waveform Benchmark

`make perf-waveform` creates a deterministic two-hour mixed-signal AAC/M4A fixture outside the measured helpers. Before the warmup or timed samples, an independent oracle directly decodes that fixture with FFmpeg to f32le and applies a benchmark-owned implementation of the production bucket semantics. The warmup and all seven fresh 2048-bucket helpers must match the oracle's exact duration bits, bucket bits, and canonical hash, as well as finite `[0,1]`, nonzero, mixed-signal, and one-8-kHz-sample duration checks. Separate probes require no-audio, bad-input, and live cancellation behavior.

Each wall sample starts before the helper process is spawned and ends when it exits, so it includes process and helper setup, decoder provenance lookup, Tokio runtime construction, the production `generate_waveform` call, correctness hashing, JSON serialization, and the atomic helper-result write. The production call dominates this wrapper work for the two-hour fixture, but short smoke runs should be interpreted as end-to-end helper overhead rather than decoder-only timing. The unique timestamped JSON report records generated UTC time, fixture-generator and helper-decoder provenance, executable/Cargo.lock/source hashes, Rust toolchain details, raw wall time, aggregate helper-plus-FFmpeg peak RSS, and maximum cumulative live-tree CPU time, with median/p95/MAD summaries; the recursive sampler targets 10 ms and rejects a run if any observed gap exceeds 100 ms. Disk-I/O accounting remains intentionally unavailable, so use platform tooling beside the report when required.

The Make target defaults fixture work and evidence to the external build drive. For a short internal APFS smoke while that drive is unavailable:

```bash
CARGO_TARGET_DIR=target \
MONTAGE_WAVEFORM_PERF_WORK_DIR=/private/tmp/montage-waveform-perf-smoke \
MONTAGE_WAVEFORM_PERF_EVIDENCE_DIR=/private/tmp/montage-waveform-perf-smoke/evidence \
MONTAGE_WAVEFORM_PERF_ARGS="--duration-s 12 --label internal-smoke" \
make perf-waveform
```

## Production Indexer Benchmarks

`make perf-audio-energy` and `make perf-clip-lifecycle` are manual, heavyweight performance workflows. They are not part of `make check` or normal CI. Each target performs one release build of `montage-index-perf` in the caller's `CARGO_TARGET_DIR` (or this checkout's `target/`), then gives its controller that exact executable.

The audio-energy controller runs the real dispatcher/indexer path against a deterministic mixed-signal M4A fixture. Its full default is a one-hour fixture, one warmup, and five timed samples, so run it manually only when that cost is intended. It needs the already prepared `audio-energy-mcp` Python environment, `uv`, `ffmpeg`, and `ffprobe`. It records end-to-end wall time (including dispatcher/process setup), sampled process-tree peak RSS, temporary-directory high water, and median/p95/MAD summaries. It rejects malformed or empty audio output and requires every timed canonical audio-energy payload to exactly match the warmup. A successful run writes a unique JSON evidence report below its work root's `evidence/` directory, retaining per-sample work and provenance for the binary, sources, tools, machine, and filesystems.

The CLIP lifecycle controller measures the real six-asset dispatcher path: one warmup and seven timed samples by default. It validates all six sidecars, asset fingerprints, exact `ViT-B-32/openai` model identity, 512-dimensional float16 embeddings, and stable semantic output against the warmup while excluding volatile timestamps and per-run timing fields from that comparison. Preflight separately verifies the imported `clip_mcp` model constants and pinned artifact. The controller removes inherited Python code paths, disables user-site imports and bytecode writes, rejects tracked, untracked, or unsafe ignored source inputs, and content-manifests the complete supplied Python workspace—including the ignored venv—for comparable evidence. A successful dispatcher exit is accepted only if its process group disappears naturally; forced cleanup still runs for hygiene but fails the sample. Its report also records sampled process-tree peak RSS, tool/runtime/model/source/filesystem provenance, and retained sample logs. Both reports are performance evidence only when the fixture/model/machine/workspace facts in their provenance are comparable.

CLIP is deliberately offline and pre-provisioned: `perf-clip-lifecycle` never runs `uv sync`, installs packages, or fetches/downloads a model. Before calling it, supply an existing isolated Python workspace in `MONTAGE_PYTHON_ROOT`, six distinct existing video paths in `MONTAGE_CLIP_ASSET_1` through `MONTAGE_CLIP_ASSET_6` (with distinct filenames), and `MONTAGE_CLIP_MODEL_WEIGHTS` pointing to the pinned Hugging Face snapshot symlink:

```text
<HF_HOME>/hub/models--timm--vit_base_patch32_clip_224.openai/snapshots/a6f597a30f7b82c51704746581f9a4e41421e878/open_clip_model.safetensors
```

That artifact is 605,143,284 bytes. The controller requires that exact snapshot layout and validates its SHA-256 (`e6d1bd7789aa45192b3bf90570a789b478bae1b74ebcce7eddd908e83a2b7c31`) before benchmarking.

For short, non-comparable smoke checks, retain the controllers' minimum five timed samples and use small disposable inputs:

```bash
CARGO_TARGET_DIR=target \
MONTAGE_AUDIO_ENERGY_PERF_ARGS="--duration-seconds 12 --samples 5 --work-root /private/tmp/montage-audio-energy-smoke --label smoke" \
make perf-audio-energy
```

```bash
export MONTAGE_PYTHON_ROOT=/absolute/path/to/montage/python
export MONTAGE_CLIP_MODEL_WEIGHTS=/absolute/path/to/huggingface/hub/models--timm--vit_base_patch32_clip_224.openai/snapshots/a6f597a30f7b82c51704746581f9a4e41421e878/open_clip_model.safetensors
export MONTAGE_CLIP_ASSET_1=/absolute/path/to/smoke-1.mp4
export MONTAGE_CLIP_ASSET_2=/absolute/path/to/smoke-2.mp4
export MONTAGE_CLIP_ASSET_3=/absolute/path/to/smoke-3.mp4
export MONTAGE_CLIP_ASSET_4=/absolute/path/to/smoke-4.mp4
export MONTAGE_CLIP_ASSET_5=/absolute/path/to/smoke-5.mp4
export MONTAGE_CLIP_ASSET_6=/absolute/path/to/smoke-6.mp4
MONTAGE_CLIP_LIFECYCLE_ARGS="--samples 5 --timeout-seconds 120 --work-root /private/tmp/montage-clip-lifecycle-smoke --label smoke" \
make perf-clip-lifecycle
```

Use `MONTAGE_AUDIO_ENERGY_PERF_ARGS` or `MONTAGE_CLIP_LIFECYCLE_ARGS` for the controllers' other manual options. The short examples exercise the full lifecycle but are intentionally not substitutes for the heavy default benchmark.

## macOS Consumer Releases

`.github/workflows/release.yml` builds a signed, notarized `Montage-aarch64-apple-darwin.dmg` and publishes it with its `.sha256` and `checksums.txt` as a GitHub release on `v*` tag pushes. Manual `workflow_dispatch` runs from a non-`v*` ref rehearse the build without publishing. The build is strict: missing Apple secrets, stub sidecars, or failed signing, notarization, or stapling fail the release.

Required GitHub Actions secrets: `APPLE_ID`, `APPLE_PASSWORD` (app-specific password with notarization access for the team), `APPLE_TEAM_ID`, `APPLE_CERTIFICATE` (base64-encoded Developer ID Application `.p12`), `APPLE_CERTIFICATE_PASSWORD`, `KEYCHAIN_PASSWORD`.

Local rehearsal for the current Mac target:

```sh
make desktop-yt-dlp
make desktop-codex
scripts/release/verify-sidecars.sh "$(rustc -vV | awk '/^host:/ { print $2 }')"
pnpm --dir apps/desktop tauri build --bundles dmg
```

## Privacy

Montage is local-first, but configured model providers, transcription services, generated-media providers, and publishing integrations can receive prompts, transcripts, audio, media-derived metadata, rendered files, or account metadata. Review `PRIVACY.md` before importing sensitive media or connecting external accounts.

## Configuration

User config lives in the platform config directory: `~/Library/Application Support/montage/` (macOS) or `~/.config/montage/` (Linux).

Environment variables:

- `ANTHROPIC_API_KEY` — Claude access for agent sessions and some indexers.
- `HF_TOKEN` — Hugging Face access for gated diarization models.
- `MONTAGE_PYTHON_ROOT` — override the bundled Python indexer workspace.
- `MONTAGE_SKILLS_ROOT` — override the bundled skills directory.

## License

Apache-2.0
