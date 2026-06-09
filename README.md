# Montage

Montage is a terminal-first, agent-native video editing harness. It combines a Rust CLI/TUI, a Tauri desktop app, Python MCP indexers, and bundled editorial skills so an agent can inspect footage, reason about edits, propose timeline changes, and render previews.

This repository is a developer-preview source release. It is intended for
contributors who can build and run the project from source. The macOS consumer
installer track now builds signed and notarized DMGs from GitHub Actions on
`v*` tags; Linux packages, Windows installers, Homebrew publishing, auto-update,
and broader bundled runtime polish remain future release work.

## What is in this repo

- `crates/` - Rust workspace crates for the CLI, TUI, core agent loop, config, MCP client, project protocol, rendering, indexing, and desktop protocol.
- `apps/desktop/` - Tauri 2 desktop app with a React/Vite frontend.
- `python/` - `uv` workspace for MCP indexers such as Whisper transcription, scene detection, audio energy, face/gaze detection, CLIP frame search, shot classification, and color analysis.
- `skills/` - bundled editorial workflows exposed through `montage skills`.
- `docs/` - design notes and research.

## Project Documents

- `CONTRIBUTING.md` - contribution setup, checks, and pull request guidance.
- `CODE_OF_CONDUCT.md` - expected community behavior.
- `SECURITY.md` - private vulnerability reporting process.
- `PRIVACY.md` - local-first scope and data-egress disclosure.
- `THIRD_PARTY_NOTICES.md` - third-party license and provenance notes.
- `ARCHITECTURE.md` - high-level source layout and data flow.
- `CHANGELOG.md` - release notes and unreleased changes.

## Prerequisites

- Rust toolchain with Cargo.
- Node.js and `pnpm` for the desktop frontend.
- Python 3.11 and `uv` for the Python indexers.
- `ffmpeg` on `PATH` for media probing, indexing, and rendering.
- Tauri system dependencies for desktop development. On Linux, install the WebKit/AppIndicator packages shown in `.github/workflows/ci.yml`.
- `ANTHROPIC_API_KEY` for agent-backed commands and indexers that call Claude.
- `HF_TOKEN` for Whisper diarization workflows that use gated Hugging Face models.

## Quick Start

Build the Rust workspace:

```bash
cargo check --workspace --all-targets
```

Create a project and import a source:

```bash
cargo run -p montage-cli --bin montage -- new my-episode --import /path/to/video.mp4
```

Open the TUI agent on a project:

```bash
cargo run -p montage-cli --bin montage -- tui my-episode
```

Store your Anthropic key in the OS keychain:

```bash
printf '%s' "$ANTHROPIC_API_KEY" | cargo run -p montage-cli --bin montage -- secrets-set
```

## CLI Commands

Common commands:

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

During development, prefix commands with:

```bash
cargo run -p montage-cli --bin montage --
```

## Desktop App

Install frontend dependencies and run the Tauri app:

```bash
make desktop
```

This installs `apps/desktop` dependencies, fetches the host `yt-dlp` sidecar binary into `apps/desktop/src-tauri/binaries/`, and starts `pnpm tauri dev`.

If the fixed Tauri dev port is busy:

```bash
make desktop-stop
```

## macOS consumer releases

Strict macOS consumer releases are built by `.github/workflows/release.yml`.
The workflow runs on `v*` tags and creates notarized DMGs for:

- `aarch64-apple-darwin`
- `x86_64-apple-darwin`

Tag pushes publish GitHub releases; manual `workflow_dispatch` runs from a
non-`v*` branch or ref can rehearse the build path without publishing release
assets.

Required GitHub Actions secrets:

- `APPLE_ID`
- `APPLE_PASSWORD`
- `APPLE_TEAM_ID`
- `APPLE_CERTIFICATE`
- `APPLE_CERTIFICATE_PASSWORD`
- `KEYCHAIN_PASSWORD`

`APPLE_CERTIFICATE` must be a base64-encoded Developer ID Application `.p12`.
`APPLE_PASSWORD` should be an Apple app-specific password with notarization
access for `APPLE_TEAM_ID`. CI currently uses supported macOS 15 runners for
Apple Silicon and Intel release builds.

Local rehearsal for the current Mac target:

```sh
make desktop-yt-dlp
make desktop-codex
scripts/release/verify-sidecars.sh "$(rustc -vV | awk '/^host:/ { print $2 }')"
pnpm --dir apps/desktop tauri build --bundles dmg
```

CI release builds are strict: missing Apple secrets, stub sidecars, failed
signing, failed notarization, or failed stapling all fail the release.
Publishing also requires exactly these release assets:

- `Montage-aarch64-apple-darwin.dmg`
- `Montage-aarch64-apple-darwin.dmg.sha256`
- `Montage-x86_64-apple-darwin.dmg`
- `Montage-x86_64-apple-darwin.dmg.sha256`
- `checksums.txt`

## Python Indexers

The Python indexers live in a `uv` workspace under `python/`:

```bash
cd python
uv sync --all-packages
```

Montage resolves the Python workspace from `MONTAGE_PYTHON_ROOT`, by walking up from the binary/current directory in development, or from packaged install locations. Most projects can use the bundled defaults without writing custom MCP config.

Some indexers download large model weights on first use. See `python/SMOKE.md` for low-cost smoke testing and notes on model/API-key requirements.

## Privacy and data egress

Montage is local-first, but configured model providers, transcription services,
generated-media providers, and publishing integrations can receive prompts,
transcripts, audio, media-derived metadata, rendered files, or account metadata.
Review `PRIVACY.md` before importing sensitive media or connecting external
accounts.

## Development Checks

For normal app/core iteration, use the Montage-only Rust lane:

```bash
make check-app
```

Use the heavier lanes only when you touch the corresponding surface:

```bash
make check-agent        # Codex auth / bridge / agent runner
make check-desktop-rust # Tauri backend
```

For full workspace compile, lint, and formatting coverage, including the
vendored Codex workspace:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo check --workspace --all-targets
```

For targeted Rust tests, prefer the touched crate first:

```bash
cargo test -p <crate>
```

For the desktop frontend:

```bash
cd apps/desktop
pnpm test
```

For Python indexer setup and the in-tree MCP package:

```bash
cd python
uv sync --all-packages
uv run python -c "import montage_mcp"
```

`make check` still runs the historical full Rust gate
(`cargo test --workspace`). In this checkout that includes vendored Codex
tests, so use it when you are intentionally validating the vendored harness
or preparing a broad integration/release change; otherwise run the narrower
Makefile lane that matches the changed subsystem.

## Packaging

macOS consumer DMG packaging is handled by the strict GitHub Actions release
workflow described above. The historical `dist/` scripts referenced by older
automation are absent; Linux packages, Windows installers, Homebrew publishing,
and auto-update remain future release work.

## Configuration

Montage reads user config from the standard platform config directory, usually:

- macOS: `~/Library/Application Support/montage/`
- Linux: `~/.config/montage/`

Useful environment variables:

- `ANTHROPIC_API_KEY` - Claude access for agent sessions and some indexers.
- `HF_TOKEN` - Hugging Face access for gated diarization models.
- `MONTAGE_PYTHON_ROOT` - override bundled Python indexer workspace.
- `MONTAGE_SKILLS_ROOT` - override bundled skills directory.

## License

Apache-2.0
