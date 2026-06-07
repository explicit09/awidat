# Montage

Montage is a terminal-first, agent-native video editing harness. It combines a Rust CLI/TUI, a Tauri desktop app, Python MCP indexers, and bundled editorial skills so an agent can inspect footage, reason about edits, propose timeline changes, and render previews.

The project is early-stage and optimized for local development on macOS and Linux.

## What is in this repo

- `crates/` - Rust workspace crates for the CLI, TUI, core agent loop, config, MCP client, project protocol, rendering, indexing, and desktop protocol.
- `apps/desktop/` - Tauri 2 desktop app with a React/Vite frontend.
- `python/` - `uv` workspace for MCP indexers such as Whisper transcription, scene detection, audio energy, face/gaze detection, CLIP frame search, shot classification, and color analysis.
- `skills/` - bundled editorial workflows exposed through `montage skills`.
- `docs/` - design notes and research.

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

## Python Indexers

The Python indexers live in a `uv` workspace under `python/`:

```bash
cd python
uv sync --all-packages
```

Montage resolves the Python workspace from `MONTAGE_PYTHON_ROOT`, by walking up from the binary/current directory in development, or from packaged install locations. Most projects can use the bundled defaults without writing custom MCP config.

Some indexers download large model weights on first use. See `python/SMOKE.md` for low-cost smoke testing and notes on model/API-key requirements.

## Development Checks

For compile, lint, and formatting coverage:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo check --workspace --all-targets
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
tests, so use it when you are intentionally validating the vendored harness;
otherwise run the narrower command that matches the changed subsystem.

## Packaging

Release packaging is not currently restored in this checkout. The historical
`dist/` scripts referenced by older automation are absent, so use development
commands until the release path is rebuilt.

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
