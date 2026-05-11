# Awidat Desktop

Tauri 2 desktop shell for Awidat. The frontend is React/Vite and the backend
bridges the UI to the Rust workspace crates.

## Development

From the repository root:

```bash
make desktop
```

The make target installs frontend dependencies, fetches the host `yt-dlp`
sidecar expected by the Tauri bundle, and starts `pnpm tauri dev`.

The Vite dev server uses Tauri's fixed port `1420`. If that port is already in
use, run:

```bash
make desktop-stop
```

## Layout

- `src/` - React frontend.
- `src-tauri/` - Tauri backend.
- `src/protocol/generated/` - generated protocol TypeScript; avoid manual edits
  unless the generation path is unavailable and the change is explicitly scoped.
