"""Face detection + speaker-to-face mapping indexer for Awidat.

Two-pass pipeline:

1. **Detect + embed.** Sample frames via ffmpeg, run dlib's HOG face
   detector, landmarks, gaze heuristic, and 128-dim recognition encoder
   on each frame. Per-frame output includes box, face id, gaze score,
   and at-camera flag.

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
import os
import subprocess
import time
from collections.abc import Iterator
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

# One frame every four seconds by default. Face detection is one of the most
# expensive visual passes; this keeps long-form auto-indexing responsive while
# preserving enough speaker/face evidence for agent context. Set
# FACE_SAMPLE_FPS=0.5 or higher for a deeper manual pass.
SAMPLE_FPS = float(os.environ.get("FACE_SAMPLE_FPS", "0.25"))

# DBSCAN cosine epsilon. dlib face embeddings cluster cleanly at
# eps=0.4 in cosine distance per the face_recognition project README;
# tighter than that splits the same person across days; looser merges
# different people.
DBSCAN_EPS = 0.4
DBSCAN_MIN_SAMPLES = 1  # singleton clusters are valid (a guest who appears once)

# Frame size to detect on. dlib HOG works at native resolution but
# downscaling keeps a 4K stream fast. The default of 480 was too
# aggressive for multi-cam podcast wide shots: three speakers each
# occupying 5-10% of frame width became 24-48px faces, below HOG's
# ~80px floor. Result: wide shots were classified `no-face` even
# though three people were visible, which cascaded into `broll_
# candidates` returning multi-cam wide-shot moments as "B-roll"
# candidates — exactly the wrong kind of footage.
# 960 puts the same 5-10% face at 48-96px, in HOG's reliable range.
# Cost: ~4x more pixels per detect, ~2x wallclock per frame, acceptable
# at the 0.25fps sample rate.
DETECT_WIDTH = int(os.environ.get("FACE_DETECT_WIDTH", "960"))

AT_CAMERA_THRESHOLD = 0.15

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


def _iter_frames(
    asset_path: str, fps: float, target_w: int
) -> tuple[Iterator[np.ndarray], int, int]:
    """Stream rgb24 frames out of ffmpeg at `fps`, scaled so width is
    `target_w`. Returns (frame_iterator, width, height)."""
    src_w, src_h = _probe_video_dims(asset_path)
    target_h = int(round(src_h * target_w / src_w))
    # Force even height — H.264 / yuv420p requires it; ffmpeg sometimes
    # rejects odd-height scale targets even when they're not encoded.
    target_h += target_h % 2
    cmd = [
        "ffmpeg",
        "-nostdin",
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
    frame_size = target_w * target_h * 3

    def frames() -> Iterator[np.ndarray]:
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
                yield np.frombuffer(chunk, dtype=np.uint8).reshape(target_h, target_w, 3)
            stderr = proc.stderr.read().decode("utf-8", errors="replace") if proc.stderr else ""
            rc = proc.wait()
            if rc != 0:
                raise RuntimeError(
                    f"ffmpeg frame extraction failed (exit {rc}): {stderr.strip()}"
                )
        finally:
            if proc.poll() is None:
                proc.kill()
                proc.wait()

    return frames(), target_w, target_h


def _detect_and_embed(
    frame: np.ndarray,
) -> list[tuple[tuple[int, int, int, int], np.ndarray, float, bool]]:
    """Run HOG face detection, landmarks, gaze, and 128-dim embedding."""
    boxes = face_recognition.face_locations(frame, model="hog")
    if not boxes:
        return []
    landmark_sets = face_recognition.face_landmarks(frame, face_locations=boxes)
    embs = face_recognition.face_encodings(frame, known_face_locations=boxes)
    out = []
    for box, emb, landmarks in zip(boxes, embs, landmark_sets, strict=True):
        score = _gaze_score(landmarks)
        out.append((box, emb, score, abs(score) < AT_CAMERA_THRESHOLD))
    return out


def _gaze_score(landmarks: dict[str, list[tuple[int, int]]]) -> float:
    if "nose_tip" not in landmarks or "left_eye" not in landmarks or "right_eye" not in landmarks:
        return 0.0
    nose = np.array(landmarks["nose_tip"]).mean(axis=0)
    left_eye = np.array(landmarks["left_eye"]).mean(axis=0)
    right_eye = np.array(landmarks["right_eye"]).mean(axis=0)
    midpoint = (left_eye + right_eye) / 2.0
    eye_distance = float(np.linalg.norm(right_eye - left_eye))
    if eye_distance < 1.0:
        return 0.0
    return float(nose[0] - midpoint[0]) / eye_distance


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
    project_root: Path | None, asset_id: str, index_root: Path | None = None
) -> list[dict[str, Any]] | None:
    """Look up the whisper sidecar for this asset and return its
    diarization segments if present. None means no diarization
    available — speaker mapping just gets skipped."""
    if index_root is not None:
        sidecar = index_root / "whisper" / f"{asset_id}.json"
    elif project_root is not None:
        sidecar = project_root / "index" / "whisper" / f"{asset_id}.json"
    else:
        return None
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
    probe_start = time.perf_counter()
    duration_s = _probe_duration_s(req.asset_path)
    probe_ms = round((time.perf_counter() - probe_start) * 1000)
    setup_start = time.perf_counter()
    frames, w, h = _iter_frames(req.asset_path, SAMPLE_FPS, DETECT_WIDTH)
    setup_ms = round((time.perf_counter() - setup_start) * 1000)
    _log.info(
        "face-mcp: asset=%s duration=%.2fs streaming frames at %dx%d",
        req.asset_id,
        duration_s,
        w,
        h,
    )

    # Pass 1: detect + embed across streamed frames. Keep only compact
    # detection metadata and embeddings, not the decoded frame list.
    pool: list[np.ndarray] = []  # all embeddings, in detection order
    per_frame_raw: list[list[dict[str, Any]]] = []  # boxes only, paired with pool
    frame_count = 0
    decode_read_ms = 0
    inference_ms = 0
    iterator = iter(frames)
    while True:
        read_start = time.perf_counter()
        try:
            frame = next(iterator)
        except StopIteration:
            decode_read_ms += round((time.perf_counter() - read_start) * 1000)
            break
        decode_read_ms += round((time.perf_counter() - read_start) * 1000)
        inference_start = time.perf_counter()
        dets = _detect_and_embed(frame)
        inference_ms += round((time.perf_counter() - inference_start) * 1000)
        per_frame_raw.append(
            [
                {
                    "box": list(map(int, box)),
                    "gaze_score": score,
                    "at_camera": at_camera,
                }
                for box, _emb, score, at_camera in dets
            ]
        )
        for _box, emb, _score, _at_camera in dets:
            pool.append(emb)
        frame_count += 1

    # Pass 2: cluster.
    cluster_start = time.perf_counter()
    pool_arr = np.array(pool) if pool else np.zeros((0, 128), dtype=np.float64)
    labels = _cluster_faces(pool_arr)
    cluster_ms = round((time.perf_counter() - cluster_start) * 1000)

    # Stitch labels back onto the per-frame faces.
    stitch_start = time.perf_counter()
    cursor = 0
    per_frame: list[dict[str, Any]] = []
    for i, faces in enumerate(per_frame_raw):
        out_faces: list[dict[str, Any]] = []
        for f in faces:
            face_id = f"f_{int(labels[cursor])}"
            out_faces.append(
                {
                    "face_id": face_id,
                    "box": f["box"],
                    "gaze_score": f["gaze_score"],
                    "at_camera": f["at_camera"],
                }
            )
            cursor += 1
        per_frame.append({"t_s": i / SAMPLE_FPS, "faces": out_faces})
    stitch_ms = round((time.perf_counter() - stitch_start) * 1000)

    # Pass 3: speaker-to-face mapping (cross-indexer; fail-soft).
    speaker_start = time.perf_counter()
    project_root = Path(req.project_root).absolute() if req.project_root else None
    if project_root is None:
        project_root = Path(req.asset_path).absolute()
        # Walk up to find the project root — the dir containing `index/`.
        while project_root != project_root.parent:
            if (project_root / "index").is_dir():
                break
            project_root = project_root.parent
    index_root = Path(req.index_root).absolute() if req.index_root else None
    diarization = _read_diarization(project_root, req.asset_id, index_root)
    speaker_to_face = _map_speakers_to_faces(per_frame, diarization)
    speaker_map_ms = round((time.perf_counter() - speaker_start) * 1000)

    # Track-level summary: total seconds each face_id is on screen.
    # Count each face_id at most once per frame — duplicate detections
    # in the same frame (rare; usually a profile + frontal of the same
    # person) shouldn't double-count.
    summary_start = time.perf_counter()
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
    summary_ms = round((time.perf_counter() - summary_start) * 1000)

    return {
        "frame_rate_sampled": SAMPLE_FPS,
        "duration_s": duration_s,
        "detect_width": w,
        "detect_height": h,
        "frame_count": frame_count,
        "faces": faces_summary,
        "speaker_to_face": speaker_to_face,
        "per_frame": per_frame,
        "perf": {
            "probe_ms": probe_ms,
            "setup_ms": setup_ms,
            "decode_read_ms": decode_read_ms,
            "inference_ms": inference_ms,
            "cluster_ms": cluster_ms,
            "stitch_ms": stitch_ms,
            "speaker_map_ms": speaker_map_ms,
            "summary_ms": summary_ms,
            "frames_processed": frame_count,
        },
    }


def main() -> None:
    server.run()
