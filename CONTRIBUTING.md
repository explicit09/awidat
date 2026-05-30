# Contributing

Thanks for helping improve Awidat.

## Development Setup

Prerequisites:

- Rust toolchain with Cargo.
- Node.js and pnpm for the desktop app.
- Python 3.11 and uv for Python indexers.
- ffmpeg on `PATH`.

Useful checks:

```bash
make check
python3 python/scripts/smoke_indexers.py --safe
pnpm --dir apps/desktop exec tsc --noEmit
```

Run the narrowest relevant check first, then broader checks when the change
touches shared behavior.

## Pull Requests

- Keep changes scoped to one behavior or cleanup.
- Match existing crate/module boundaries.
- Avoid unrelated formatting churn.
- Add tests for new logic and bug fixes when practical.
- Update docs when behavior, setup, or commands change.

## Style

Awidat follows the workspace lints in `Cargo.toml`. Avoid `unwrap` and
`expect` in production Rust unless a nearby pattern explicitly allows it.

Generated files under `apps/desktop/src/protocol/generated/` should be updated
through their generation path, not hand-edited.

## Release Packaging

Release packaging is currently not restored in this checkout. Do not rely on
historical `dist/` commands unless that path is rebuilt first.
