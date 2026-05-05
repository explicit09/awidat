# Sidecar binaries

Tauri bundles these alongside the desktop app at build time. Dropped
out of git because some are tens of MB; they're fetched on demand
by `make desktop-yt-dlp` (also auto-run by `make desktop`).

## yt-dlp

Standalone macOS / Linux / Windows builds of yt-dlp from the
upstream GitHub releases. Naming follows Tauri's externalBin
convention: `<base>-<rust-target-triple>`.

Files this dir should contain (per platform target):

| Triple                         | Filename                              |
|--------------------------------|---------------------------------------|
| `aarch64-apple-darwin`         | `yt-dlp-aarch64-apple-darwin`         |
| `x86_64-apple-darwin`          | `yt-dlp-x86_64-apple-darwin`          |
| `x86_64-unknown-linux-gnu`     | `yt-dlp-x86_64-unknown-linux-gnu`     |
| `aarch64-unknown-linux-gnu`    | `yt-dlp-aarch64-unknown-linux-gnu`    |
| `x86_64-pc-windows-msvc.exe`   | `yt-dlp-x86_64-pc-windows-msvc.exe`   |

For your active dev triple you only need that one file. CI / release
builds populate the rest.
