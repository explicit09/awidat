# Index sidecar contract

The footage index is the single biggest determinant of agent quality.
Future indexers (speaker emotion, conversational structure, visual
moments, cross-modal alignment) must slot into this layout without engine
changes. If they can't, this document is wrong.

## What an indexer is

A footage *indexer* is a process that reads a source asset (a video or
audio file) and produces a JSON sidecar describing some channel of the
asset — words, shot boundaries, audio energy, speaker emotion, etc.

Indexers are not part of the `montage` engine. They are external programs,
typically Python MCP servers, that the engine launches via `montage index`.
The engine never imports an indexer's code; the only contract between the
engine and an indexer is the *sidecar JSON shape* defined here.

## Where sidecars live

```
<project>/
├── index/
│   ├── manifest.json
│   ├── whisper/
│   │   ├── ep-014-cam-a.mp4.json
│   │   └── ep-014-audio.wav.json
│   ├── scenedetect/
│   │   └── ep-014-cam-a.mp4.json
│   ├── audio-energy/
│   │   └── ep-014-audio.wav.json
│   ├── beats/
│   │   └── ep-014-audio.wav.json
│   └── speaker-emotion/        # added in v1.5; engine notices via manifest
│       └── ep-014-audio.wav.json
└── ...
```

**One indexer = one subdirectory.** The directory name is the indexer's
canonical id (see [Naming conventions](#naming-conventions)). Sidecars from
different indexers never share a file. Sidecars within an indexer's
directory are keyed by asset.

This is **not** a single `index/<asset>.json` file with all signals merged.
That shape would calcify the signal set and break additions of new indexers.

## The shared coordinate model

Every indexer's body emits timestamps. To make signals from different
indexers joinable on the timeline, every emitted timestamp uses the same
canonical type:

| Type | Meaning | Encoding |
| --- | --- | --- |
| [`AssetId`](src/index.rs) | Logical id of a source asset, e.g. `"raw/ep-014-cam-a.mp4"` | String, JSON pass-through |
| [`TimeSeconds`](src/index.rs) | Seconds since start of asset | `f64` |
| `segment_id` (when applicable) | Optional opaque id for a contiguous indexer-defined segment | String |

`f64` seconds are the lingua franca because OTIO time is `(value, rate)`
which already needs conversion to seconds for any cross-rate join, and
indexers run at heterogeneous rates (whisper at word boundaries,
scenedetect at frame boundaries, audio-energy at fixed RMS windows).
Forcing every indexer to seconds makes the join trivial.

## The self-describing header

Every sidecar JSON document has this shape:

```json
{
  "indexer": "whisper",
  "indexer_version": "1.4.2",
  "schema_version": "1",
  "asset_id": "raw/ep-014-cam-a.mp4",
  "asset_sha256": "9f2a…",
  "produced_at": "2026-05-02T10:14:33Z",
  "data": { /* indexer-specific */ }
}
```

The header is the same for every indexer. The `data` object is opaque to
the engine — typed only inside the indexer that produced it. The engine
deserializes the header into [`IndexSidecarHeader`](src/index.rs); the
body lives as `serde_json::Value`.

| Field | Meaning |
| --- | --- |
| `indexer` | Canonical indexer id; matches the parent directory name. |
| `indexer_version` | Software version of the producing process. Bumped on bug fixes that don't change the schema. |
| `schema_version` | Version of the *body* schema. Bumped only when `data`'s shape changes. Independent of `indexer_version`. |
| `asset_id` | Asset this sidecar describes. |
| `asset_sha256` | SHA-256 of the asset bytes at index time. The engine compares against the live asset to detect stale sidecars. |
| `produced_at` | UTC RFC-3339 timestamp. |

## The manifest

`index/manifest.json` is the registry the engine consults to learn what
indexers have run. Shape:

```json
{
  "version": "0.1",
  "indexers": [
    {
      "name": "whisper",
      "version": "1.4.2",
      "schema_version": "1",
      "assets": ["raw/ep-014-cam-a.mp4", "raw/ep-014-audio.wav"],
      "last_run": "2026-05-02T10:14:33Z"
    }
  ]
}
```

The engine treats the manifest as **data, never code**. There is no list
of "known indexer names" in the engine. Adding a new indexer in v1.5 is a
pure data operation: drop a new MCP server, register an entry in the
manifest, write sidecars to a new directory.

`montage validate` cross-checks the manifest against disk:

- Every manifest entry → corresponding directory exists. (Warning if not.)
- Every sidecar found on disk → header consistent with the manifest entry
  for its directory. (Warning on mismatch.)
- Every directory under `index/` → listed in the manifest. (Warning on
  orphan.)

All four are **warnings, not errors**. A partially-indexed project is
still a usable project.

## Naming conventions

Indexer ids are lowercase, hyphen-separated, no whitespace, no slashes:

- `whisper`, `whisperx` ✓
- `audio-energy`, `beats`, `speaker-emotion` ✓
- `Whisper` ✗ (not lowercase)
- `audio_energy` ✗ (use hyphen)
- `audio/energy` ✗ (no slashes)

The directory name and the value of the `indexer` field always match.
`montage validate` warns if they don't.

## Sidecar filename convention

`index/<indexer>/<asset-relative-path>.json`. Slashes in the asset path
are preserved as nested directories — i.e. the sidecar for asset
`raw/ep-014-cam-a.mp4` lives at
`index/whisper/raw/ep-014-cam-a.mp4.json`.

This keeps a 1:1 mapping between source assets and sidecars and avoids
needing a manifest scan to find a single sidecar.

## Adding an indexer in v1.5: worked example

Suppose v1.5 adds a `speaker-emotion` indexer that produces a per-window
valence/arousal track. The agent uses it to find emotionally peak moments
better than `audio-energy` alone can.

### Step 1. Define the body schema (in the indexer's own crate / repo).

```jsonc
// schema_version: "1"
// data shape:
{
  "windows": [
    {
      "start_s": 0.0,
      "end_s": 1.0,
      "valence": 0.62,
      "arousal": 0.81,
      "dominant_speaker_id": "spk-1"
    }
  ]
}
```

### Step 2. Implement the indexer as an MCP server.

It exposes a `speaker_emotion.run(asset_id) -> Sidecar` tool. The body
matches the schema above; the header is filled with `indexer:
"speaker-emotion"`, the indexer's own version, and `schema_version: "1"`.

### Step 3. Register the indexer in montage's MCP config.

Same shape as any MCP server registration in `~/.config/montage/config.toml`.
The engine launches the server when `montage index` runs.

### Step 4. Run `montage index <project>`.

The engine:

- launches every registered indexer in parallel,
- collects each one's `Sidecar`,
- writes the body to `index/<indexer>/<asset>.json`,
- updates `index/manifest.json` to add the new entry.

### Step 5. The agent reads the new signal.

The agent's existing `read_index(asset, channel)` tool grows a new channel
value `"speaker-emotion"` because skills know about it via their `SKILL.md`
— **not** because the engine has special-cased it. From the engine's
perspective, `read_index` just reads `index/speaker-emotion/<asset>.json`
and returns it.

### What did NOT change in the engine

- No new types in [`crates/proto/`](src/index.rs).
- No new struct fields anywhere.
- No new code paths in `montage init` / `montage validate` / `montage index`.
- No new dependency on speaker-emotion's data schema.

That's the whole point.

## Failure modes the contract guards against

| Failure if we'd built v1 wrong | What the v1 contract does |
| --- | --- |
| All signals share `index/<asset>.json` → adding a v1.5 channel is a breaking schema change | One file per (indexer, asset). Additive. |
| Engine has hardcoded list of channels → `read_index(channel="speaker-emotion")` rejects unknown channels | Engine treats channel name as a pass-through directory lookup. |
| Sidecar headers omit indexer version → can't detect stale sidecars after an indexer bug fix | `indexer_version` + `asset_sha256` together catch every stale-cache case. |
| Manifest is implicit (engine scans `index/` and infers) → impossible to register an indexer that's run on zero assets so far | Manifest is explicit, the source of truth. |
| Body type is strongly typed in the engine → engine has to be redeployed to ship a new indexer | Body is `serde_json::Value` to the engine. |

## See also

- [`src/index.rs`](src/index.rs) — Rust types implementing this contract.
- [`OTIO_NOTES.md`](OTIO_NOTES.md) — sister doc for the OTIO superset.
