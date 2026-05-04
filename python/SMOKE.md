# Indexer smoke tests

The `cargo test -p awidat-index --test end_to_end -- --ignored` test exercises
`audio-energy-mcp` only, because it has no model downloads and no API keys —
which means it's the only indexer cheap enough to run on every commit.

The other three indexers have nontrivial install and warm-up costs (model
downloads, GPU detection, HuggingFace gates). Run these manually before
committing changes that touch their code.

## Setup once

```bash
cd python && uv sync --all-packages
```

This downloads ~3GB of Python dependencies (torch, faster-whisper, sentence-
transformers, opencv). On first model use, models also download:

| Indexer | First-run download | Where it lands |
|---|---|---|
| whisper (`large-v3-turbo`) | ~1.6GB | `~/.cache/huggingface/hub/` |
| whisper (`small.en` fallback) | ~470MB | same |
| whisper diarization (`pyannote/speaker-diarization-community-1`) | ~30MB | same; requires `HF_TOKEN` and accepting the model EULA at <https://huggingface.co/pyannote/speaker-diarization-community-1> |
| topic (`all-MiniLM-L6-v2`) | ~80MB | same |
| clip (`ViT-B-32` / OpenAI) | ~150MB | `~/.cache/clip/` (open_clip default) |
| face, gaze (dlib `face_recognition_models`) | ~70MB | bundled in the wheel |
| scenedetect, audio-energy, shot, frame-quality | none | n/a |

## Smoke each indexer

Drop a real podcast/interview asset under a temp project's `raw/`, then:

```bash
PROJ=/tmp/awidat-smoke
mkdir -p "$PROJ/raw" "$PROJ/.awidat"
cp ~/Downloads/your-real-asset.wav "$PROJ/raw/"

# Configure all four indexers (substitute your absolute paths).
cat > "$PROJ/.awidat/config.toml" <<EOF
[[mcp.servers]]
name = "audio-energy"
command = "$HOME/.local/bin/uv"
args = ["run", "--package", "audio-energy-mcp", "audio-energy-mcp"]
cwd = "$PWD/python"
kind = "indexer"

[[mcp.servers]]
name = "scenedetect"
command = "$HOME/.local/bin/uv"
args = ["run", "--package", "scenedetect-mcp", "scenedetect-mcp"]
cwd = "$PWD/python"
kind = "indexer"

[[mcp.servers]]
name = "whisper"
command = "$HOME/.local/bin/uv"
args = ["run", "--package", "whisper-mcp", "whisper-mcp"]
cwd = "$PWD/python"
kind = "indexer"

[mcp.servers.env]
WHISPER_MODEL = "small.en"

[[mcp.servers]]
name = "topic"
command = "$HOME/.local/bin/uv"
args = ["run", "--package", "topic-mcp", "topic-mcp"]
cwd = "$PWD/python"
kind = "indexer"

[[mcp.servers]]
name = "clip"
command = "$HOME/.local/bin/uv"
args = ["run", "--package", "clip-mcp", "clip-mcp"]
cwd = "$PWD/python"
kind = "indexer"

[[mcp.servers]]
name = "face"
command = "$HOME/.local/bin/uv"
args = ["run", "--package", "face-mcp", "face-mcp"]
cwd = "$PWD/python"
kind = "indexer"

# shot reads scenedetect + face sidecars — run those first.
[[mcp.servers]]
name = "shot"
command = "$HOME/.local/bin/uv"
args = ["run", "--package", "shot-mcp", "shot-mcp"]
cwd = "$PWD/python"
kind = "indexer"

[[mcp.servers]]
name = "gaze"
command = "$HOME/.local/bin/uv"
args = ["run", "--package", "gaze-mcp", "gaze-mcp"]
cwd = "$PWD/python"
kind = "indexer"

[[mcp.servers]]
name = "frame-quality"
command = "$HOME/.local/bin/uv"
args = ["run", "--package", "frame-quality-mcp", "frame-quality-mcp"]
cwd = "$PWD/python"
kind = "indexer"
EOF

# Initialize and run.
awidat init "$PROJ" || true        # safe to skip if already initialized
awidat index "$PROJ"
awidat validate "$PROJ"
```

Then inspect the sidecars:

```bash
ls -lah $PROJ/index/*/
jq . $PROJ/index/audio-energy/raw/your-real-asset.wav.json | head -40
jq '.data | keys' $PROJ/index/whisper/raw/your-real-asset.wav.json
```

## Topic indexer cross-dependency

`topic-mcp` reads the whisper sidecar from disk to derive boundaries. If
you run `awidat index --indexer topic` before whisper has produced a
transcript, the sidecar will contain `topics: []` and a `note` explaining
what to do next. This is intentional — the engine doesn't model dependency
order between indexers; the agent does.

## Diarization

To enable speaker labels:

1. Create a HuggingFace account and accept the model gate at
   <https://huggingface.co/pyannote/speaker-diarization-community-1>.
2. `export HF_TOKEN=hf_...` (or set in the indexer's `[mcp.servers.env]`
   block).
3. Re-run `awidat index --indexer whisper`.

To opt out entirely: `WHISPER_DIARIZE=false` in the indexer env.
