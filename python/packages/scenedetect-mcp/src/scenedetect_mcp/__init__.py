"""Fast shot-boundary indexer for Awidat.

Emits `shots[]` — contiguous source-time ranges the agent can use for
cut-safe navigation. The default path samples scaled frames with ffmpeg
instead of running PySceneDetect across every source frame. That keeps
long 4K interviews usable in the desktop re-run loop.

Set `SCENEDETECT_BACKEND=pyscenedetect` to use the legacy full-frame
PySceneDetect implementation for offline benchmarking.
"""

from __future__ import annotations

import logging
import os
import subprocess
import time
from collections.abc import Iterator
from fractions import Fraction
from typing import Any

import cv2
import numpy as np
from scenedetect import ContentDetector, SceneManager, open_video

from awidat_mcp import IndexAssetRequest, IndexerServer

INDEXER_NAME = "scenedetect"
INDEXER_VERSION = "0.2.0"
SCHEMA_VERSION = "1"

BACKEND = os.environ.get("SCENEDETECT_BACKEND", "sampled").lower()
SAMPLE_FPS = float(os.environ.get("SCENEDETECT_SAMPLE_FPS", "1.0"))
PROBE_W = int(os.environ.get("SCENEDETECT_PROBE_W", "320"))
THRESHOLD = float(os.environ.get("SCENEDETECT_THRESHOLD", "27.0"))
MIN_SHOT_S = float(os.environ.get("SCENEDETECT_MIN_SHOT_S", "0.4"))

_log = logging.getLogger(INDEXER_NAME)


server = IndexerServer(
    name=INDEXER_NAME,
    indexer_version=INDEXER_VERSION,
    schema_version=SCHEMA_VERSION,
)


def _probe_video(asset_path: str) -> tuple[int, int, float, float]:
    out = subprocess.run(
        [
            "ffprobe",
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height,avg_frame_rate:format=duration",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
            asset_path,
        ],
        check=True,
        capture_output=True,
        text=True,
    )
    lines = [line.strip() for line in out.stdout.splitlines() if line.strip()]
    width = int(lines[0])
    height = int(lines[1])
    frame_rate = float(Fraction(lines[2])) if len(lines) > 2 and lines[2] != "0/0" else 0.0
    duration_s = float(lines[3]) if len(lines) > 3 else 0.0
    return width, height, frame_rate, duration_s


def _extract_frames_bgr(
    asset_path: str, fps: float
) -> tuple[Iterator[np.ndarray], int, int, float, float]:
    src_w, src_h, frame_rate, duration_s = _probe_video(asset_path)
    target_w = PROBE_W
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
        "bgr24",
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

    return frames(), target_w, target_h, frame_rate, duration_s


def _frame_score(prev_bgr: np.ndarray, curr_bgr: np.ndarray) -> float:
    prev = cv2.resize(prev_bgr, (160, 90), interpolation=cv2.INTER_AREA)
    curr = cv2.resize(curr_bgr, (160, 90), interpolation=cv2.INTER_AREA)
    prev_gray = cv2.cvtColor(prev, cv2.COLOR_BGR2GRAY)
    curr_gray = cv2.cvtColor(curr, cv2.COLOR_BGR2GRAY)
    luma_delta = float(cv2.absdiff(prev_gray, curr_gray).mean())

    prev_hsv = cv2.cvtColor(prev, cv2.COLOR_BGR2HSV)
    curr_hsv = cv2.cvtColor(curr, cv2.COLOR_BGR2HSV)
    prev_hist = cv2.calcHist([prev_hsv], [0, 1], None, [24, 16], [0, 180, 0, 256])
    curr_hist = cv2.calcHist([curr_hsv], [0, 1], None, [24, 16], [0, 180, 0, 256])
    cv2.normalize(prev_hist, prev_hist, 0, 1, cv2.NORM_MINMAX)
    cv2.normalize(curr_hist, curr_hist, 0, 1, cv2.NORM_MINMAX)
    hist_delta = max(0.0, 1.0 - float(cv2.compareHist(prev_hist, curr_hist, cv2.HISTCMP_CORREL)))

    return luma_delta + hist_delta * 18.0


def _sampled_shots(asset_path: str) -> dict[str, Any]:
    setup_start = time.perf_counter()
    frames, w, h, frame_rate, duration_s = _extract_frames_bgr(asset_path, SAMPLE_FPS)
    setup_ms = round((time.perf_counter() - setup_start) * 1000)
    _log.info(
        "scenedetect-mcp: sampled backend streaming frames at %.2ffps %dx%d",
        SAMPLE_FPS,
        w,
        h,
    )
    cuts: list[float] = []
    prev: np.ndarray | None = None
    last_cut = 0.0
    frame_count = 0
    decode_read_ms = 0
    analysis_ms = 0
    iterator = iter(frames)
    idx = 0
    while True:
        read_start = time.perf_counter()
        try:
            frame = next(iterator)
        except StopIteration:
            decode_read_ms += round((time.perf_counter() - read_start) * 1000)
            break
        decode_read_ms += round((time.perf_counter() - read_start) * 1000)
        t_s = idx / SAMPLE_FPS
        if prev is not None:
            analysis_start = time.perf_counter()
            score = _frame_score(prev, frame)
            analysis_ms += round((time.perf_counter() - analysis_start) * 1000)
            if score >= THRESHOLD and t_s - last_cut >= MIN_SHOT_S:
                cuts.append(t_s)
                last_cut = t_s
        prev = frame
        frame_count += 1
        idx += 1

    if duration_s <= 0.0 and frame_count > 0:
        duration_s = frame_count / SAMPLE_FPS

    boundaries = [0.0, *cuts, duration_s]
    shots: list[dict[str, float | int]] = []
    for idx, (start_s, end_s) in enumerate(zip(boundaries, boundaries[1:])):
        if end_s - start_s < MIN_SHOT_S:
            continue
        shots.append(
            {
                "index": idx,
                "start_s": start_s,
                "end_s": end_s,
                "start_frame": int(round(start_s * frame_rate)) if frame_rate else idx,
                "end_frame": int(round(end_s * frame_rate)) if frame_rate else idx + 1,
            }
        )

    return {
        "frame_rate": frame_rate,
        "duration_s": duration_s,
        "threshold": THRESHOLD,
        "backend": "sampled-ffmpeg",
        "frame_rate_sampled": SAMPLE_FPS,
        "detect_width": w,
        "detect_height": h,
        "frame_count": frame_count,
        "shots": shots,
        "perf": {
            "setup_ms": setup_ms,
            "decode_read_ms": decode_read_ms,
            "analysis_ms": analysis_ms,
            "frames_processed": frame_count,
        },
    }


def _pyscenedetect_shots(asset_path: str) -> dict[str, Any]:
    video = open_video(asset_path)
    sm = SceneManager()
    sm.add_detector(ContentDetector(threshold=THRESHOLD, min_scene_len=15))
    sm.detect_scenes(video=video, show_progress=False)
    scenes = sm.get_scene_list()

    shots: list[dict[str, float | int]] = []
    for idx, (start, end) in enumerate(scenes):
        start_s = float(start.get_seconds())
        end_s = float(end.get_seconds())
        if end_s - start_s < MIN_SHOT_S:
            continue
        shots.append(
            {
                "index": idx,
                "start_s": start_s,
                "end_s": end_s,
                "start_frame": int(start.get_frames()),
                "end_frame": int(end.get_frames()),
            }
        )

    return {
        "frame_rate": float(video.frame_rate),
        "duration_s": float(video.duration.get_seconds()),
        "threshold": THRESHOLD,
        "backend": "pyscenedetect",
        "shots": shots,
    }


@server.index_asset
def handle(req: IndexAssetRequest) -> dict[str, Any]:
    if BACKEND == "pyscenedetect":
        return _pyscenedetect_shots(req.asset_path)
    return _sampled_shots(req.asset_path)


def main() -> None:
    server.run()
