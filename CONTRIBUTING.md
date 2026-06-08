# Contributing

Thanks for helping improve Montage.

## Development Setup

Prerequisites:

- Rust toolchain with Cargo.
- Node.js and pnpm for the desktop app.
- Python 3.11 and uv for Python indexers.
- ffmpeg on `PATH`.

Useful checks:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo check --workspace --all-targets
python3 python/scripts/smoke_indexers.py --safe
pnpm --dir apps/desktop test
```

Run the narrowest relevant check first, then broader checks when the change
touches shared behavior.

`make check` remains the historical full Rust gate and includes
`cargo test --workspace`, which currently exercises vendored Codex tests as
well as Montage crates. Prefer targeted `cargo test -p <crate>` runs unless
the change is meant to validate the whole vendored workspace.

## Where to start

Read `ARCHITECTURE.md` for the source layout, then inspect focused docs under
`docs/` for the subsystem you want to change. Issues labeled `good first issue`
should stay small, have clear reproduction or acceptance criteria, and avoid
cross-cutting architecture changes.

Before opening a pull request, run the checks that match your change. For most
Rust changes that means:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo check --workspace --all-targets
```

Add a targeted `cargo test -p <crate>` run when you touch behavior in a crate.

## Pull Requests

- Keep changes scoped to one behavior or cleanup.
- Match existing crate/module boundaries.
- Avoid unrelated formatting churn.
- Add tests for new logic and bug fixes when practical.
- Update docs when behavior, setup, or commands change.

## Style

Montage follows the workspace lints in `Cargo.toml`. Avoid `unwrap` and
`expect` in production Rust unless a nearby pattern explicitly allows it.

Generated files under `apps/desktop/src/protocol/generated/` should be updated
through their generation path, not hand-edited.

## Release Packaging

Release packaging is currently not restored in this checkout. Do not rely on
historical `dist/` commands unless that path is rebuilt first.
