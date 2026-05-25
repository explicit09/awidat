#!/usr/bin/env python3
"""Adaptive caption and overlay layout planning from visual evidence."""

from __future__ import annotations

from statistics import mean
from typing import Any

from adaptive_layout_evidence import (
    composition_for_time,
    face_regions,
    frame_quality_risk,
    important_regions,
    scene_id as evidence_scene_id,
)


ZONE_ORDER = ["bottom", "top", "center_lower", "left", "right", "negative_space"]
EDL_POSITION_BY_ZONE = {
    "bottom": "bottom",
    "top": "top",
    "center_lower": "center",
    "left": "center",
    "right": "center",
    "negative_space": "center",
}
ZONE_LABELS = {
    "bottom": "bottom-third",
    "top": "top-third",
    "center_lower": "center-lower",
    "left": "left-side",
    "right": "right-side",
    "negative_space": "background-negative-space",
}


def _safe_rect(
    *,
    frame_width: int,
    frame_height: int,
    safe_margin_x: int,
    safe_margin_y: int,
) -> dict[str, float]:
    return {
        "x": float(safe_margin_x),
        "y": float(safe_margin_y),
        "width": float(frame_width - safe_margin_x * 2),
        "height": float(frame_height - safe_margin_y * 2),
    }


def _caption_size(phrase: dict[str, Any], *, frame_width: int) -> tuple[float, float]:
    text = str(phrase.get("text", "")).strip()
    lines = text.splitlines() or [text]
    font_size = float(phrase.get("font_size", 52))
    longest_line = max((len(line) for line in lines), default=0)
    width = min(
        frame_width * 0.82,
        max(font_size * 3.5, longest_line * font_size * 0.58 + font_size * 0.72),
    )
    height = len(lines) * font_size * 1.2 + font_size * 0.64
    return width, height


def _zone_box(
    zone: str,
    phrase: dict[str, Any],
    *,
    frame_width: int,
    frame_height: int,
    safe_margin_x: int,
    safe_margin_y: int,
    negative_space: dict[str, float] | None,
) -> dict[str, float]:
    width, height = _caption_size(phrase, frame_width=frame_width)
    safe = _safe_rect(
        frame_width=frame_width,
        frame_height=frame_height,
        safe_margin_x=safe_margin_x,
        safe_margin_y=safe_margin_y,
    )

    if zone == "top":
        x = (frame_width - width) / 2.0
        y = safe["y"]
    elif zone == "center_lower":
        x = (frame_width - width) / 2.0
        y = frame_height * 0.62 - height / 2.0
    elif zone == "left":
        x = safe["x"]
        y = (frame_height - height) / 2.0
    elif zone == "right":
        x = frame_width - safe_margin_x - width
        y = (frame_height - height) / 2.0
    elif zone == "negative_space" and negative_space:
        x = negative_space["x"] + max(0.0, negative_space["width"] - width) / 2.0
        y = negative_space["y"] + max(0.0, negative_space["height"] - height) / 2.0
    else:
        x = (frame_width - width) / 2.0
        y = frame_height - safe_margin_y - height
    x = min(max(x, safe["x"]), safe["x"] + safe["width"] - width)
    y = min(max(y, safe["y"]), safe["y"] + safe["height"] - height)
    return {
        "x": round(x, 3),
        "y": round(y, 3),
        "width": round(width, 3),
        "height": round(height, 3),
    }


def _intersection_ratio(a: dict[str, float], b: dict[str, float]) -> float:
    left = max(a["x"], b["x"])
    top = max(a["y"], b["y"])
    right = min(a["x"] + a["width"], b["x"] + b["width"])
    bottom = min(a["y"] + a["height"], b["y"] + b["height"])
    if right <= left or bottom <= top:
        return 0.0
    overlap = (right - left) * (bottom - top)
    area = max(1.0, a["width"] * a["height"])
    return overlap / area


def _negative_space_from_regions(
    regions: list[dict[str, Any]],
    *,
    frame_width: int,
    frame_height: int,
    safe_margin_x: int,
    safe_margin_y: int,
) -> dict[str, float] | None:
    safe = _safe_rect(
        frame_width=frame_width,
        frame_height=frame_height,
        safe_margin_x=safe_margin_x,
        safe_margin_y=safe_margin_y,
    )
    if not regions:
        return safe
    centers = [
        region["box"]["x"] + region["box"]["width"] / 2.0
        for region in regions
        if region["kind"] == "face"
    ]
    if not centers:
        return safe
    avg_x = mean(centers)
    if avg_x < frame_width / 2.0:
        return {
            "x": frame_width * 0.54,
            "y": safe["y"],
            "width": frame_width - frame_width * 0.54 - safe_margin_x,
            "height": safe["height"],
        }
    return {
        "x": safe["x"],
        "y": safe["y"],
        "width": frame_width * 0.46 - safe_margin_x,
        "height": safe["height"],
    }


def _safe_area_score(
    box: dict[str, float],
    *,
    frame_width: int,
    frame_height: int,
    safe_margin_x: int,
    safe_margin_y: int,
) -> float:
    inside = (
        box["x"] >= safe_margin_x
        and box["y"] >= safe_margin_y
        and box["x"] + box["width"] <= frame_width - safe_margin_x
        and box["y"] + box["height"] <= frame_height - safe_margin_y
    )
    return 1.0 if inside else 0.0


def _has_caption_backing(phrase: dict[str, Any]) -> bool:
    background = str(phrase.get("background", "")).strip().lower()
    stroke_width = float(phrase.get("stroke_width", 0))
    return stroke_width >= 2.0 or background not in {"", "transparent", "none"}


def _edl_hint(phrase: dict[str, Any], selected_zone: str, box: dict[str, float]) -> dict[str, Any]:
    kind = str(phrase.get("overlay_kind", "caption"))
    base = {
        "op": "insert_caption" if kind == "caption" else "insert_title",
        "start_s": round(float(phrase.get("start_s", 0.0)), 3),
        "end_s": round(float(phrase.get("end_s", 0.0)), 3),
        "text": str(phrase.get("text", "")).strip(),
        "position": EDL_POSITION_BY_ZONE[selected_zone],
        "font_size": int(phrase.get("font_size", 52)),
        "color": str(phrase.get("color", "#FFFFFF")),
        "safe_area": str(phrase.get("safe_area", "mobile")),
        "layout_zone": selected_zone,
        "bounding_box": box,
    }
    if kind == "caption":
        base["word_timings_json"] = phrase.get("word_timings", [])
    else:
        base["font_weight"] = str(phrase.get("font_weight", "bold"))
        base["animation"] = str(phrase.get("animation", "fade_in_out"))
    return base


def _motion_risk(shot: dict[str, Any]) -> float:
    motion = str(shot.get("motion", "")).strip()
    if motion in {"fast-cut", "handheld"}:
        return 0.75
    if motion == "slow-pan":
        return 0.35
    return min(1.0, max(0.0, float(shot.get("action_score", 0.0))))


def _score_zone(
    zone: str,
    box: dict[str, float],
    phrase: dict[str, Any],
    visual_regions: list[dict[str, Any]],
    composition_region: dict[str, Any],
    shot_region: dict[str, Any],
    previous_zone: str | None,
    quality_risk: float,
    *,
    frame_width: int,
    frame_height: int,
    safe_margin_x: int,
    safe_margin_y: int,
) -> tuple[dict[str, float], list[str]]:
    safe_area = _safe_area_score(
        box,
        frame_width=frame_width,
        frame_height=frame_height,
        safe_margin_x=safe_margin_x,
        safe_margin_y=safe_margin_y,
    )
    reasons: list[str] = []
    face_penalty = 0.0
    body_penalty = 0.0
    gaze_penalty = 0.0
    for region in visual_regions:
        ratio = _intersection_ratio(box, region["box"])
        if ratio <= 0.0:
            continue
        kind = region["kind"]
        if kind in {"face", "eyes", "mouth"}:
            face_penalty = max(face_penalty, ratio * float(region["weight"]))
            reason = f"overlaps_{'face' if kind == 'face' else kind}"
            if reason not in reasons:
                reasons.append(reason)
        elif kind == "gaze_line":
            gaze_penalty = max(gaze_penalty, ratio * float(region["weight"]))
            if "crosses_gaze_line" not in reasons:
                reasons.append("crosses_gaze_line")
        elif kind == "subject_body":
            body_penalty = max(body_penalty, ratio * float(region["weight"]))
            if "overlaps_subject_body" not in reasons:
                reasons.append("overlaps_subject_body")
        else:
            body_penalty = max(body_penalty, ratio * float(region["weight"]))
            reason = f"overlaps_{kind}"
            if reason not in reasons:
                reasons.append(reason)

    motion = _motion_risk(shot_region)
    source = str(composition_region.get("composition_source", ""))
    composition_confidence = float(composition_region.get("composition_confidence", 0.55))
    busy = min(1.0, motion + quality_risk + (0.2 if "model:" in source else 0.0))
    readability = 1.0 if _has_caption_backing(phrase) else max(0.35, 1.0 - busy)
    contrast = 1.0 if _has_caption_backing(phrase) else 0.45
    consistency = 1.0 if previous_zone in {None, zone} else 0.62
    if safe_area < 1.0 and "outside_safe_area" not in reasons:
        reasons.append("outside_safe_area")
    if busy >= 0.65 and not _has_caption_backing(phrase):
        reasons.append("busy_background_needs_box")

    avoidance = max(0.0, 1.0 - min(1.0, face_penalty * 2.4 + body_penalty + gaze_penalty))
    score = (
        readability * 0.20
        + safe_area * 0.18
        + avoidance * 0.28
        + contrast * 0.14
        + max(0.0, 1.0 - busy) * 0.08
        + consistency * 0.12
    )
    if face_penalty > 0.0:
        score -= 0.35
    if zone == "bottom":
        score += 0.04
    if zone == "negative_space" and not visual_regions:
        score -= 0.1
    scores = {
        "readability": round(readability, 3),
        "safe_area": round(safe_area, 3),
        "face_gaze_avoidance": round(avoidance, 3),
        "background_contrast": round(contrast, 3),
        "motion_busy_region_risk": round(busy, 3),
        "consistency": round(consistency, 3),
        "confidence": round(max(0.0, min(1.0, score * composition_confidence + 0.2)), 3),
        "total": round(max(0.0, min(1.0, score)), 3),
    }
    return scores, reasons


def plan_adaptive_layout(
    phrases: list[dict[str, Any]],
    *,
    face: dict[str, Any] | None = None,
    gaze: dict[str, Any] | None = None,
    shot: dict[str, Any] | None = None,
    composition: dict[str, Any] | None = None,
    frame_quality: dict[str, Any] | None = None,
    frame_width: int = 1080,
    frame_height: int = 1920,
    safe_margin_x: int = 72,
    safe_margin_y: int = 144,
) -> dict[str, Any]:
    recommendations: list[dict[str, Any]] = []
    previous_zone: str | None = None
    previous_scene_id: str | None = None
    for index, phrase in enumerate(phrases, start=1):
        start_s = float(phrase.get("start_s", 0.0))
        end_s = float(phrase.get("end_s", start_s))
        visual_regions = face_regions(
            face or gaze,
            start_s=start_s,
            end_s=end_s,
            frame_width=frame_width,
            frame_height=frame_height,
        )
        composition_region = composition_for_time(composition, start_s, end_s)
        shot_region = composition_for_time(shot, start_s, end_s)
        scene_id = evidence_scene_id(composition_region or shot_region)
        scoring_previous_zone = previous_zone if scene_id == previous_scene_id else None
        quality_risk = frame_quality_risk(frame_quality, start_s, end_s)
        visual_regions.extend(
            important_regions(
                composition_region,
                frame_width=frame_width,
                frame_height=frame_height,
            )
        )
        negative_space = _negative_space_from_regions(
            visual_regions,
            frame_width=frame_width,
            frame_height=frame_height,
            safe_margin_x=safe_margin_x,
            safe_margin_y=safe_margin_y,
        )

        boxes = {
            zone: _zone_box(
                zone,
                phrase,
                frame_width=frame_width,
                frame_height=frame_height,
                safe_margin_x=safe_margin_x,
                safe_margin_y=safe_margin_y,
                negative_space=negative_space,
            )
            for zone in ZONE_ORDER
        }
        scores: dict[str, dict[str, float]] = {}
        rejected_zones: list[dict[str, Any]] = []
        for zone in ZONE_ORDER:
            zone_scores, reasons = _score_zone(
                zone,
                boxes[zone],
                phrase,
                visual_regions,
                composition_region,
                shot_region,
                scoring_previous_zone,
                quality_risk,
                frame_width=frame_width,
                frame_height=frame_height,
                safe_margin_x=safe_margin_x,
                safe_margin_y=safe_margin_y,
            )
            scores[zone] = zone_scores
            hard_reasons = [
                reason for reason in reasons
                if reason in {
                    "overlaps_face",
                    "overlaps_eyes",
                    "overlaps_mouth",
                    "overlaps_hands",
                    "overlaps_ui",
                    "overlaps_product",
                    "overlaps_key_action",
                    "outside_safe_area",
                }
            ]
            if hard_reasons:
                rejected_zones.append({"zone": zone, "reasons": hard_reasons})

        selected_zone = max(
            ZONE_ORDER,
            key=lambda zone: (scores[zone]["total"], 1 if zone == previous_zone else 0),
        )
        if any(item["zone"] == selected_zone for item in rejected_zones):
            accepted = [
                zone for zone in ZONE_ORDER
                if not any(item["zone"] == zone for item in rejected_zones)
            ]
            if accepted:
                selected_zone = max(accepted, key=lambda zone: scores[zone]["total"])
        previous_zone = selected_zone
        previous_scene_id = scene_id
        selected_scores = scores[selected_zone]
        recommendation = {
            "phrase_id": str(phrase.get("id", f"phrase-{index}")),
            "scene_id": scene_id,
            "content_type": str(phrase.get("overlay_kind", "caption")),
            "text": str(phrase.get("text", "")).strip(),
            "start_s": round(start_s, 3),
            "end_s": round(end_s, 3),
            "selected_zone": selected_zone,
            "zone_label": ZONE_LABELS[selected_zone],
            "bounding_box": boxes[selected_zone],
            "style_recommendation": {
                "style": str(phrase.get("style", "classic")),
                "boxed": bool(_has_caption_backing(phrase)),
                "position": EDL_POSITION_BY_ZONE[selected_zone],
                "safe_area": str(phrase.get("safe_area", "mobile")),
            },
            "scores": scores,
            "rejected_zones": rejected_zones,
            "confidence": selected_scores["confidence"],
            "edl_hint": _edl_hint(phrase, selected_zone, boxes[selected_zone]),
        }
        recommendations.append(recommendation)

    return {
        "version": 1,
        "status": "ready",
        "frame": {
            "width": frame_width,
            "height": frame_height,
            "safe_margin_x": safe_margin_x,
            "safe_margin_y": safe_margin_y,
        },
        "zone_order": ZONE_ORDER,
        "recommendations": recommendations,
    }
