"""Gaze indexer — looking-at-camera vs off-camera.

Heuristic-only. dlib's 5-point face landmarks give us the eye outer
corners, nose tip, and mouth corners. We compute the horizontal
distance from each eye's center to the nose tip, normalized by face
width. When a person looks at the camera, the nose sits roughly
midway between the eyes; turning the head shifts that ratio.

Score range:
  -1.0 = looking far left
   0.0 = looking straight at camera
  +1.0 = looking far right

The agent uses this for "find the moments of direct address" (the
on-screen anchor signal) or "find the off-camera glances" (often
authentic emotional beats).

Schema version: "1".
"""

from __future__ import annotations

import logging
import subprocess
from typing import Any

import face_recognition
import numpy as np

from awidat_mcp import IndexAssetRequest, IndexerServer

INDEXER_NAME = "gaze"
INDEXER_VERSION = "0.1.0"
SCHEMA_VERSION = "1"

SAMPLE_FPS = 1.0
DETECT_WIDTH = 640

# Score absolute value below this → "at-camera". Empirically tuned on
# interview footage; with the 5-point landmark backend, head-turns
# below ~15° register as ~0.15 ratio shift.
AT_CAMERA_THRESHOLD = 0.15

_log = logging.getLogger(INDEXER_NAME)


server = IndexerServer(
    name=INDEXER_NAME,
    indexer_version=INDEXER_VERSION,
    schema_version=SCHEMA_VERSION,
)


def _probe_dims(asset_path: str) -> tuple[int, int]:
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
            "csv=p=0",
            asset_path,
        ],
        check=True,
        capture_output=True,
        text=True,
    )
    w, h = (int(x) for x in out.stdout.strip().split(","))
    return w, h


def _extract_frames(asset_path: str, fps: float) -> tuple[list[np.ndarray], int, int]:
    src_w, src_h = _probe_dims(asset_path)
    target_w = DETECT_WIDTH
    target_h = int(round(src_h * target_w / src_w))
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
    fsize = target_w * target_h * 3
    frames = []
    for i in range(len(raw) // fsize):
        chunk = raw[i * fsize : (i + 1) * fsize]
        frames.append(
            np.frombuffer(chunk, dtype=np.uint8).reshape(target_h, target_w, 3).copy()
        )
    return frames, target_w, target_h


def _gaze_score(landmarks: dict[str, list[tuple[int, int]]]) -> float:
    """Compute a looking-at-camera score from 5-point landmarks.

    Inputs:
      `nose_tip`        — 1 point
      `chin`            — present in 68-point but ignored here
      `left_eye`/`right_eye` — multiple points; we average to get center

    Returns horizontal offset (signed, normalized): positive = looking
    right, negative = looking left, ~0 = at camera.
    """
    if "nose_tip" not in landmarks or "left_eye" not in landmarks or "right_eye" not in landmarks:
        return 0.0
    nose = np.array(landmarks["nose_tip"]).mean(axis=0)
    left_eye = np.array(landmarks["left_eye"]).mean(axis=0)
    right_eye = np.array(landmarks["right_eye"]).mean(axis=0)
    midpoint = (left_eye + right_eye) / 2.0
    eye_distance = float(np.linalg.norm(right_eye - left_eye))
    if eye_distance < 1.0:
        return 0.0
    # Horizontal offset of nose from eye-midpoint, normalized by IPD.
    # Positive = nose right of midpoint = head turned right (subject's
    # right; viewer's left).
    return float(nose[0] - midpoint[0]) / eye_distance


@server.index_asset
def handle(req: IndexAssetRequest) -> dict[str, Any]:
    frames, w, h = _extract_frames(req.asset_path, SAMPLE_FPS)
    _log.info(
        "gaze-mcp: asset=%s frames=%d at %dx%d", req.asset_id, len(frames), w, h
    )

    per_frame: list[dict[str, Any]] = []
    for i, frame in enumerate(frames):
        boxes = face_recognition.face_locations(frame, model="hog")
        if not boxes:
            per_frame.append({"t_s": i / SAMPLE_FPS, "faces": []})
            continue
        landmark_sets = face_recognition.face_landmarks(frame, face_locations=boxes)
        faces_out = []
        for box, lm in zip(boxes, landmark_sets, strict=True):
            score = _gaze_score(lm)
            faces_out.append(
                {
                    "box": list(map(int, box)),
                    "gaze_score": score,
                    "at_camera": abs(score) < AT_CAMERA_THRESHOLD,
                }
            )
        per_frame.append({"t_s": i / SAMPLE_FPS, "faces": faces_out})

    return {
        "frame_rate_sampled": SAMPLE_FPS,
        "detect_width": w,
        "detect_height": h,
        "frame_count": len(frames),
        "at_camera_threshold": AT_CAMERA_THRESHOLD,
        "per_frame": per_frame,
    }


def main() -> None:
    server.run()
