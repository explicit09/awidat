#!/usr/bin/env python3
"""Plan clip-level camera matching from color-analysis sidecars."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
from typing import Any


def clamp(value: float, lo: float, hi: float) -> float:
    return max(lo, min(hi, value))


def load_body(path: str) -> dict[str, Any]:
    raw = json.loads(Path(path).read_text())
    return raw.get("data", raw)


def summary(path: str, label: str | None) -> dict[str, Any]:
    body = load_body(path)
    s = body.get("summary", {})
    tags = s.get("issue_tags", [])
    policy = s.get("policy", {})
    confidence = float(s.get("confidence", 0.0))
    safe = bool(s.get("auto_correct_safe", False)) or policy.get("recommended_action") == "no_op"
    score = confidence
    if "already_balanced" in tags:
        score += 0.4
    score -= 0.12 * len([t for t in tags if t != "already_balanced"])
    if "unsafe_to_auto_correct" in tags or not safe:
        score -= 1.0
    return {
        "path": path,
        "label": label or Path(path).name,
        "summary": s,
        "issue_tags": tags,
        "confidence": confidence,
        "auto_correct_safe": safe,
        "reference_score": score,
    }


def rb_delta(s: dict[str, Any]) -> float:
    return float(s.get("mean_r", 0.0)) - float(s.get("mean_b", 0.0))


def gm_delta(s: dict[str, Any]) -> float:
    return float(s.get("mean_g", 0.0)) - ((float(s.get("mean_r", 0.0)) + float(s.get("mean_b", 0.0))) / 2.0)


def match_correction(src: dict[str, Any], ref: dict[str, Any]) -> dict[str, float]:
    src_s = src["summary"]
    ref_s = ref["summary"]
    src_mid = float(src_s.get("luma_p50_mean", src_s.get("brightness_mean", 128.0)))
    ref_mid = float(ref_s.get("luma_p50_mean", ref_s.get("brightness_mean", 128.0)))
    src_span = max(1.0, float(src_s.get("luma_p95_mean", 180.0)) - float(src_s.get("luma_p05_mean", 40.0)))
    ref_span = max(1.0, float(ref_s.get("luma_p95_mean", 180.0)) - float(ref_s.get("luma_p05_mean", 40.0)))
    return {
        "exposure_ev": clamp((ref_mid - src_mid) / 104.0, -1.0, 1.0),
        "contrast": clamp(ref_span / src_span, 0.75, 1.3),
        "saturation": 1.0,
        "temperature": clamp(-(rb_delta(src_s) - rb_delta(ref_s)) / 90.0, -0.85, 0.85),
        "tint": clamp(-(gm_delta(src_s) - gm_delta(ref_s)) / 80.0, -0.85, 0.85),
        "shadows": clamp(
            (float(src_s.get("underexposed_fraction_mean", 0.0)) - float(ref_s.get("underexposed_fraction_mean", 0.0))) * 1.5,
            0.0,
            0.5,
        ),
        "highlights": clamp(
            -(float(src_s.get("overexposed_fraction_mean", 0.0)) - float(ref_s.get("overexposed_fraction_mean", 0.0))) * 1.5,
            -0.5,
            0.0,
        ),
    }


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--color-index", action="append", required=True)
    parser.add_argument("--camera-label", action="append", default=[])
    args = parser.parse_args()

    labels = args.camera_label + [None] * len(args.color_index)
    cameras = [summary(path, labels[i]) for i, path in enumerate(args.color_index)]
    reference = max(cameras, key=lambda c: c["reference_score"])
    matches = []
    for camera in cameras:
        if camera is reference:
            matches.append(
                {
                    "camera": camera["label"],
                    "role": "reference",
                    "requires_review": False,
                    "recommended_correction": {},
                    "graph_ops": [],
                    "reason": camera["issue_tags"],
                }
            )
            continue
        unsafe = "unsafe_to_auto_correct" in camera["issue_tags"] or camera["confidence"] < 0.55
        matches.append(
            {
                "camera": camera["label"],
                "role": "match_to_reference",
                "reference_camera": reference["label"],
                "requires_review": unsafe,
                "recommended_correction": match_correction(camera, reference),
                "graph_ops": ["Set Color Correction"],
                "reason": camera["issue_tags"],
            }
        )

    print(
        json.dumps(
            {
                "status": "planned",
                "reference_camera": reference["label"],
                "reference_confidence": reference["confidence"],
                "matches": matches,
            },
            indent=2,
        )
    )


if __name__ == "__main__":
    main()
