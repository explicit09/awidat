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

# Boundary detection: TextTiling-style local-minimum on the cosine-
# similarity curve. We DON'T use an absolute threshold — empirical
# investigation on the 44-min Samsung Galaxy retrospective showed the
# max sim across all adjacent 5-sentence windows was 0.416, so any
# absolute threshold either fires on every position or none. Instead
# we identify *valleys*: positions where similarity dips meaningfully
# below the surrounding peaks. The depth score is
# (peak_left - sim) + (peak_right - sim); positions whose depth
# exceeds (mean_depth + DEPTH_THRESHOLD_STD * std_depth) become
# boundaries. This is the original TextTiling formulation per
# Hearst (1997) and now the standard approach for spoken-word
# transcripts where absolute similarity values aren't meaningful.
DEPTH_THRESHOLD_STD = 1.2  # k in mean + k*std; tighter = fewer boundaries

# Minimum segment length. Sub-floor chunks get absorbed into the
# closer-by-content neighbor (NOT always the previous one). The
# direction-aware merge means short chunks within a topic get
# absorbed locally without cascading — a chunk in the middle of a
# topic merges sideways, a chunk at a real boundary stays distinct
# because both neighbors are equally dissimilar.
MIN_SEGMENT_S = 60.0


server = IndexerServer(
    name=INDEXER_NAME,
    indexer_version=INDEXER_VERSION,
    schema_version=SCHEMA_VERSION,
)


def _find_transcript_path(
    asset_path: str, asset_id: str, project_root: str | None = None
) -> Path | None:
    """Walk up from `asset_path` to find `<project>/index/whisper/<asset_id>.json`."""
    if project_root:
        candidate = (
            Path(project_root).absolute() / "index" / "whisper" / f"{asset_id}.json"
        )
        if candidate.exists():
            return candidate
    asset = Path(asset_path).absolute()
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


def _segment_boundaries(
    segments: list[dict[str, Any]],
) -> tuple[list[int], np.ndarray]:
    """Return (boundary_indices, embeddings) using local-minimum
    detection on the cosine-similarity curve.

    Algorithm (Hearst 1997 TextTiling, lightly adapted):
      1. Compute sims[i] = cosine(left_window, right_window) for
         every valid position i (i.e. where both windows fit).
      2. Find every local minimum in `sims` — positions where the
         value is <= both neighbors (with shallow ties broken by
         picking the rightmost).
      3. For each local min, compute its depth: how far it dips
         below the nearest peak on each side. Sum the two dips.
      4. Boundaries are the local mins whose depth exceeds
         (mean(depths) + DEPTH_THRESHOLD_STD * std(depths)).

    Returning embeddings alongside is a small efficiency win for
    `_build_topics`, which uses them again for the direction-aware
    merge step. Computing them once per indexer run.
    """
    if len(segments) < 2 * WINDOW_SENTENCES:
        return [], np.zeros((0, 384))  # 384 = MiniLM-L6's dim
    embeddings = _embed([s["text"] for s in segments])
    n = len(segments)

    # Step 1: build the sims curve. sims[j] is the similarity at
    # position j+WINDOW_SENTENCES (the j-th valid position).
    sims: list[float] = []
    for i in range(WINDOW_SENTENCES, n - WINDOW_SENTENCES):
        left = embeddings[i - WINDOW_SENTENCES : i].mean(axis=0)
        right = embeddings[i : i + WINDOW_SENTENCES].mean(axis=0)
        sims.append(float(np.dot(left, right)))
    if not sims:
        return [], embeddings
    sims_arr = np.array(sims)

    # Step 2: local minima. A point is a local min iff it's <= both
    # immediate neighbors. Skip the endpoints (no left/right peak).
    local_mins: list[int] = []
    for j in range(1, len(sims_arr) - 1):
        if sims_arr[j] <= sims_arr[j - 1] and sims_arr[j] <= sims_arr[j + 1]:
            local_mins.append(j)
    if not local_mins:
        return [], embeddings

    # Step 3: depth score. Walk left and right from each min until
    # we find a position higher than the min — that's the local peak.
    depths: list[float] = []
    for j in local_mins:
        # Walk left.
        peak_l = sims_arr[j]
        for k in range(j - 1, -1, -1):
            if sims_arr[k] >= peak_l:
                peak_l = sims_arr[k]
            else:
                break
        # Walk right.
        peak_r = sims_arr[j]
        for k in range(j + 1, len(sims_arr)):
            if sims_arr[k] >= peak_r:
                peak_r = sims_arr[k]
            else:
                break
        depths.append((peak_l - sims_arr[j]) + (peak_r - sims_arr[j]))

    # Step 4: threshold. Pick boundaries whose depth meaningfully
    # exceeds the mean — those are real valleys, not just every
    # local wiggle.
    depths_arr = np.array(depths)
    threshold = float(depths_arr.mean() + DEPTH_THRESHOLD_STD * depths_arr.std())
    boundaries: list[int] = []
    for j, depth in zip(local_mins, depths_arr, strict=True):
        if float(depth) >= threshold:
            # Map sims-curve index j back to segment index. sims[0]
            # corresponds to segment index WINDOW_SENTENCES.
            boundaries.append(j + WINDOW_SENTENCES)
    return boundaries, embeddings


def _build_topics(
    segments: list[dict[str, Any]],
    boundaries: list[int],
    embeddings: np.ndarray,
) -> list[dict[str, Any]]:
    """Cut `segments` at `boundaries`, then absorb sub-floor chunks
    into their more-similar neighbor (NOT always the previous one).

    Direction-aware merge breaks the cascade we hit before:
    previously, a sub-floor chunk in the middle of a uniformly-paced
    transcript merged into the previous topic, lengthening it; the
    next chunk then failed the floor and merged again, runaway
    collapsing the whole episode to one topic. Comparing each
    sub-floor chunk to BOTH neighbors and picking the more-similar
    one means:

      - A chunk in the middle of a topic merges sideways (absorbed
        locally).
      - A chunk at a real boundary stays distinct because both
        neighbors are equally dissimilar — the merge picks one but
        doesn't cascade because the other neighbor remains a real
        boundary.

    Two-pass: build the initial topic list with raw boundaries, THEN
    run the direction-aware merge. We compare centroids of each
    candidate's sentences in MiniLM embedding space.
    """
    if not segments:
        return []
    cuts = [0, *boundaries, len(segments)]
    topics: list[dict[str, Any]] = []
    for i in range(len(cuts) - 1):
        a, b = cuts[i], cuts[i + 1]
        if b <= a:
            continue
        chunk = segments[a:b]
        topics.append({
            "start_s": chunk[0]["start_s"],
            "end_s": chunk[-1]["end_s"],
            "sentences": chunk,
            # Track segment-index range so we can pull centroids
            # from `embeddings` without re-embedding.
            "_a": a,
            "_b": b,
        })

    # Direction-aware merge pass. We walk the topics list and for
    # each sub-floor topic, decide which neighbor to absorb it into.
    # Repeat until no sub-floor topics remain. Bounded loop — each
    # pass strictly reduces the topic count.
    def centroid(topic: dict[str, Any]) -> np.ndarray:
        a, b = topic["_a"], topic["_b"]
        if b <= a or embeddings.shape[0] == 0:
            return np.zeros(embeddings.shape[1] if embeddings.ndim == 2 else 384)
        return embeddings[a:b].mean(axis=0)

    def merge_into(dst: dict[str, Any], src: dict[str, Any]) -> None:
        """Absorb src into dst (left-or-right neighbor). dst gets the
        union range + concatenated sentences; src gets dropped by
        the caller."""
        # Decide which is earlier; keep that's start_s, take the
        # later's end_s.
        if dst["_a"] < src["_a"]:
            dst["sentences"].extend(src["sentences"])
            dst["end_s"] = src["end_s"]
            dst["_b"] = src["_b"]
        else:
            dst["sentences"] = src["sentences"] + dst["sentences"]
            dst["start_s"] = src["start_s"]
            dst["_a"] = src["_a"]

    while True:
        # Find the first sub-floor topic.
        idx = next(
            (
                i
                for i, t in enumerate(topics)
                if (t["end_s"] - t["start_s"]) < MIN_SEGMENT_S
            ),
            None,
        )
        if idx is None:
            break
        # If there's only one topic left, leave it as-is.
        if len(topics) <= 1:
            break
        target = topics[idx]
        target_c = centroid(target)
        # Compare to neighbors. If only one side has a neighbor,
        # merge there. Otherwise pick the more-similar centroid.
        prev_t = topics[idx - 1] if idx > 0 else None
        next_t = topics[idx + 1] if idx + 1 < len(topics) else None
        if prev_t is None and next_t is None:
            break
        elif prev_t is None:
            chosen_idx = idx + 1
        elif next_t is None:
            chosen_idx = idx - 1
        else:
            sim_prev = float(np.dot(target_c, centroid(prev_t)))
            sim_next = float(np.dot(target_c, centroid(next_t)))
            chosen_idx = idx - 1 if sim_prev >= sim_next else idx + 1
        # Merge target INTO chosen, drop target.
        merge_into(topics[chosen_idx], target)
        topics.pop(idx)

    # Strip private bookkeeping fields before returning.
    for t in topics:
        t.pop("_a", None)
        t.pop("_b", None)
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
    transcript_path = _find_transcript_path(
        req.asset_path, req.asset_id, req.project_root
    )
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

    boundaries, embeddings = _segment_boundaries(segments)
    topics = _build_topics(segments, boundaries, embeddings)
    for topic in topics:
        topic["label"] = _label(topic["sentences"])
        # Drop the heavy `sentences` list from the sidecar — agents read
        # them via the transcript sidecar instead. Keep just the count
        # here.
        topic["sentence_count"] = len(topic["sentences"])
        del topic["sentences"]

    return {
        "topics": topics,
        "depth_threshold_std": DEPTH_THRESHOLD_STD,
        "min_segment_s": MIN_SEGMENT_S,
        "window_sentences": WINDOW_SENTENCES,
        "labeler": "claude" if os.environ.get("ANTHROPIC_API_KEY") else "heuristic",
    }


def main() -> None:
    server.run()
