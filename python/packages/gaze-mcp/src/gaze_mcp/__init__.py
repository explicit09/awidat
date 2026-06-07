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
import json
import subprocess
from collections.abc import Iterator
from pathlib import Path
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


def _read_face_sidecar(
    asset_path: str, asset_id: str, project_root: str | None = None
) -> dict[str, Any] | None:
    if project_root:
        candidate = Path(project_root).absolute() / "index" / "face" / f"{asset_id}.json"
        if candidate.exists():
            try:
                return json.loads(candidate.read_text()).get("data", {})
            except (OSError, json.JSONDecodeError):
                return None
    asset = Path(asset_path).absolute()
    for ancestor in asset.parents:
        candidate = ancestor / "index" / "face" / f"{asset_id}.json"
        if candidate.exists():
            try:
                return json.loads(candidate.read_text()).get("data", {})
            except (OSError, json.JSONDecodeError):
                return None
        if (ancestor / ".awidat").exists():
            return None
    return None


def _gaze_from_face_sidecar(face_body: dict[str, Any] | None) -> dict[str, Any] | None:
    if not face_body:
        return None
    per_frame = face_body.get("per_frame")
    if not isinstance(per_frame, list):
        return None
    out_frames = []
    has_gaze = False
    for entry in per_frame:
        if not isinstance(entry, dict):
            continue
        faces_out = []
        for face in entry.get("faces", []):
            if not isinstance(face, dict):
                continue
            if "gaze_score" not in face or "at_camera" not in face:
                continue
            has_gaze = True
            faces_out.append(
                {
                    "box": face.get("box", []),
                    "gaze_score": float(face["gaze_score"]),
                    "at_camera": bool(face["at_camera"]),
                }
            )
        out_frames.append({"t_s": float(entry.get("t_s", 0.0)), "faces": faces_out})
    if not has_gaze:
        return None
    return {
        "frame_rate_sampled": face_body.get("frame_rate_sampled", SAMPLE_FPS),
        "detect_width": face_body.get("detect_width", 0),
        "detect_height": face_body.get("detect_height", 0),
        "frame_count": face_body.get("frame_count", len(out_frames)),
        "at_camera_threshold": AT_CAMERA_THRESHOLD,
        "source": "face",
        "per_frame": out_frames,
    }


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


def _iter_frames(asset_path: str, fps: float) -> tuple[Iterator[np.ndarray], int, int]:
    src_w, src_h = _probe_dims(asset_path)
    target_w = DETECT_WIDTH
    target_h = int(round(src_h * target_w / src_w))
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
    fsize = target_w * target_h * 3

    def frames() -> Iterator[np.ndarray]:
        proc = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        assert proc.stdout is not None
        try:
            while True:
                chunk = proc.stdout.read(fsize)
                if not chunk:
                    break
                if len(chunk) != fsize:
                    raise RuntimeError(
                        f"ffmpeg produced a partial frame ({len(chunk)} of {fsize} bytes)"
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
    from_face = _gaze_from_face_sidecar(
        _read_face_sidecar(req.asset_path, req.asset_id, req.project_root)
    )
    if from_face is not None:
        return from_face

    frames, w, h = _iter_frames(req.asset_path, SAMPLE_FPS)
    _log.info(
        "gaze-mcp: asset=%s streaming frames at %dx%d", req.asset_id, w, h
    )

    per_frame: list[dict[str, Any]] = []
    frame_count = 0
    for i, frame in enumerate(frames):
        boxes = face_recognition.face_locations(frame, model="hog")
        if not boxes:
            per_frame.append({"t_s": i / SAMPLE_FPS, "faces": []})
            frame_count += 1
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
        frame_count += 1

    return {
        "frame_rate_sampled": SAMPLE_FPS,
        "detect_width": w,
        "detect_height": h,
        "frame_count": frame_count,
        "at_camera_threshold": AT_CAMERA_THRESHOLD,
        "per_frame": per_frame,
    }


def main() -> None:
    server.run()
