# AGENTS.md

Awidat is a Rust workspace with a Tauri desktop app, Python `uv` MCP indexers,
and bundled editorial skills. Read local docs before changing behavior,
especially `README.md`, `python/SMOKE.md`, and focused docs under `docs/`.

## Agent Behavior

- Think before coding. State assumptions before non-obvious choices; if multiple interpretations exist, surface the tradeoff instead of picking silently. If something is unclear, stop and ask.
- Simplicity first. Write the minimum code that solves the problem: no speculative features, no single-use abstractions, no unrequested flexibility/configurability, and no error handling for impossible scenarios. If 200 lines could be 50, rewrite it. Ask whether a senior engineer would call the solution overcomplicated.
- Make surgical changes. Touch only files and lines needed for the request, match existing style, and avoid opportunistic refactors, formatting churn, or adjacent cleanup.
- Clean up only artifacts introduced by your change, such as now-unused imports, variables, functions, or tests. Mention unrelated dead code instead of deleting it.
- For multi-step work, state a brief plan with verifiable success criteria, then loop until verified.
- For bug fixes, prefer a reproducing test or focused verification before and after the fix when practical.
- Every changed line should trace directly to the user's request.

## Commands

- Workspace check: `make check`
- Rust checks: `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`
- CLI dev: `cargo run -p awidat-cli -- <command>`
- Desktop dev: `make desktop`
- Python sync: `cd python && uv sync --all-packages`

## Conventions

- Follow workspace lints in `Cargo.toml`; avoid `unwrap` and `expect` unless the local crate or test explicitly allows them.
- Keep changes within existing crate boundaries and match nearby patterns.
- Do not hand-edit `apps/desktop/src/protocol/generated/` unless the generation path is unavailable and the change is explicitly scoped.
- Python indexers live under `python/packages/*-mcp/`; use shared `awidat-mcp` patterns and avoid broad smoke tests that trigger model downloads unless required.
- Bundled skills live under `skills/<name>/SKILL.md`; prefer scripts for repeatable skill logic.

## Testing

- Run the narrow relevant check first, then broader checks when the blast radius justifies it.
- For desktop UI changes, run or build the desktop app when feasible.
- Release packaging references may exist in Makefile or CI, but this checkout has no top-level `dist/`; do not rely on `make package` unless that path is restored.

## Operational Notes

- Keep unrelated worktree changes intact.
- Keep edits scoped to the requested behavior and subsystem.
