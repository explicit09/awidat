# Indexer smoke tests

## Safe metadata smoke

The deterministic, no-download smoke check is:

```bash
python3 python/scripts/smoke_indexers.py --safe
```

It validates the `uv` workspace member list, package layout, indexer
schema/version markers, the common sidecar header shape, and the eval
workflow contract for real-corpus gate variables and model-sidecar
preflight. It does not import heavy indexer modules, run ffmpeg,
download models, or touch gated Hugging Face flows. This is the subset
that is safe for CI-style use.

## Safe real-indexer smoke

The lowest-cost real sidecar smoke runs the `audio-energy-mcp` indexer
through the Rust index dispatcher against a tiny checked-in WAV fixture:

```bash
python3 python/scripts/smoke_indexers.py --safe --audio-energy
```

This may install Python package dependencies through `uv`, but it does not
download model weights or require API keys. It is available as an opt-in
job in `.github/workflows/evals.yml`.

The guarded full setup command is:

```bash
MONTAGE_RUN_FULL_INDEXER_SMOKE=1 python3 python/scripts/smoke_indexers.py --full
```

That only syncs the full Python dependency workspace. Use the manual
real-asset commands below for model-backed indexers after accepting any
required model gates.

The `cargo test -p montage-index --test end_to_end -- --ignored` test exercises
`audio-energy-mcp` only, because it has no model downloads and no API keys —
which means it's the only indexer cheap enough to run on every commit.

The model-backed indexers have nontrivial install and warm-up costs
(model downloads, GPU detection, HuggingFace gates). Run these manually
before committing changes that touch their code.

## Fast whisper.cpp transcript backend

The whisper indexer defaults to `WHISPER_BACKEND=auto`. When `whisper-cli`,
`ffmpeg`, `ffprobe`, and the configured ggml model are present, it uses the
fast `whisper.cpp` chunked backend followed by WhisperX word alignment. If
those prerequisites are missing, it falls back to the pure WhisperX path.

Install the default ggml model with:

```bash
python3 python/scripts/download_whisper_cpp_model.py
```

The script writes:

```text
~/.cache/montage/whisper.cpp/ggml-large-v3-turbo-q5_0.bin
```

Useful environment variables:

| Variable | Purpose |
|---|---|
| `WHISPER_BACKEND=auto` | Prefer `whisper.cpp` when available, otherwise fallback to WhisperX |
| `WHISPER_BACKEND=whispercpp-aligned` | Require the fast backend and fail if prerequisites are missing |
| `WHISPER_BACKEND=whisperx` | Force the original WhisperX path |
| `WHISPER_CPP_MODEL` | Override the ggml model path |
| `WHISPER_CPP_THREADS` | Pass `-t` to `whisper-cli` |
| `WHISPER_CPP_PROCESSORS` | Pass `-p` to `whisper-cli` |
| `WHISPER_CPP_DEVICE` | Pass `--device` to `whisper-cli` |
| `WHISPER_CPP_NO_GPU=true` | Pass `-ng` to disable GPU |
| `WHISPER_CPP_EXTRA_ARGS` | Additional shell-style args for `whisper-cli` |

## Setup once

```bash
cd python && uv sync --all-packages
```

This downloads ~3GB of Python dependencies (torch, faster-whisper, sentence-
transformers, opencv). On first model use, models also download:

| Indexer | First-run download | Where it lands |
|---|---|---|
| whisper.cpp fast backend (`ggml-large-v3-turbo-q5_0`) | ~550MB | `~/.cache/montage/whisper.cpp/` |
| whisper (`large-v3-turbo`) | ~1.6GB | `~/.cache/huggingface/hub/` |
| whisper (`small.en` fallback) | ~470MB | same |
| whisper diarization (`pyannote/speaker-diarization-community-1`) | ~30MB | same; requires `HF_TOKEN` and accepting the model EULA at <https://huggingface.co/pyannote/speaker-diarization-community-1> |
| topic (`all-MiniLM-L6-v2`) | ~80MB | same |
| clip (`ViT-B-32` / OpenAI) | ~150MB | `~/.cache/clip/` (open_clip default) |
| face, gaze (dlib `face_recognition_models`) | ~70MB | bundled in the wheel |
| scenedetect, audio-energy, shot, composition, frame-quality | none | n/a |

## Smoke each indexer

Drop a real podcast/interview asset under a temp project's `raw/`, then:

```bash
PROJ=/tmp/montage-smoke
mkdir -p "$PROJ/raw" "$PROJ/.montage"
cp ~/Downloads/your-real-asset.wav "$PROJ/raw/"

# Configure the indexers (substitute your absolute paths).
cat > "$PROJ/.montage/config.toml" <<EOF
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

[[mcp.servers]]
name = "gaze"
command = "$HOME/.local/bin/uv"
args = ["run", "--package", "gaze-mcp", "gaze-mcp"]
cwd = "$PWD/python"
kind = "indexer"

# gaze reuses face's per-frame landmarks when index/face/<asset>.json exists.
# shot reads scenedetect, face, gaze, clip, and composition sidecars when
# present. For manual single-indexer checks, run those producers first.
[[mcp.servers]]
name = "shot"
command = "$HOME/.local/bin/uv"
args = ["run", "--package", "shot-mcp", "shot-mcp"]
cwd = "$PWD/python"
kind = "indexer"

[[mcp.servers]]
name = "composition"
command = "$HOME/.local/bin/uv"
args = ["run", "--package", "composition-mcp", "composition-mcp"]
cwd = "$PWD/python"
kind = "indexer"

# composition reads scenedetect and optional face/gaze sidecars. If a
# model classifier writes index/composition-model/<asset>.json, matching
# model:* regions override heuristic labels in index/composition.

[[mcp.servers]]
name = "frame-quality"
command = "$HOME/.local/bin/uv"
args = ["run", "--package", "frame-quality-mcp", "frame-quality-mcp"]
cwd = "$PWD/python"
kind = "indexer"
EOF

# Initialize and run.
montage init "$PROJ" || true        # safe to skip if already initialized
montage index "$PROJ"
montage validate "$PROJ"
```

Then inspect the sidecars:

```bash
ls -lah $PROJ/index/*/
jq . $PROJ/index/audio-energy/raw/your-real-asset.wav.json | head -40
jq '.data | keys' $PROJ/index/whisper/raw/your-real-asset.wav.json
```

## Composition model sidecar contract

`composition-mcp` accepts optional model-backed annotations at
`index/composition-model/<asset>.json`. The safe smoke validates the
shape without importing any model code, including the checked-in example
at `python/fixtures/composition-model/sample.json`. The sidecar `data`
object must contain non-empty `regions`; each region must include:

- `start_s` and `end_s`, with `end_s > start_s`
- `composition_source` beginning with `model:`
- `composition_confidence` from `0.0` to `1.0`
- controlled-label `subject_role`, `depth_layer`, and `framing`

Accepted `subject_role` values are `environment`,
`background_person`, `featured_subject`, `primary_speaker`,
`secondary_subject`, and `object_detail`.

Accepted `depth_layer` values are `foreground`, `midground`,
`background`, and `mixed_depth`.

Accepted `framing` values are `extreme_close_up`, `single_close`,
`single_medium`, `wide_context`, `two_shot`, `group`, and `insert`.

Overlapping model regions override the heuristic fields emitted by
`composition-mcp`, while the heuristic values are retained under
`heuristic_*` audit keys.

To validate a real indexed project after a model-backed classifier has
written `index/composition-model` sidecars, point the safe smoke at the
project root. `MONTAGE_REAL_CORPUS` is accepted as the same project-root
fallback used by the live eval workflow:

```bash
MONTAGE_COMPOSITION_MODEL_PROJECT="$PROJ" \
MONTAGE_COMPOSITION_MODEL_MIN_REGIONS=25 \
python3 python/scripts/smoke_safe.py
```

The invalid-region tolerance defaults to zero. Safe smoke accepts
`MONTAGE_REAL_VISUAL_MIN_COMPOSITION_MODEL_REGIONS` as the fallback for
`MONTAGE_COMPOSITION_MODEL_MIN_REGIONS`, matching the real-corpus
workflow mapping; a value of `0` keeps that gate disabled, matching the
workflow condition. For a temporary rollout window, set
`MONTAGE_COMPOSITION_MODEL_MAX_INVALID_REGIONS` to the same value as
`MONTAGE_REAL_VISUAL_MAX_INVALID_COMPOSITION_MODEL_REGIONS`; the
real-corpus workflow forwards that value automatically. If
`MONTAGE_COMPOSITION_MODEL_MAX_INVALID_REGIONS` is forwarded as blank but
`MONTAGE_REAL_VISUAL_MAX_INVALID_COMPOSITION_MODEL_REGIONS` is set, the
safe smoke uses the real-corpus value as the fallback. Blank optional
threshold environment variables otherwise use defaults, so unset GitHub
repository variables do not break the preflight. If any composition-model
project-tree threshold is set, either `MONTAGE_COMPOSITION_MODEL_PROJECT`
or `MONTAGE_REAL_CORPUS` must also be set; otherwise the safe smoke fails
instead of silently skipping the configured gate.

The project-tree check stays schema-only: it reads every
`index/composition-model/**/*.json` sidecar, validates the same region
contract as the checked-in sample, and fails if the total model-region
count is below `MONTAGE_COMPOSITION_MODEL_MIN_REGIONS`. If sidecars are
present but invalid, the failure summarizes valid and invalid region
counts and includes sample path/reason diagnostics so model rollouts can
distinguish missing output from contract-breaking output.

`composition-mcp` sidecars include a `verification` object with
`passed`, `checked_regions`, and `issues`. This is a lightweight sanity
report for generated regions and does not render media. `frame-quality-mcp`
sidecars include `thumbnail_score` per sampled frame and ranked
`thumbnail_candidates` derived from sharpness, exposure, and contrast.
`audio-energy-mcp` sidecars include `true_peak_dbfs` next to integrated
LUFS for delivery loudness review.

Safe smoke also preflights mounted real-corpus fixture coverage when
`MONTAGE_REAL_MIN_ASSESSOR_PROPOSAL_FIXTURES`,
`MONTAGE_REAL_MIN_TRANSITION_PLANNER_FIXTURES`, or
`MONTAGE_REAL_MIN_ROUGH_ASSEMBLY_FIXTURES` is non-zero. It requires
`MONTAGE_REAL_CORPUS` to point at a project directory with
`project.otio.json`, then counts the default single fixture file and
`.montage/eval/<fixture-kind>/*.json` directory layout before Rust parses
and applies the fixtures.

## Topic indexer cross-dependency

`topic-mcp` reads the whisper sidecar from disk to derive boundaries. If
you run `montage index --indexer topic` before whisper has produced a
transcript, the sidecar will contain `topics: []` and a `note` explaining
what to do next. This is intentional — the engine doesn't model dependency
order between indexers; the agent does.

## Diarization

To enable speaker labels:

1. Create a HuggingFace account and accept the model gate at
   <https://huggingface.co/pyannote/speaker-diarization-community-1>.
2. `export HF_TOKEN=hf_...` (or set in the indexer's `[mcp.servers.env]`
   block).
3. Re-run `montage index --indexer whisper`.

To opt out entirely: `WHISPER_DIARIZE=false` in the indexer env.
