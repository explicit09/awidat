"""Frame-quality indexer.

Per-second numerical scores for three axes of frame usability:

- `blur` — Laplacian variance. Low values = blurry. Threshold of ~100
  is the canonical "good vs bad" cutoff for typical interview footage.
- `brightness` — mean luma in [0, 255]. Helpful for "find dark moments
  / find blown-out exposure."
- `contrast` — luma std-dev. Low contrast often indicates flat/foggy
  footage that won't grade well.

The agent uses these to filter b-roll candidates ("only the sharp,
well-exposed seconds") and to flag bad takes for the user.

Schema version: "1".
"""

from __future__ import annotations

import logging
import subprocess
from typing import Any

import cv2
import numpy as np

from awidat_mcp import IndexAssetRequest, IndexerServer

INDEXER_NAME = "frame-quality"
INDEXER_VERSION = "0.1.0"
SCHEMA_VERSION = "1"

SAMPLE_FPS = 1.0
PROBE_W = 640

# Conventional Laplacian-variance threshold for "sharp" frames.
BLUR_SHARP_THRESHOLD = 100.0

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


def _extract_frames_bgr(
    asset_path: str, fps: float
) -> tuple[list[np.ndarray], int, int]:
    src_w, src_h = _probe_dims(asset_path)
    target_w = PROBE_W
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
        "bgr24",
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


def _score_frame(frame_bgr: np.ndarray) -> dict[str, float]:
    gray = cv2.cvtColor(frame_bgr, cv2.COLOR_BGR2GRAY)
    blur = float(cv2.Laplacian(gray, cv2.CV_64F).var())
    brightness = float(gray.mean())
    contrast = float(gray.std())
    return {
        "blur": blur,
        "brightness": brightness,
        "contrast": contrast,
        "is_sharp": blur >= BLUR_SHARP_THRESHOLD,
    }


@server.index_asset
def handle(req: IndexAssetRequest) -> dict[str, Any]:
    frames, w, h = _extract_frames_bgr(req.asset_path, SAMPLE_FPS)
    _log.info(
        "frame-quality-mcp: asset=%s frames=%d at %dx%d",
        req.asset_id,
        len(frames),
        w,
        h,
    )
    per_frame = [
        {"t_s": i / SAMPLE_FPS, **_score_frame(f)} for i, f in enumerate(frames)
    ]
    # Aggregate stats so the agent can read a one-line summary without
    # iterating per_frame.
    if per_frame:
        blurs = np.array([e["blur"] for e in per_frame])
        brights = np.array([e["brightness"] for e in per_frame])
        contrasts = np.array([e["contrast"] for e in per_frame])
        summary = {
            "blur_p10": float(np.percentile(blurs, 10)),
            "blur_p50": float(np.percentile(blurs, 50)),
            "blur_p90": float(np.percentile(blurs, 90)),
            "brightness_mean": float(brights.mean()),
            "contrast_mean": float(contrasts.mean()),
            "fraction_sharp": float((blurs >= BLUR_SHARP_THRESHOLD).mean()),
        }
    else:
        summary = {}
    return {
        "frame_rate_sampled": SAMPLE_FPS,
        "detect_width": w,
        "detect_height": h,
        "frame_count": len(frames),
        "blur_sharp_threshold": BLUR_SHARP_THRESHOLD,
        "summary": summary,
        "per_frame": per_frame,
    }


def main() -> None:
    server.run()
