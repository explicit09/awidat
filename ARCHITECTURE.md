# Architecture

Montage is organized as a Rust-first media editing workspace with a desktop
shell, Python indexers, bundled editorial skills, and a vendored agent runtime.

```text
media import
  -> project manifest and probes
  -> Python and Rust indexers
  -> searchable media facts
  -> agent tools and bundled skills
  -> EDL/timeline edits
  -> render/export
```

`crates/` contains the Rust workspace. The crates cover the CLI, agent integration, project protocol, media indexing, rendering, desktop protocol, secrets,
configuration, social publishing, and shared test support.

`apps/desktop/` contains the Tauri 2 desktop application and React/Vite
frontend. It presents local projects, authentication choices, timeline data,
and desktop-specific protocol surfaces while delegating core behavior to the
Rust workspace. `StageShell` is the application workspace; browser checks
load this same application through the shared IPC fixture.

`python/` is a `uv` workspace for MCP indexers. These packages extract
transcripts, scenes, audio energy, faces, gaze, CLIP-searchable frames, shot
classification, and color metadata for agent tools to query.

`skills/` contains bundled editorial workflows exposed through Montage skill
commands. Skills keep repeatable editing procedures close to the repository so
agents can run them consistently instead of re-creating instructions by hand.

`vendor/codex-rs/` contains the vendored Codex-derived agent runtime used by
Montage as an external Codex app-server sidecar. The desktop bridge owns
process lifecycle and protocol mapping. Local changes should stay narrow,
documented, and compatible with the
repository's public OAuth/API-key posture.

The social publishing surface is split between `crates/social/` and
`crates/social-server/`. The Rust library owns shared social publishing types,
migrations, and client behavior, while the server crate exposes the local or
hosted service boundary for publishing integrations.
