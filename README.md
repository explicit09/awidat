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

The JSON report records the fixture configuration, machine facts, exact dispatch correctness counts, raw samples in milliseconds, and median/p95/MAD statistics. The `time -l` output supplies the CPU, peak-memory, page-fault, and filesystem-I/O evidence alongside that report.

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
