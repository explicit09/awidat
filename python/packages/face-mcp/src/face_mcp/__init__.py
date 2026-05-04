"""Face detection + speaker-to-face mapping indexer for Awidat.

Two-pass pipeline:

1. **Detect + embed.** Sample 1 fps via ffmpeg, run dlib's HOG face
   detector + 128-dim recognition encoder on each frame. Per-frame
   output: `[{box: [top, right, bottom, left], embedding}]`.

2. **Cluster + label.** All embeddings across the asset go through
   DBSCAN with cosine metric. Each cluster becomes a `face_id`.
   Singleton clusters are kept as their own id (people who only show
   up briefly still need to be track-able).

3. **Map to speakers.** If a whisper sidecar with diarization exists,
   for each speaker label (A/B/C/...) we count which face_id is
   on-screen during that speaker's segments. The face_id with the
   highest weighted overlap is mapped to that speaker. The heuristic
   beats the alternative (proper audio-visual sync) and is good enough
   for the editorial use-case ("show me when speaker B is on camera").

Schema version: "1".
"""

from __future__ import annotations

import base64
import json
import logging
import subprocess
from collections import Counter, defaultdict
from pathlib import Path
from typing import Any

import face_recognition
import numpy as np
from PIL import Image
from sklearn.cluster import DBSCAN

from awidat_mcp import IndexAssetRequest, IndexerServer

INDEXER_NAME = "face"
INDEXER_VERSION = "0.1.0"
SCHEMA_VERSION = "1"

# 1 fps mirrors clip-mcp — same time grid means a future joint query
# (e.g. "find frames where speaker B is on screen AND clip says
# 'pointing at phone'") aligns trivially.
SAMPLE_FPS = 1.0

# DBSCAN cosine epsilon. dlib face embeddings cluster cleanly at
# eps=0.4 in cosine distance per the face_recognition project README;
# tighter than that splits the same person across days; looser merges
# different people.
DBSCAN_EPS = 0.4
DBSCAN_MIN_SAMPLES = 1  # singleton clusters are valid (a guest who appears once)

# Frame size to detect on. dlib HOG works at native resolution but
# downscaling to 640px wide keeps a 4K stream fast without losing
# detection quality at typical interview-cam shot scales.
DETECT_WIDTH = 640

_log = logging.getLogger(INDEXER_NAME)


server = IndexerServer(
    name=INDEXER_NAME,
    indexer_version=INDEXER_VERSION,
    schema_version=SCHEMA_VERSION,
)


def _probe_video_dims(asset_path: str) -> tuple[int, int]:
    """Return (width, height) of the source video stream."""
    out = subprocess.run(
        [
            "ffprobe",
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height",
            "-of",
            "json",
            asset_path,
        ],
        check=True,
        capture_output=True,
        text=True,
    )
    streams = json.loads(out.stdout).get("streams", [])
    if not streams:
        raise RuntimeError(f"face-mcp: ffprobe found no video stream in {asset_path}")
    s = streams[0]
    return int(s["width"]), int(s["height"])


def _probe_duration_s(asset_path: str) -> float:
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


def _extract_frames(
    asset_path: str, fps: float, target_w: int
) -> tuple[list[np.ndarray], int, int]:
    """Stream rgb24 frames out of ffmpeg at `fps`, scaled so width is
    `target_w`. Returns (list_of_HxWx3_arrays, width, height)."""
    src_w, src_h = _probe_video_dims(asset_path)
    target_h = int(round(src_h * target_w / src_w))
    # Force even height — H.264 / yuv420p requires it; ffmpeg sometimes
    # rejects odd-height scale targets even when they're not encoded.
    target_h += target_h % 2
    cmd = [
        "ffmpeg",
        "-v",
        "error",
        "-i",
        asset_path,
        "-vf",
        f"fps={fps},scale={target_w}:{target_h}",
        "-pix_fmt",
        "rgb24",
        "-f",
        "rawvideo",
        "-",
    ]
    proc = subprocess.run(cmd, check=True, capture_output=True)
    raw = proc.stdout
    frame_size = target_w * target_h * 3
    n = len(raw) // frame_size
    frames: list[np.ndarray] = []
    for i in range(n):
        chunk = raw[i * frame_size : (i + 1) * frame_size]
        arr = np.frombuffer(chunk, dtype=np.uint8).reshape(target_h, target_w, 3).copy()
        frames.append(arr)
    return frames, target_w, target_h


def _detect_and_embed(
    frame: np.ndarray,
) -> list[tuple[tuple[int, int, int, int], np.ndarray]]:
    """Run face_recognition's HOG detector + 128-dim encoder on `frame`.
    Returns a list of ((top, right, bottom, left), embedding)."""
    boxes = face_recognition.face_locations(frame, model="hog")
    if not boxes:
        return []
    embs = face_recognition.face_encodings(frame, known_face_locations=boxes)
    return list(zip(boxes, embs, strict=True))


def _cluster_faces(
    all_embeddings: np.ndarray,
) -> np.ndarray:
    """DBSCAN over the L2-normalized embedding pool. Returns an array
    of cluster labels, one per input row. -1 (DBSCAN noise) is
    promoted to a unique singleton id so every detection is named."""
    if all_embeddings.size == 0:
        return np.zeros(0, dtype=np.int64)
    # face_recognition embeddings are not L2-normalized by default;
    # cosine DBSCAN expects unit vectors for the metric to be meaningful.
    norms = np.linalg.norm(all_embeddings, axis=1, keepdims=True)
    norms[norms == 0] = 1.0
    normed = all_embeddings / norms
    db = DBSCAN(eps=DBSCAN_EPS, min_samples=DBSCAN_MIN_SAMPLES, metric="cosine").fit(
        normed
    )
    labels = db.labels_.copy()
    # Replace -1 noise points with fresh ids so each unmatched detection
    # gets its own face_id (e.g. a passing extra still gets indexed).
    next_id = labels.max() + 1 if (labels >= 0).any() else 0
    for i, lbl in enumerate(labels):
        if lbl == -1:
            labels[i] = next_id
            next_id += 1
    return labels


def _read_diarization(
    project_root: Path, asset_id: str
) -> list[dict[str, Any]] | None:
    """Look up the whisper sidecar for this asset and return its
    diarization segments if present. None means no diarization
    available — speaker mapping just gets skipped."""
    sidecar = project_root / "index" / "whisper" / f"{asset_id}.json"
    if not sidecar.exists():
        return None
    try:
        doc = json.loads(sidecar.read_text())
    except (OSError, json.JSONDecodeError):
        return None
    segments = doc.get("data", {}).get("segments", [])
    # Only segments with a speaker label contribute to the mapping.
    return [
        {
            "start_s": float(s["start_s"]),
            "end_s": float(s["end_s"]),
            "speaker": s["speaker"],
        }
        for s in segments
        if s.get("speaker")
    ] or None


def _map_speakers_to_faces(
    per_frame: list[dict[str, Any]],
    diarization: list[dict[str, Any]] | None,
) -> dict[str, str]:
    """For each diarized speaker, pick the face_id most often on screen
    during that speaker's segments. Returns `{speaker_label: face_id}`."""
    if not diarization:
        return {}
    overlaps: dict[str, Counter] = defaultdict(Counter)
    for entry in per_frame:
        t = entry["t_s"]
        for seg in diarization:
            if seg["start_s"] <= t < seg["end_s"]:
                # Each face on screen contributes one vote for this speaker.
                for face_id in {f["face_id"] for f in entry["faces"]}:
                    overlaps[seg["speaker"]][face_id] += 1
    mapping: dict[str, str] = {}
    used: set[str] = set()
    # Greedy assign: highest-confidence speaker first; one face_id per
    # speaker (no two speakers map to the same face).
    ranked = sorted(
        overlaps.items(),
        key=lambda kv: max(kv[1].values()) if kv[1] else 0,
        reverse=True,
    )
    for speaker, counts in ranked:
        for face_id, _votes in counts.most_common():
            if face_id not in used:
                mapping[speaker] = face_id
                used.add(face_id)
                break
    return mapping


@server.index_asset
def handle(req: IndexAssetRequest) -> dict[str, Any]:
    duration_s = _probe_duration_s(req.asset_path)
    frames, w, h = _extract_frames(req.asset_path, SAMPLE_FPS, DETECT_WIDTH)
    _log.info(
        "face-mcp: asset=%s duration=%.2fs frames=%d at %dx%d",
        req.asset_id,
        duration_s,
        len(frames),
        w,
        h,
    )

    # Pass 1: detect + embed across all frames.
    pool: list[np.ndarray] = []  # all embeddings, in detection order
    per_frame_raw: list[list[dict[str, Any]]] = []  # boxes only, paired with pool
    for frame in frames:
        dets = _detect_and_embed(frame)
        per_frame_raw.append(
            [{"box": list(map(int, box))} for box, _emb in dets]
        )
        for _box, emb in dets:
            pool.append(emb)

    # Pass 2: cluster.
    pool_arr = np.array(pool) if pool else np.zeros((0, 128), dtype=np.float64)
    labels = _cluster_faces(pool_arr)

    # Stitch labels back onto the per-frame faces.
    cursor = 0
    per_frame: list[dict[str, Any]] = []
    for i, faces in enumerate(per_frame_raw):
        out_faces: list[dict[str, Any]] = []
        for f in faces:
            face_id = f"f_{int(labels[cursor])}"
            out_faces.append({"face_id": face_id, "box": f["box"]})
            cursor += 1
        per_frame.append({"t_s": i / SAMPLE_FPS, "faces": out_faces})

    # Pass 3: speaker-to-face mapping (cross-indexer; fail-soft).
    project_root = Path(req.asset_path).absolute()
    # Walk up to find the project root — the dir containing `index/`.
    while project_root != project_root.parent:
        if (project_root / "index").is_dir():
            break
        project_root = project_root.parent
    diarization = _read_diarization(project_root, req.asset_id)
    speaker_to_face = _map_speakers_to_faces(per_frame, diarization)

    # Track-level summary: total seconds each face_id is on screen.
    # Count each face_id at most once per frame — duplicate detections
    # in the same frame (rare; usually a profile + frontal of the same
    # person) shouldn't double-count.
    onscreen_s: dict[str, float] = defaultdict(float)
    for entry in per_frame:
        seen_in_frame = {f["face_id"] for f in entry["faces"]}
        for fid in seen_in_frame:
            onscreen_s[fid] += 1.0 / SAMPLE_FPS
    faces_summary = [
        {
            "face_id": fid,
            "onscreen_s": onscreen_s[fid],
            "speaker_label": next(
                (sp for sp, mapped_fid in speaker_to_face.items() if mapped_fid == fid),
                None,
            ),
        }
        for fid in sorted(onscreen_s.keys())
    ]

    return {
        "frame_rate_sampled": SAMPLE_FPS,
        "duration_s": duration_s,
        "detect_width": w,
        "detect_height": h,
        "frame_count": len(frames),
        "faces": faces_summary,
        "speaker_to_face": speaker_to_face,
        "per_frame": per_frame,
    }


def main() -> None:
    server.run()
