# Awidat

Awidat is a terminal-first, agent-native video editing harness. It combines a Rust CLI/TUI, a Tauri desktop app, Python MCP indexers, and bundled editorial skills so an agent can inspect footage, reason about edits, propose timeline changes, and render previews.

The project is early-stage and optimized for local development on macOS and Linux.

## What is in this repo

- `crates/` - Rust workspace crates for the CLI, TUI, core agent loop, config, MCP client, project protocol, rendering, indexing, and desktop protocol.
- `apps/desktop/` - Tauri 2 desktop app with a React/Vite frontend.
- `python/` - `uv` workspace for MCP indexers such as Whisper transcription, scene detection, audio energy, face/gaze detection, CLIP frame search, shot classification, and color analysis.
- `skills/` - bundled editorial workflows exposed through `awidat skills`.
- `dist/` - packaging, install, and Homebrew release support.
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

Build and test the Rust workspace:

```bash
cargo test --workspace
```

Create a project and import a source:

```bash
cargo run -p awidat-cli -- new my-episode --import /path/to/video.mp4
```

Open the TUI agent on a project:

```bash
cargo run -p awidat-cli -- tui my-episode
```

Store your Anthropic key in the OS keychain:

```bash
printf '%s' "$ANTHROPIC_API_KEY" | cargo run -p awidat-cli -- secrets-set
```

## CLI Commands

Common commands:

```bash
awidat init <path>
awidat new <name> --import <url-or-path>
awidat validate <project>
awidat index <project>
awidat chat <project>
awidat tui <project>
awidat skills list
awidat skills run <skill-name> <project>
awidat lessons learn
awidat lessons show
awidat resume
awidat upgrade
```

During development, prefix commands with:

```bash
cargo run -p awidat-cli --
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

Awidat resolves the Python workspace from `AWIDAT_PYTHON_ROOT`, by walking up from the binary/current directory in development, or from packaged install locations. Most projects can use the bundled defaults without writing custom MCP config.

Some indexers download large model weights on first use. See `python/SMOKE.md` for low-cost smoke testing and notes on model/API-key requirements.

## Development Checks

Run the same checks used by the Makefile:

```bash
make check
```

Or run them individually:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## Packaging

Build a host release tarball:

```bash
make package
```

Output is written under `dist/build/`. For the distribution model, local install testing, and release flow, see `dist/README.md`.

## Configuration

Awidat reads user config from the standard platform config directory, usually:

- macOS: `~/Library/Application Support/awidat/`
- Linux: `~/.config/awidat/`

Useful environment variables:

- `ANTHROPIC_API_KEY` - Claude access for agent sessions and some indexers.
- `HF_TOKEN` - Hugging Face access for gated diarization models.
- `AWIDAT_PYTHON_ROOT` - override bundled Python indexer workspace.
- `AWIDAT_SKILLS_ROOT` - override bundled skills directory.

## License

Apache-2.0
