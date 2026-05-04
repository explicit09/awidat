# awidat distribution

This directory contains the packaging + installation pieces for end-user
distribution of `awidat`.

## End-user install

Two equivalent paths.

**curl (works everywhere):**

```bash
curl -fsSL https://github.com/explicit09/awidat/releases/latest/download/install.sh | sh
```

**Homebrew (macOS / Linuxbrew, after the tap is set up):**

```bash
brew install explicit09/awidat/awidat
```

Both end at the same place: `awidat` on your `$PATH`, python indexer
tree resolved, ready for `awidat new` / `awidat tui` / `awidat secrets-set`.
Behind the scenes (curl path):

1. Detects OS+arch (macOS arm64/x86_64, Linux x86_64/aarch64).
2. Downloads `awidat-<triple>.tar.gz` from GitHub Releases.
3. Extracts to `~/.local/share/awidat/versions/<sha>/` (versioned for
   atomic upgrades; override the parent dir with `AWIDAT_HOME`).
4. Symlinks the `current` pointer + `~/.local/bin/awidat`.
5. Runs `uv sync --all-packages` once to materialize the per-indexer
   venvs (~3 GB of wheels — torch, dlib, opencv, faster-whisper).

Subsequent runs use the same script and are idempotent.

## Upgrading

```bash
awidat upgrade
```

Same end result as re-running `install.sh`. Atomic: never overwrites
the running binary (extracts new version to a sibling dir, swaps the
`current` symlink). Use `--from <url-or-path>` to install from a
specific source, `--check` to print what's installed without
fetching.

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

## CI / release pipeline

`/.github/workflows/release.yml` triggers on `v*` tag pushes. It:

1. Builds tarballs for all 4 supported triples (matrix: macOS arm64
   + macOS Intel native; Linux x86_64 native; Linux aarch64 via
   `cross`).
2. Downloads the matching `uv` release per triple and vendors it
   into the tarball.
3. Computes SHA256 per tarball.
4. Publishes a GitHub Release with all tarballs + `checksums.txt`
   + `install.sh` as assets.

After the release publishes, `homebrew-bump.yml` triggers automatically:
fetches the per-platform SHAs from the new release, rewrites the
`AUTOBUMP_*` blocks in `dist/homebrew/awidat.rb`, and pushes the new
formula to the `explicit09/homebrew-awidat` tap repo.

### One-time setup before the first release

The release workflow runs end-to-end on every `v*` tag with no
manual setup. The Homebrew tap, however, has prerequisites:

1. Create a public repo at `github.com/explicit09/homebrew-awidat`
   (Homebrew requires the `homebrew-` prefix for taps).
2. Create a fine-grained PAT with `contents: write` on that tap
   repo. Save it as `HOMEBREW_TAP_TOKEN` in this repo's Actions
   secrets.
3. Push the initial Formula by hand:
   ```
   cp dist/homebrew/awidat.rb /path/to/homebrew-awidat/Formula/awidat.rb
   cd /path/to/homebrew-awidat && git add . && git commit && git push
   ```

After that, every subsequent release auto-bumps the formula.

### Cutting a release

```bash
git tag v0.2.0
git push origin v0.2.0
```

That's it. Watch the Actions tab — release builds in ~10-15 min,
homebrew-bump runs once the release publishes, and within ~20 min
the new version is live at:

- `curl ...releases/latest/download/install.sh | sh`
- `awidat upgrade` (for users already on a previous version)
- `brew install explicit09/awidat/awidat` (after `brew update`)

## What's NOT here yet (deferred)

- **Code signing / notarization.** macOS Gatekeeper will warn on the
  first run. Real ship: get a Developer ID, codesign + notarize.
- **Bundled ffmpeg.** Currently relies on `ffmpeg` being on PATH. Can
  bundle a static ffmpeg in `bin/` if user-install friction warrants.
- **Windows support.** Not a target today. Awidat assumes a POSIX
  shell + uv + ffmpeg. Windows would need MSI packaging + WSL detection.
- **Stable VERSION endpoint for `awidat upgrade --check`.** Currently
  prints what's installed but doesn't fetch the remote VERSION for
  comparison. Add when there's a release.json (or equivalent) at
  the canonical URL.
