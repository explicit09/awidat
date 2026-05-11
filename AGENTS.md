# AGENTS.md

Guidance for coding agents working in this repository.

## Project Overview

Awidat is a terminal-first, agent-native video editing harness. The repo contains:

- Rust workspace crates under `crates/` for the CLI, TUI, core agent loop, project protocol, MCP client, config, indexing, rendering, and desktop protocol.
- A Tauri 2 desktop app under `apps/desktop/` with a React/Vite frontend and Rust backend.
- Python MCP indexers under `python/`, managed as a `uv` workspace.
- Bundled editorial skills under `skills/`.
- Packaging and install support under `dist/`.

Prefer reading the local code and docs before changing behavior. `README.md`, `dist/README.md`, and `python/SMOKE.md` are useful orientation docs.

## Common Commands

Run workspace checks:

```bash
make check
```

Run individual Rust checks:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Run the CLI during development:

```bash
cargo run -p awidat-cli -- <command>
```

Run the desktop app:

```bash
make desktop
```

Install/sync Python indexers:

```bash
cd python
uv sync --all-packages
```

Build a release tarball:

```bash
make package
```

## Rust Conventions

- The workspace uses Rust 2024 and forbids unsafe code through workspace lints.
- Most crates inherit strict workspace lints from `Cargo.toml`; keep code clippy-clean under `-D warnings`.
- Avoid `unwrap` and `expect`; the workspace denies them in most crates.
- Prefer existing crate boundaries:
  - `crates/proto` for project-format types and validation.
  - `crates/core` for agent/session/tool behavior.
  - `crates/config` for defaults and config loading.
  - `crates/mcp` for MCP client behavior.
  - `crates/render` for ffmpeg/rendering work.
  - `crates/index` for index orchestration.
  - `crates/cli` for CLI commands.
  - `crates/tui` for terminal UI.
- Keep public APIs documented where the crate already expects docs.

## Desktop Conventions

- Frontend code lives in `apps/desktop/src/`.
- Tauri backend code lives in `apps/desktop/src-tauri/src/`.
- Generated protocol TypeScript lives in `apps/desktop/src/protocol/generated/`; avoid editing generated files by hand unless the generation path is unavailable and the change is explicitly scoped.
- The desktop dev server uses Tauri's fixed Vite port `1420`; use `make desktop-stop` if it is occupied.
- `make desktop` also fetches the host `yt-dlp` sidecar binary expected by the Tauri bundle.

## Python Indexer Conventions

- Python packages live under `python/packages/*-mcp/`.
- Each indexer should be a `uv` workspace member in `python/pyproject.toml`.
- Use the shared `awidat-mcp` Python package for common MCP sidecar behavior.
- Heavy model downloads and gated model access are expected for some indexers; see `python/SMOKE.md` before adding broad smoke tests.
- `AWIDAT_PYTHON_ROOT` can override Python workspace discovery in development.

## Skills Conventions

- Bundled skills live under `skills/<name>/SKILL.md`.
- A bundled skills directory is identified by `skills/.bundled-marker`.
- `AWIDAT_SKILLS_ROOT` can override bundled skills discovery.
- Keep skill instructions direct and task-specific. Use scripts under a skill directory for repeatable analysis instead of embedding large procedural logic in prose.

## Testing Guidance

- For Rust-only changes, run the narrow relevant test first, then `cargo test --workspace` if the blast radius is broad.
- For lint-sensitive changes, run `cargo fmt --all -- --check` and clippy before handing off.
- For desktop UI changes, run or build the desktop app when feasible.
- For Python indexer changes, prefer targeted `uv run --package <package> ...` checks. Avoid triggering large model downloads unless the task requires it.
- For packaging changes, read `dist/README.md` and test with `make package` when practical.

## Operational Notes

- Required local tools commonly include Rust, Node.js, `pnpm`, Python 3.11, `uv`, and `ffmpeg`.
- Agent-backed commands and some indexers need `ANTHROPIC_API_KEY`.
- Some diarization flows need `HF_TOKEN` and accepted Hugging Face model terms.
- Keep unrelated worktree changes intact. Do not revert files you did not change unless explicitly asked.
- Keep edits scoped to the requested behavior and the subsystem you are touching.
