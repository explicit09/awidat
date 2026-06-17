# Sidecar binaries

Tauri bundles these alongside the desktop app at build time. Dropped
out of git because some are tens of MB; they're fetched on demand
by the root `Makefile` sidecar targets (also auto-run by `make desktop`).

## Codex

The desktop agent uses a bundled `codex` CLI sidecar by default, falling
back to `MONTAGE_CODEX_BIN` and then `codex` on `PATH` only when the
sidecar is absent. Build the active-target sidecar with:

```sh
make desktop-codex
```

CI compile checks use `make desktop-sidecar-check-stubs` to satisfy
Tauri's externalBin path validation without downloading media tools or
rebuilding the full Codex/MCP binaries for every Rust check. Do not use
those stubs for a runnable desktop app or release package.

Files this dir should contain (per platform target):

| Triple                         | Filename                              |
|--------------------------------|---------------------------------------|
| `aarch64-apple-darwin`         | `codex-aarch64-apple-darwin`          |
| `x86_64-apple-darwin`          | `codex-x86_64-apple-darwin`           |
| `x86_64-unknown-linux-gnu`     | `codex-x86_64-unknown-linux-gnu`      |
| `aarch64-unknown-linux-gnu`    | `codex-aarch64-unknown-linux-gnu`     |
| `x86_64-pc-windows-msvc.exe`   | `codex-x86_64-pc-windows-msvc.exe`    |

## yt-dlp

Standalone macOS / Linux / Windows builds of yt-dlp from the
upstream GitHub releases. `make desktop-yt-dlp` pins the release
version in the root `Makefile` with `YT_DLP_VERSION` instead of
following GitHub's mutable `latest` redirect. Naming follows Tauri's
externalBin convention: `<base>-<rust-target-triple>`.

Files this dir should contain (per platform target):

| Triple                         | Filename                              |
|--------------------------------|---------------------------------------|
| `aarch64-apple-darwin`         | `yt-dlp-aarch64-apple-darwin`         |
| `x86_64-apple-darwin`          | `yt-dlp-x86_64-apple-darwin`          |
| `x86_64-unknown-linux-gnu`     | `yt-dlp-x86_64-unknown-linux-gnu`     |
| `aarch64-unknown-linux-gnu`    | `yt-dlp-aarch64-unknown-linux-gnu`    |
| `x86_64-pc-windows-msvc.exe`   | `yt-dlp-x86_64-pc-windows-msvc.exe`   |

For your active dev triple you only need that one file. CI / release
builds should fetch the target-specific binary before `tauri build`,
for example:

```sh
make desktop-yt-dlp TARGET_TRIPLE=aarch64-apple-darwin
```

For macOS DMG builds, fetch the sidecar for the same target triple as
the Tauri build. The pinned yt-dlp macOS asset is shared by both
`aarch64-apple-darwin` and `x86_64-apple-darwin`; the filename still
uses the Tauri target triple so `.sidecar("yt-dlp")` resolves correctly.
Bump `YT_DLP_VERSION` in the root `Makefile` when upgrading the bundled
downloader; set `YT_DLP_REFRESH=1` to force a local re-download.

## Required runtime sidecars

Consumer builds require these sidecars beside `montage-desktop`:

- `codex` for the bundled agent CLI.
- `ffmpeg` and `ffprobe` for render, proxy, thumbnail, waveform, and media probe work.
- `montage-mcp-server` so the agent can call Montage tools in packaged builds.
- `uv` for the bundled Python indexer workspace.
- `yt-dlp` for media import support.

Use the root Makefile targets to populate runnable binaries for the active
target:

```sh
make desktop-codex
make desktop-ffmpeg
make desktop-mcp-server
make desktop-uv
make desktop-yt-dlp
```

Release builds run those targets for the matrix target and then call
`scripts/release/verify-sidecars.sh`, which rejects CI check stubs.
