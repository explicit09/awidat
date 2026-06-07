"""CLIP frame indexer for Awidat.

Samples one frame per second from the source video, encodes each through
ViT-B-32 (OpenAI weights, via open_clip), and writes the 512-dim
embeddings into a sidecar. The embedding store is the substrate for a
future `clip_search` tool that takes a free-text prompt ("close-up of
a face", "dark room", "person holding a phone") and returns the top-K
matching frames as `[t_s, score]` pairs.

Why per-second, not per-frame: at 24 fps a 10-min episode is 14k frames.
Embedding all of them costs ~$0 but writes ~30 MB of float32 per
episode. One frame per second is enough granularity for editorial
queries — the agent rarely asks "what was on screen at frame 1372",
it asks "where is the user holding the phone." Per-second sampling
keeps the sidecar under 1 MB for a 30-min episode.

Why float16 base64: lists of 512 floats x N seconds, JSON-encoded as
text decimals, balloons fast. Packing the embedding tensor as
little-endian float16 and base64-encoding it is 4x smaller than text
floats, and float16 loses no useful precision for cosine similarity at
top-K retrieval scale.

Schema version: "1".
"""

from __future__ import annotations

import base64
import logging
import os
import subprocess
import time
from collections.abc import Iterator
from typing import Any

import numpy as np
import open_clip
import torch
from PIL import Image

from awidat_mcp import IndexAssetRequest, IndexerServer

INDEXER_NAME = "clip"
INDEXER_VERSION = "0.1.0"
SCHEMA_VERSION = "1"

# Pinned model id. ViT-B-32 / OpenAI is the smallest CLIP that gives
# usable retrieval — ~150 MB, runs CPU-only at ~30 frames/sec on M-series.
# Larger models (ViT-L-14, EVA-CLIP) live behind a config flag in v1.5.
MODEL_ARCH = "ViT-B-32"
MODEL_PRETRAINED = "openai"

# One frame every two seconds by default. Enough for visual search and match
# candidates while keeping first-pass indexing responsive on long 4K sources.
SAMPLE_FPS = float(os.environ.get("CLIP_SAMPLE_FPS", "0.5"))

# Output pixel buffer ffmpeg writes per frame. Square crop matches the
# CLIP preprocess input — saves a resize round-trip on the Python side.
PROBE_W = 224
PROBE_H = 224

_log = logging.getLogger(INDEXER_NAME)


server = IndexerServer(
    name=INDEXER_NAME,
    indexer_version=INDEXER_VERSION,
    schema_version=SCHEMA_VERSION,
)


def _probe_duration_s(asset_path: str) -> float:
    """Return duration in seconds via ffprobe. Float, fail-loud."""
    out = subprocess.run(
        [
            "ffprobe",
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "format=duration",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
            asset_path,
        ],
        check=True,
        capture_output=True,
        text=True,
    )
    return float(out.stdout.strip())


def _iter_frames(asset_path: str, fps: float) -> Iterator[Image.Image]:
    """Stream `fps`-rate RGB frames out of ffmpeg as PIL Images.

    ffmpeg writes raw rgb24 to stdout at PROBE_W x PROBE_H. We slice
    that bytestream per-frame and yield immediately. No intermediate
    disk files, and no full-video stdout buffer in Python memory.
    """
    cmd = [
        "ffmpeg",
        "-nostdin",
        "-v",
        "error",
        "-i",
        asset_path,
        "-vf",
        f"fps={fps},scale={PROBE_W}:{PROBE_H}",
        "-pix_fmt",
        "rgb24",
        "-f",
        "rawvideo",
        "-",
    ]
    frame_size = PROBE_W * PROBE_H * 3
    proc = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    assert proc.stdout is not None
    try:
        while True:
            chunk = proc.stdout.read(frame_size)
            if not chunk:
                break
            if len(chunk) != frame_size:
                raise RuntimeError(
                    f"ffmpeg produced a partial frame ({len(chunk)} of {frame_size} bytes)"
                )
            yield Image.frombytes("RGB", (PROBE_W, PROBE_H), chunk)
        stderr = proc.stderr.read().decode("utf-8", errors="replace") if proc.stderr else ""
        rc = proc.wait()
        if rc != 0:
            raise RuntimeError(f"ffmpeg frame extraction failed (exit {rc}): {stderr.strip()}")
    finally:
        if proc.poll() is None:
            proc.kill()
            proc.wait()


_MODEL_CACHE: dict[str, Any] = {}


def _load_model() -> tuple[Any, Any, torch.device]:
    """Lazy-load and cache the CLIP model so reindexing many assets in
    one process pays the ~3s load only once."""
    if "model" in _MODEL_CACHE:
        return _MODEL_CACHE["model"], _MODEL_CACHE["preprocess"], _MODEL_CACHE["device"]
    device = torch.device("cuda" if torch.cuda.is_available() else "cpu")
    model, _, preprocess = open_clip.create_model_and_transforms(
        MODEL_ARCH, pretrained=MODEL_PRETRAINED, device=device
    )
    # Switch to inference mode (PyTorch API). We never train this model.
    model.train(False)
    _MODEL_CACHE["model"] = model
    _MODEL_CACHE["preprocess"] = preprocess
    _MODEL_CACHE["device"] = device
    return model, preprocess, device


def _encode_batch(images: list[Image.Image]) -> np.ndarray:
    """Run the CLIP image encoder over `images`. Returns (N, 512) float32
    numpy array. Embeddings are L2-normalized so cosine similarity is
    just a dot product."""
    model, preprocess, device = _load_model()
    if not images:
        return np.zeros((0, 512), dtype=np.float32)
    tensors = torch.stack([preprocess(im) for im in images]).to(device)
    with torch.no_grad():
        feats = model.encode_image(tensors)
        feats = feats / feats.norm(dim=-1, keepdim=True)
    return feats.cpu().float().numpy().astype(np.float32)


def _pack_embeddings_b64(arr: np.ndarray) -> str:
    """Float32 -> float16 -> little-endian bytes -> base64. See module
    docstring for the size-rationale."""
    return base64.b64encode(arr.astype(np.float16).tobytes()).decode("ascii")


@server.index_asset
def handle(req: IndexAssetRequest) -> dict[str, Any]:
    probe_start = time.perf_counter()
    duration_s = _probe_duration_s(req.asset_path)
    probe_ms = round((time.perf_counter() - probe_start) * 1000)
    _log.info(
        "clip-mcp: asset=%s duration=%.2fs sampling @ %.2f fps",
        req.asset_id,
        duration_s,
        SAMPLE_FPS,
    )
    model_start = time.perf_counter()
    _load_model()
    model_load_ms = round((time.perf_counter() - model_start) * 1000)

    # Encode streamed frames in batches of 32. This avoids buffering the
    # whole decoded video as raw RGB plus PIL objects.
    BATCH = 32
    chunks: list[np.ndarray] = []
    batch: list[Image.Image] = []
    frame_count = 0
    decode_read_ms = 0
    inference_ms = 0
    iterator = iter(_iter_frames(req.asset_path, SAMPLE_FPS))
    while True:
        read_start = time.perf_counter()
        try:
            frame = next(iterator)
        except StopIteration:
            decode_read_ms += round((time.perf_counter() - read_start) * 1000)
            break
        decode_read_ms += round((time.perf_counter() - read_start) * 1000)
        batch.append(frame)
        if len(batch) >= BATCH:
            inference_start = time.perf_counter()
            chunks.append(_encode_batch(batch))
            inference_ms += round((time.perf_counter() - inference_start) * 1000)
            frame_count += len(batch)
            batch.clear()
    if batch:
        inference_start = time.perf_counter()
        chunks.append(_encode_batch(batch))
        inference_ms += round((time.perf_counter() - inference_start) * 1000)
        frame_count += len(batch)
    embeddings = (
        np.concatenate(chunks, axis=0)
        if chunks
        else np.zeros((0, 512), dtype=np.float32)
    )

    # Build the per-second timeline — frame i corresponds to t_s = i / SAMPLE_FPS.
    timestamps = [i / SAMPLE_FPS for i in range(frame_count)]

    return {
        "model": f"{MODEL_ARCH}/{MODEL_PRETRAINED}",
        "embedding_dim": int(embeddings.shape[1]) if embeddings.size else 512,
        "embedding_dtype": "float16",
        "embedding_encoding": "base64",
        "frame_rate_sampled": SAMPLE_FPS,
        "duration_s": duration_s,
        "frame_count": frame_count,
        "timestamps_s": timestamps,
        "embeddings_b64": _pack_embeddings_b64(embeddings),
        "perf": {
            "probe_ms": probe_ms,
            "model_load_ms": model_load_ms,
            "decode_read_ms": decode_read_ms,
            "inference_ms": inference_ms,
            "frames_processed": frame_count,
        },
    }


def decode_embeddings(body: dict[str, Any]) -> np.ndarray:
    """Helper for downstream consumers (clip_search tool, tests).
    Inverse of `_pack_embeddings_b64`. Public so the Rust core / future
    Python tests can recover the float32 (N, dim) array."""
    raw = base64.b64decode(body["embeddings_b64"])
    n = body["frame_count"]
    dim = body["embedding_dim"]
    return np.frombuffer(raw, dtype=np.float16).reshape(n, dim).astype(np.float32)


def encode_text(query: str) -> np.ndarray:
    """Encode a free-text query into the same 512-dim CLIP embedding
    space as the indexed frames. Returns a single L2-normalized
    float32 vector, shape (512,).

    Reuses `_load_model()`'s cache, so this is ~50ms warm. The first
    call after server startup pays the ~3s model load — but then the
    embedded model lives for the whole MCP session, so all subsequent
    queries are cheap. This is the entire point of running clip-mcp
    long-lived via McpHost: amortize the load cost across every
    `clip_search` call in the agent's TUI session, not per call.
    """
    model, _preprocess, device = _load_model()
    tokenizer = open_clip.get_tokenizer(MODEL_ARCH)
    tokens = tokenizer([query]).to(device)
    with torch.no_grad():
        feat = model.encode_text(tokens)
        feat = feat / feat.norm(dim=-1, keepdim=True)
    return feat.cpu().float().numpy()[0].astype(np.float32)


# Register the extra tool *after* the IndexerServer has built its
# FastMCP instance. This is the official path to "an indexer with one
# additional non-indexer tool"; the awidat-mcp shim deliberately keeps
# the indexer surface clean and lets indexers reach into _mcp for the
# few cases where they need to expose more (cross-indexer queries,
# warm-cache reuse). Document the pattern in INDEX_SCHEMA.md if a
# second indexer ever needs it.
@server._mcp.tool(  # noqa: SLF001  -- intentional, see comment above.
    description=(
        "Encode a free-text query into the CLIP embedding space (512-dim, "
        "L2-normalized, base64-packed float16). Used by the Rust-side "
        "`clip_search` tool, which then computes cosine similarity against "
        "the per-frame embeddings already in this asset's CLIP sidecar. "
        "Reuses the model loaded during indexing — cheap warm, expensive cold."
    )
)
def query_text(query: str) -> dict[str, Any]:
    vec = encode_text(query)
    return {
        "model": f"{MODEL_ARCH}/{MODEL_PRETRAINED}",
        "embedding_dim": int(vec.shape[0]),
        "embedding_dtype": "float16",
        "embedding_encoding": "base64",
        "embedding_b64": base64.b64encode(vec.astype(np.float16).tobytes()).decode("ascii"),
    }


def main() -> None:
    server.run()
