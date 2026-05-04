# awidat distribution

This directory contains the packaging + installation pieces for end-user
distribution of `awidat`. Compiled in Phase 3 of the v1.5 plan.

## End-user install

```bash
curl -fsSL https://awidat.example/install.sh | sh
```

That's it. Behind the scenes:

1. Detects OS+arch (macOS arm64/x86_64, Linux x86_64/aarch64).
2. Downloads `awidat-<triple>.tar.gz`.
3. Extracts to `~/.local/share/awidat/` (override with `AWIDAT_HOME`).
4. Symlinks `awidat` into `~/.local/bin/`.
5. Runs `uv sync --all-packages` once to materialize the per-indexer
   venvs (~3 GB of wheels — torch, dlib, opencv, faster-whisper).

Subsequent re-runs of the script are idempotent and upgrade in place.

## Why this distribution model

The Rust binary is small (~10 MB). The Python indexers depend on heavy
native extensions (PyTorch, dlib, OpenCV) whose wheels are best resolved
against the user's actual OS / glibc / CUDA / MPS — not pre-baked on a
CI runner. Bundling Python via PyOxidizer fights this; Docker breaks
MCP stdio for desktop users; pre-built shiv zipapps don't handle native
extensions well.

The boring/right answer is: ship the Rust binary + a vendored `uv` +
the python source tree, and let `uv` materialize the venvs on the user's
machine. This matches the runtime model already in place
(`uv run --package <name>-mcp <name>-mcp` for each indexer launch).

Cross-harness reference: this is the same shape `cargo install` ends
up at (binary + source bundle), and it's what most ML-CLI tools land
on (ollama, whisper.cpp, marian-decoder).

## Building a release tarball

```bash
./dist/package.sh             # builds for the host platform
make package                  # same, via Makefile
```

Output lands in `dist/build/awidat-<triple>.tar.gz` and contains:

```
bin/awidat               # the Rust CLI, stripped
bin/uv                   # vendored uv (so the installer doesn't
                         # require the user to have uv preinstalled)
python/                  # the uv workspace tree (sources +
                         # pyproject.toml + uv.lock; no .venv)
share/awidat/install.sh  # the bundled installer
VERSION                  # awidat --version snapshot
```

## Local-install testing

To smoke a release tarball against your own machine without a webserver:

```bash
./dist/package.sh
AWIDAT_RELEASE_BASE=file://$(pwd)/dist/build \
  bash dist/build/awidat-aarch64-apple-darwin/share/awidat/install.sh
```

That tells the installer to fetch from the local build dir, mirroring
exactly what the real release flow looks like minus the curl.

## What's NOT here yet (deferred)

- **CI release pipeline.** No GitHub Actions yaml that builds for all
  triples and publishes to a releases page. Single-platform local
  packaging works today; cross-compilation + multi-platform CI is the
  next packaging arc.
- **Code signing / notarization.** macOS Gatekeeper will warn on the
  first run. Real ship: get a Developer ID, codesign + notarize.
- **Homebrew formula.** Layered on top of #1 once the install URL is
  stable. `brew install awidat` becomes a one-liner that runs the
  install.sh under the hood.
- **Auto-update.** `awidat upgrade` re-runs install.sh. Cheap to add
  once the release endpoint is live.
- **Bundled ffmpeg.** Currently relies on `ffmpeg` being on PATH. Can
  bundle a static ffmpeg in `bin/` if user-install friction warrants.
