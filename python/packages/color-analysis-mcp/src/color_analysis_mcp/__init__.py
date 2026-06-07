"""Color-analysis indexer.

Samples video frames at 1 fps and emits compact exposure/color metrics
plus normalized correction recommendations. The output is meant to feed
graph-native `Set Color Correction` EDL ops, not one-off FFmpeg scripts.

Schema version: "1".
"""

from __future__ import annotations

import logging
import os
import subprocess
import time
from collections.abc import Iterator
from typing import Any

import cv2
import numpy as np

from awidat_mcp import IndexAssetRequest, IndexerServer

INDEXER_NAME = "color-analysis"
INDEXER_VERSION = "0.1.0"
SCHEMA_VERSION = "1"

SAMPLE_FPS = float(os.environ.get("COLOR_ANALYSIS_SAMPLE_FPS", "0.25"))
PROBE_W = int(os.environ.get("COLOR_ANALYSIS_PROBE_W", "320"))
SCENE_MAX_GAP_S = 12.0

_log = logging.getLogger(INDEXER_NAME)

server = IndexerServer(
    name=INDEXER_NAME,
    indexer_version=INDEXER_VERSION,
    schema_version=SCHEMA_VERSION,
)


def _clamp(value: float, lo: float, hi: float) -> float:
    return max(lo, min(hi, value))


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
    dims = [int(x) for x in out.stdout.replace("\n", ",").split(",") if x.strip()]
    if len(dims) < 2:
        raise RuntimeError(f"ffprobe did not report width/height for {asset_path!r}")
    w, h = dims[:2]
    return w, h


def _extract_frames_bgr(
    asset_path: str, fps: float
) -> tuple[Iterator[np.ndarray], int, int]:
    src_w, src_h = _probe_dims(asset_path)
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


def _estimate_cast(mean_r: float, mean_g: float, mean_b: float) -> str:
    rb_delta = mean_r - mean_b
    gm_delta = mean_g - ((mean_r + mean_b) / 2.0)
    if abs(rb_delta) < 6.0 and abs(gm_delta) < 5.0:
        return "neutral"
    if abs(rb_delta) >= abs(gm_delta):
        return "warm" if rb_delta > 0 else "cool"
    return "green" if gm_delta > 0 else "magenta"


def _issue_tags(
    brightness: float,
    contrast: float,
    mean_r: float,
    mean_g: float,
    mean_b: float,
    overexposed_fraction: float,
    underexposed_fraction: float,
    luma_p05: float,
    luma_p95: float,
) -> list[str]:
    tags: list[str] = []
    if brightness < 80.0 or luma_p95 < 130.0:
        tags.append("underexposed")
    if brightness > 178.0 or luma_p05 > 145.0:
        tags.append("overexposed")
    if contrast < 28.0 or (luma_p95 - luma_p05) < 72.0:
        tags.append("low_contrast")

    cast = _estimate_cast(mean_r, mean_g, mean_b)
    if cast != "neutral":
        tags.append(f"{cast}_cast")

    if overexposed_fraction > 0.08:
        tags.append("clipped_highlights")
    if underexposed_fraction > 0.08:
        tags.append("crushed_shadows")

    if not tags:
        tags.append("already_balanced")
    return tags


def _auto_correct_safe(
    tags: list[str],
    overexposed_fraction: float,
    underexposed_fraction: float,
    ignored_fraction: float,
) -> bool:
    if "already_balanced" in tags:
        return False
    if ignored_fraction > 0.4:
        return False
    if overexposed_fraction > 0.18 or underexposed_fraction > 0.24:
        return False
    if "clipped_highlights" in tags and "crushed_shadows" in tags:
        return False
    return True


def _confidence(
    tags: list[str],
    overexposed_fraction: float,
    underexposed_fraction: float,
    ignored_fraction: float,
) -> float:
    score = 0.92
    score -= min(0.28, ignored_fraction * 0.45)
    score -= min(0.24, overexposed_fraction * 1.2)
    score -= min(0.24, underexposed_fraction * 0.9)
    if "already_balanced" in tags:
        score = min(score, 0.75)
    if "clipped_highlights" in tags or "crushed_shadows" in tags:
        score -= 0.08
    if "unsafe_to_auto_correct" in tags:
        score = min(score, 0.35)
    return float(_clamp(score, 0.0, 1.0))


def _edit_types(tags: list[str]) -> list[str]:
    edits = []
    if any(
        tag in tags
        for tag in [
            "underexposed",
            "overexposed",
            "clipped_highlights",
            "crushed_shadows",
        ]
    ):
        edits.append("lighting_correction")
    if "low_contrast" in tags:
        edits.append("contrast_correction")
    if any(
        tag in tags
        for tag in ["warm_cast", "cool_cast", "green_cast", "magenta_cast"]
    ):
        edits.append("white_balance")
    return edits


def _policy_from_summary(summary: dict[str, Any]) -> dict[str, Any]:
    tags = list(summary.get("issue_tags", []))
    confidence = float(summary.get("confidence", 0.0))
    safe = bool(summary.get("auto_correct_safe", False))
    edit_types = _edit_types(tags)

    if "already_balanced" in tags:
        action = "no_op"
        requires_review = False
        requires_contact_sheet = False
        apply_mode = "none"
        graph_ops: list[str] = []
    elif safe and confidence >= 0.65:
        action = "auto_correct"
        requires_review = False
        requires_contact_sheet = True
        apply_mode = "automatic"
        graph_ops = ["Set Color Correction"]
    else:
        action = "review_only"
        requires_review = True
        requires_contact_sheet = True
        apply_mode = "manual_review"
        graph_ops = ["Set Color Correction"] if edit_types else []

    if "unsafe_to_auto_correct" in tags:
        action = "review_only"
        requires_review = True
        requires_contact_sheet = True
        apply_mode = "manual_review"

    return {
        "recommended_action": action,
        "reason": tags,
        "confidence": confidence,
        "edit_types": edit_types,
        "requires_review": requires_review,
        "requires_contact_sheet": requires_contact_sheet,
        "apply_mode": apply_mode,
        "graph_ops": graph_ops,
    }


def _recommend(
    brightness: float,
    contrast: float,
    mean_r: float,
    mean_g: float,
    mean_b: float,
    overexposed_fraction: float,
    underexposed_fraction: float,
    luma_p05: float | None = None,
    luma_p50: float | None = None,
    luma_p95: float | None = None,
) -> dict[str, float]:
    mid_luma = brightness if luma_p50 is None else (brightness * 0.35 + luma_p50 * 0.65)
    exposure_ev = _clamp((124.0 - mid_luma) / 104.0, -1.35, 1.35)
    if overexposed_fraction > 0.08:
        exposure_ev = min(exposure_ev, -0.15)
    if underexposed_fraction > 0.12:
        exposure_ev = max(exposure_ev, 0.25)

    contrast_span = None
    if luma_p05 is not None and luma_p95 is not None:
        contrast_span = max(1.0, luma_p95 - luma_p05)
    contrast_basis = contrast if contrast_span is None else (contrast * 0.45 + contrast_span * 0.55)
    contrast_gain = _clamp(78.0 / max(contrast_basis, 1.0), 0.72, 1.32)
    rb_delta = mean_r - mean_b
    gm_delta = mean_g - ((mean_r + mean_b) / 2.0)
    temperature = _clamp(-rb_delta / 90.0, -0.85, 0.85)
    tint = _clamp(-gm_delta / 80.0, -0.85, 0.85)
    shadows = _clamp(underexposed_fraction * 2.1, 0.0, 0.65)
    highlights = _clamp(-overexposed_fraction * 2.2, -0.65, 0.0)
    return {
        "exposure_ev": float(exposure_ev),
        "contrast": float(contrast_gain),
        "saturation": 1.0,
        "temperature": float(temperature),
        "tint": float(tint),
        "shadows": float(shadows),
        "highlights": float(highlights),
    }


def _score_frame(frame_bgr: np.ndarray, t_s: float) -> dict[str, Any]:
    gray = cv2.cvtColor(frame_bgr, cv2.COLOR_BGR2GRAY)
    rgb = cv2.cvtColor(frame_bgr, cv2.COLOR_BGR2RGB)
    mean_r, mean_g, mean_b = (float(v) for v in rgb.reshape(-1, 3).mean(axis=0))
    brightness = float(gray.mean())
    contrast = float(gray.std())
    over = float((gray >= 245).mean())
    under = float((gray <= 10).mean())
    luma_p01 = float(np.percentile(gray, 1))
    luma_p05 = float(np.percentile(gray, 5))
    luma_p10 = float(np.percentile(gray, 10))
    luma_p50 = float(np.percentile(gray, 50))
    luma_p90 = float(np.percentile(gray, 90))
    luma_p95 = float(np.percentile(gray, 95))
    luma_p99 = float(np.percentile(gray, 99))
    low_content = bool(contrast < 3.0 and (brightness < 16.0 or brightness > 239.0))
    black_frame = bool(brightness < 8.0 and contrast < 2.0)
    tags = _issue_tags(
        brightness,
        contrast,
        mean_r,
        mean_g,
        mean_b,
        over,
        under,
        luma_p05,
        luma_p95,
    )
    ignored_for_summary = low_content or black_frame
    ignored_fraction = 1.0 if ignored_for_summary else 0.0
    safe = _auto_correct_safe(tags, over, under, ignored_fraction)
    if not safe and "already_balanced" not in tags:
        tags = [*tags, "unsafe_to_auto_correct"]
    return {
        "t_s": t_s,
        "brightness": brightness,
        "contrast": contrast,
        "luma_p01": luma_p01,
        "luma_p05": luma_p05,
        "luma_p10": luma_p10,
        "luma_p50": luma_p50,
        "luma_p90": luma_p90,
        "luma_p95": luma_p95,
        "luma_p99": luma_p99,
        "mean_r": mean_r,
        "mean_g": mean_g,
        "mean_b": mean_b,
        "estimated_color_cast": _estimate_cast(mean_r, mean_g, mean_b),
        "overexposed_fraction": over,
        "underexposed_fraction": under,
        "clipped_highlight_fraction": over,
        "crushed_shadow_fraction": under,
        "low_content": low_content,
        "black_frame": black_frame,
        "ignored_for_summary": ignored_for_summary,
        "issue_tags": tags,
        "auto_correct_safe": safe,
        "confidence": _confidence(tags, over, under, ignored_fraction),
        "recommended_correction": _recommend(
            brightness,
            contrast,
            mean_r,
            mean_g,
            mean_b,
            over,
            under,
            luma_p05,
            luma_p50,
            luma_p95,
        ),
    }


def _aggregate(per_frame: list[dict[str, Any]]) -> dict[str, Any]:
    if not per_frame:
        return {}
    brightness = np.array([f["brightness"] for f in per_frame], dtype=np.float64)
    contrast = np.array([f["contrast"] for f in per_frame], dtype=np.float64)
    luma_p01 = np.array([f["luma_p01"] for f in per_frame], dtype=np.float64)
    luma_p05 = np.array([f["luma_p05"] for f in per_frame], dtype=np.float64)
    luma_p10 = np.array([f["luma_p10"] for f in per_frame], dtype=np.float64)
    luma_p50 = np.array([f["luma_p50"] for f in per_frame], dtype=np.float64)
    luma_p90 = np.array([f["luma_p90"] for f in per_frame], dtype=np.float64)
    luma_p95 = np.array([f["luma_p95"] for f in per_frame], dtype=np.float64)
    luma_p99 = np.array([f["luma_p99"] for f in per_frame], dtype=np.float64)
    mean_r = np.array([f["mean_r"] for f in per_frame], dtype=np.float64)
    mean_g = np.array([f["mean_g"] for f in per_frame], dtype=np.float64)
    mean_b = np.array([f["mean_b"] for f in per_frame], dtype=np.float64)
    over = np.array([f["overexposed_fraction"] for f in per_frame], dtype=np.float64)
    under = np.array([f["underexposed_fraction"] for f in per_frame], dtype=np.float64)
    ignored = np.array([1.0 if f.get("ignored_for_summary") else 0.0 for f in per_frame], dtype=np.float64)
    cast = _estimate_cast(float(mean_r.mean()), float(mean_g.mean()), float(mean_b.mean()))
    tags = _issue_tags(
        float(brightness.mean()),
        float(contrast.mean()),
        float(mean_r.mean()),
        float(mean_g.mean()),
        float(mean_b.mean()),
        float(over.mean()),
        float(under.mean()),
        float(luma_p05.mean()),
        float(luma_p95.mean()),
    )
    ignored_fraction = float(ignored.mean())
    safe = _auto_correct_safe(tags, float(over.mean()), float(under.mean()), ignored_fraction)
    if not safe and "already_balanced" not in tags:
        tags = [*tags, "unsafe_to_auto_correct"]
    summary = {
        "brightness_mean": float(brightness.mean()),
        "brightness_p01": float(np.percentile(brightness, 1)),
        "brightness_p05": float(np.percentile(brightness, 5)),
        "brightness_p10": float(np.percentile(brightness, 10)),
        "brightness_p50": float(np.percentile(brightness, 50)),
        "brightness_p90": float(np.percentile(brightness, 90)),
        "brightness_p95": float(np.percentile(brightness, 95)),
        "brightness_p99": float(np.percentile(brightness, 99)),
        "contrast_mean": float(contrast.mean()),
        "contrast_p10": float(np.percentile(contrast, 10)),
        "contrast_p90": float(np.percentile(contrast, 90)),
        "luma_p01_mean": float(luma_p01.mean()),
        "luma_p05_mean": float(luma_p05.mean()),
        "luma_p10_mean": float(luma_p10.mean()),
        "luma_p50_mean": float(luma_p50.mean()),
        "luma_p90_mean": float(luma_p90.mean()),
        "luma_p95_mean": float(luma_p95.mean()),
        "luma_p99_mean": float(luma_p99.mean()),
        "mean_r": float(mean_r.mean()),
        "mean_g": float(mean_g.mean()),
        "mean_b": float(mean_b.mean()),
        "estimated_color_cast": cast,
        "overexposed_fraction_mean": float(over.mean()),
        "underexposed_fraction_mean": float(under.mean()),
        "clipped_highlight_fraction_mean": float(over.mean()),
        "crushed_shadow_fraction_mean": float(under.mean()),
        "ignored_fraction": ignored_fraction,
        "issue_tags": tags,
        "auto_correct_safe": safe,
        "confidence": _confidence(tags, float(over.mean()), float(under.mean()), ignored_fraction),
        "recommended_correction": _recommend(
            float(brightness.mean()),
            float(contrast.mean()),
            float(mean_r.mean()),
            float(mean_g.mean()),
            float(mean_b.mean()),
            float(over.mean()),
            float(under.mean()),
            float(luma_p05.mean()),
            float(luma_p50.mean()),
            float(luma_p95.mean()),
        ),
    }
    summary["policy"] = _policy_from_summary(summary)
    return summary


def _summarize(per_frame: list[dict[str, Any]]) -> dict[str, Any]:
    usable = [f for f in per_frame if not f.get("ignored_for_summary")]
    summary = _aggregate(usable or per_frame)
    if summary and per_frame:
        ignored_fraction = 1.0 - (len(usable) / len(per_frame))
        summary["ignored_fraction"] = float(ignored_fraction)
        tags = list(summary.get("issue_tags", []))
        safe = _auto_correct_safe(
            tags,
            float(summary.get("overexposed_fraction_mean", 0.0)),
            float(summary.get("underexposed_fraction_mean", 0.0)),
            ignored_fraction,
        )
        if not safe and "already_balanced" not in tags and "unsafe_to_auto_correct" not in tags:
            tags.append("unsafe_to_auto_correct")
        summary["issue_tags"] = tags
        summary["auto_correct_safe"] = safe
        summary["confidence"] = _confidence(
            tags,
            float(summary.get("overexposed_fraction_mean", 0.0)),
            float(summary.get("underexposed_fraction_mean", 0.0)),
            ignored_fraction,
        )
        summary["policy"] = _policy_from_summary(summary)
    return summary


def _scene_signature(frame: dict[str, Any]) -> tuple[str, ...]:
    return tuple(
        tag
        for tag in frame.get("issue_tags", [])
        if tag
        not in {
            "unsafe_to_auto_correct",
            "clipped_highlights",
            "crushed_shadows",
        }
    )


def _scenes(per_frame: list[dict[str, Any]]) -> list[dict[str, Any]]:
    usable = [f for f in per_frame if not f.get("ignored_for_summary")]
    if not usable:
        usable = per_frame
    if not usable:
        return []

    groups: list[list[dict[str, Any]]] = []
    current: list[dict[str, Any]] = []
    current_sig: tuple[str, ...] | None = None
    current_start = 0.0
    for frame in usable:
        sig = _scene_signature(frame)
        t_s = float(frame["t_s"])
        if (
            current
            and current_sig is not None
            and (sig != current_sig or t_s - current_start >= SCENE_MAX_GAP_S)
        ):
            groups.append(current)
            current = []
            current_start = t_s
        if not current:
            current_sig = sig
            current_start = t_s
        current.append(frame)
    if current:
        groups.append(current)

    scenes = []
    for idx, frames in enumerate(groups):
        summary = _aggregate(frames)
        start_s = float(frames[0]["t_s"])
        end_s = float(frames[-1]["t_s"] + (1.0 / SAMPLE_FPS))
        scenes.append(
            {
                "scene_id": f"color_scene_{idx + 1}",
                "start_s": start_s,
                "end_s": end_s,
                **summary,
            }
        )
    return scenes


def _ignored_frames(per_frame: list[dict[str, Any]]) -> list[dict[str, Any]]:
    ignored = []
    for frame in per_frame:
        reasons = []
        if frame.get("black_frame"):
            reasons.append("black_frame")
        if frame.get("low_content"):
            reasons.append("low_content")
        if reasons:
            ignored.append({"t_s": frame["t_s"], "reasons": reasons})
    return ignored


@server.index_asset
def handle(req: IndexAssetRequest) -> dict[str, Any]:
    setup_start = time.perf_counter()
    frames, w, h = _extract_frames_bgr(req.asset_path, SAMPLE_FPS)
    setup_ms = round((time.perf_counter() - setup_start) * 1000)
    _log.info(
        "color-analysis-mcp: asset=%s streaming frames at %dx%d",
        req.asset_id,
        w,
        h,
    )
    per_frame = []
    decode_read_ms = 0
    analysis_ms = 0
    iterator = iter(frames)
    i = 0
    while True:
        read_start = time.perf_counter()
        try:
            frame = next(iterator)
        except StopIteration:
            decode_read_ms += round((time.perf_counter() - read_start) * 1000)
            break
        decode_read_ms += round((time.perf_counter() - read_start) * 1000)
        analysis_start = time.perf_counter()
        per_frame.append(_score_frame(frame, i / SAMPLE_FPS))
        analysis_ms += round((time.perf_counter() - analysis_start) * 1000)
        i += 1
    aggregate_start = time.perf_counter()
    summary = _summarize(per_frame)
    scenes = _scenes(per_frame)
    ignored_frames = _ignored_frames(per_frame)
    aggregate_ms = round((time.perf_counter() - aggregate_start) * 1000)
    return {
        "frame_rate_sampled": SAMPLE_FPS,
        "detect_width": w,
        "detect_height": h,
        "frame_count": len(per_frame),
        "summary": summary,
        "scenes": scenes,
        "ignored_frames": ignored_frames,
        "per_frame": per_frame,
        "perf": {
            "setup_ms": setup_ms,
            "decode_read_ms": decode_read_ms,
            "analysis_ms": analysis_ms,
            "aggregate_ms": aggregate_ms,
            "frames_processed": len(per_frame),
        },
    }


def main() -> None:
    server.run()
