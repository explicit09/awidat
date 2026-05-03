"""Topic-segmentation indexer for Awidat.

Reads the transcript sidecar produced by `whisper-mcp` (sibling under
`<project>/index/whisper/<asset>.json`), runs sentence-embedding boundary
detection (TextTiling-with-embeddings: sliding-window cosine similarity
of adjacent N-sentence windows), and optionally labels each segment via:

- Anthropic Claude (if `ANTHROPIC_API_KEY` set + `topic-mcp[claude]`
  extra installed).
- Local Ollama (if running + `topic-mcp[ollama]` extra installed).
- Heuristic top-keyword fallback otherwise.

By design **does not require any API key for the v1 install** — the boundary
detection alone is enough for the agent to navigate, and labels are nice-to-
have.

This indexer reads a sibling sidecar — a controlled cross-indexer
dependency. If the transcript is absent we emit `topics: []` with a
descriptive `note`, and the engine moves on. The agent can re-run
`awidat index --indexer topic` after `whisper` finishes.

Schema version: "1".
"""

from __future__ import annotations

import json
import os
import re
import sys
from collections import Counter
from pathlib import Path
from typing import Any

import numpy as np

from awidat_mcp import IndexAssetRequest, IndexerServer

INDEXER_NAME = "topic"
INDEXER_VERSION = "0.1.0"
SCHEMA_VERSION = "1"

# Sentence-window size for cosine-similarity boundary detection. 5 = good
# default for podcast / interview transcripts (≈ 30-60s of speech per
# window). Higher = coarser segmentation.
WINDOW_SENTENCES = 5

# Boundary threshold: cosine-similarity drops below this between adjacent
# windows mark a boundary. Tuned for spoken-word; tighter cuts on tighter
# threshold.
BOUNDARY_THRESHOLD = 0.55

# Minimum segment length. Short ones get merged with the neighbor.
MIN_SEGMENT_S = 30.0


server = IndexerServer(
    name=INDEXER_NAME,
    indexer_version=INDEXER_VERSION,
    schema_version=SCHEMA_VERSION,
)


def _find_transcript_path(asset_path: str, asset_id: str) -> Path | None:
    """Walk up from `asset_path` to find `<project>/index/whisper/<asset_id>.json`."""
    asset = Path(asset_path).resolve()
    for ancestor in asset.parents:
        candidate = ancestor / "index" / "whisper" / f"{asset_id}.json"
        if candidate.exists():
            return candidate
        # Stop if we found the project root (.awidat/ marker) without
        # finding the transcript.
        if (ancestor / ".awidat").exists():
            return None
    return None


def _extract_segments(transcript: dict[str, Any]) -> list[dict[str, Any]]:
    """Extract a flat list of speech segments from the whisper sidecar body."""
    body = transcript.get("data", transcript)
    segs = body.get("segments")
    if not isinstance(segs, list):
        return []
    out = []
    for s in segs:
        if not isinstance(s, dict):
            continue
        text = s.get("text") or ""
        start = s.get("start_s") or s.get("start") or 0.0
        end = s.get("end_s") or s.get("end") or 0.0
        if not text.strip():
            continue
        out.append({"text": text.strip(), "start_s": float(start), "end_s": float(end)})
    return out


def _embed(texts: list[str]) -> np.ndarray:
    # Lazy import — sentence-transformers loads ~80MB on first call.
    from sentence_transformers import SentenceTransformer

    model = SentenceTransformer("all-MiniLM-L6-v2")
    return model.encode(texts, normalize_embeddings=True, show_progress_bar=False)


def _segment_boundaries(segments: list[dict[str, Any]]) -> list[int]:
    """Return indices into `segments` where a topic boundary falls."""
    if len(segments) < 2 * WINDOW_SENTENCES:
        return []
    embeddings = _embed([s["text"] for s in segments])
    boundaries: list[int] = []
    n = len(segments)
    for i in range(WINDOW_SENTENCES, n - WINDOW_SENTENCES):
        left = embeddings[i - WINDOW_SENTENCES : i].mean(axis=0)
        right = embeddings[i : i + WINDOW_SENTENCES].mean(axis=0)
        # Cosine similarity (vectors are normalized).
        sim = float(np.dot(left, right))
        if sim < BOUNDARY_THRESHOLD:
            # Suppress duplicates within a small window.
            if not boundaries or i - boundaries[-1] > WINDOW_SENTENCES:
                boundaries.append(i)
    return boundaries


def _build_topics(
    segments: list[dict[str, Any]], boundaries: list[int]
) -> list[dict[str, Any]]:
    if not segments:
        return []
    cuts = [0, *boundaries, len(segments)]
    topics: list[dict[str, Any]] = []
    for i in range(len(cuts) - 1):
        a, b = cuts[i], cuts[i + 1]
        if b <= a:
            continue
        chunk = segments[a:b]
        start_s = chunk[0]["start_s"]
        end_s = chunk[-1]["end_s"]
        if end_s - start_s < MIN_SEGMENT_S and topics:
            # Merge into previous.
            topics[-1]["end_s"] = end_s
            topics[-1]["sentences"].extend(chunk)
            continue
        topics.append({"start_s": start_s, "end_s": end_s, "sentences": chunk})
    return topics


_STOPWORDS = {
    "the", "a", "an", "and", "or", "but", "to", "of", "for", "in", "on",
    "at", "with", "is", "are", "was", "were", "be", "been", "being",
    "this", "that", "these", "those", "i", "you", "he", "she", "it", "we",
    "they", "what", "which", "who", "do", "does", "did", "have", "has",
    "had", "so", "if", "as", "from", "by", "about", "just", "like",
    "really", "going", "yeah", "right", "okay", "know", "think", "thing",
    "things", "well", "lot", "actually",
}
_TOKEN = re.compile(r"[a-zA-Z][a-zA-Z\-']{2,}")


def _heuristic_label(sentences: list[dict[str, Any]]) -> str:
    text = " ".join(s["text"].lower() for s in sentences)
    tokens = [t for t in _TOKEN.findall(text) if t not in _STOPWORDS]
    if not tokens:
        return "untitled"
    counts = Counter(tokens)
    top = [w for w, _ in counts.most_common(3)]
    return ", ".join(top)


def _claude_label(sentences: list[dict[str, Any]]) -> str | None:
    """Best-effort Claude label. Returns None if unavailable."""
    if not os.environ.get("ANTHROPIC_API_KEY"):
        return None
    try:
        import anthropic  # type: ignore[import-not-found]
    except ImportError:
        return None
    text = " ".join(s["text"] for s in sentences)
    if len(text) > 4000:
        text = text[:2000] + " ... " + text[-2000:]
    try:
        client = anthropic.Anthropic()
        resp = client.messages.create(
            model="claude-haiku-4-5-20251001",
            max_tokens=80,
            messages=[
                {
                    "role": "user",
                    "content": (
                        "Give a 3-7 word topic title for this transcript "
                        "segment. Reply with only the title, no quotes, no "
                        "preamble.\n\n" + text
                    ),
                }
            ],
        )
        return resp.content[0].text.strip().rstrip(".")
    except Exception as e:  # noqa: BLE001 — best-effort labeling
        print(f"topic-mcp: Claude label failed: {e}", file=sys.stderr)
        return None


def _label(sentences: list[dict[str, Any]]) -> str:
    return _claude_label(sentences) or _heuristic_label(sentences)


@server.index_asset
def handle(req: IndexAssetRequest) -> dict[str, Any]:
    transcript_path = _find_transcript_path(req.asset_path, req.asset_id)
    if transcript_path is None:
        # **Raise** rather than returning an empty success — past
        # behavior wrote a `topics: []` sidecar with the asset's
        # SHA, and the engine's idempotency cache then skipped
        # topic forever after, even when whisper finished later.
        # Raising surfaces an MCP `is_error: true` to the engine,
        # which leaves no sidecar on disk so the next dispatcher
        # pass re-tries this pair. The agent / user re-runs
        # `awidat index --indexer topic` after whisper completes.
        raise RuntimeError(
            f"transcript sidecar not found at "
            f"<project>/index/whisper/{req.asset_id}.json; run whisper-mcp "
            f"first then re-run `awidat index --indexer topic`."
        )

    transcript = json.loads(transcript_path.read_text())
    segments = _extract_segments(transcript)
    if not segments:
        # Same reasoning as above: don't cache a no-segments empty
        # success; a future whisper retry might produce real
        # segments.
        raise RuntimeError(
            f"transcript at index/whisper/{req.asset_id}.json has no "
            f"segments — whisper may have failed mid-run"
        )

    boundaries = _segment_boundaries(segments)
    topics = _build_topics(segments, boundaries)
    for topic in topics:
        topic["label"] = _label(topic["sentences"])
        # Drop the heavy `sentences` list from the sidecar — agents read
        # them via the transcript sidecar instead. Keep just the count
        # here.
        topic["sentence_count"] = len(topic["sentences"])
        del topic["sentences"]

    return {
        "topics": topics,
        "boundary_threshold": BOUNDARY_THRESHOLD,
        "window_sentences": WINDOW_SENTENCES,
        "labeler": "claude" if os.environ.get("ANTHROPIC_API_KEY") else "heuristic",
    }


def main() -> None:
    server.run()
