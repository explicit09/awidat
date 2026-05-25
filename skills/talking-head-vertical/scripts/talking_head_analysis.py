"""Talking-head vertical evidence heuristics."""

from __future__ import annotations

import math
import re
import statistics


FILLER_RE = re.compile(r"^(um+|uh+|erm+|like|you know|i mean|sort of|kind of)$", re.I)
FALSE_START_RE = re.compile(r"\b(\w+)\s+\1\b", re.I)


def summarize_face(face: dict) -> dict:
    frames = face.get("per_frame", [])
    centers = []
    boxes = []
    for frame in frames:
        faces = frame.get("faces") or []
        if not faces:
            continue
        best = max(faces, key=lambda item: float(item.get("confidence", 0.0)))
        box = normalized_box(best.get("bbox") or best)
        if not box:
            continue
        x, y, w, h = box
        boxes.append({"x": round(x, 3), "y": round(y, 3), "w": round(w, 3), "h": round(h, 3)})
        centers.append((x + w / 2.0, y + h / 2.0))
    if not centers:
        return {
            "coverage": 0.0,
            "center": None,
            "headroom": None,
            "eye_line_y": None,
            "stability": 0.0,
            "mouth_band": None,
            "face_box": None,
        }
    median_box = {
        "x": round(statistics.median(box["x"] for box in boxes), 3),
        "y": round(statistics.median(box["y"] for box in boxes), 3),
        "w": round(statistics.median(box["w"] for box in boxes), 3),
        "h": round(statistics.median(box["h"] for box in boxes), 3),
    }
    jitter = 0.0
    if len(centers) > 1:
        jitter = statistics.pstdev(point[0] for point in centers) + statistics.pstdev(
            point[1] for point in centers
        )
    center_x = statistics.mean(point[0] for point in centers)
    center_y = statistics.mean(point[1] for point in centers)
    return {
        "coverage": round(len(centers) / max(1, len(frames)), 3),
        "center": {"x": round(center_x, 3), "y": round(center_y, 3)},
        "headroom": median_box["y"],
        "eye_line_y": round(median_box["y"] + median_box["h"] * 0.28, 3),
        "mouth_band": {
            "top": round(median_box["y"] + median_box["h"] * 0.56, 3),
            "bottom": round(median_box["y"] + median_box["h"] * 0.86, 3),
        },
        "face_box": median_box,
        "stability": round(max(0.0, 1.0 - jitter * 8.0), 3),
    }


def normalized_box(raw: dict) -> tuple[float, float, float, float] | None:
    if all(key in raw for key in ["x", "y", "w", "h"]):
        x, y, w, h = (float(raw[key]) for key in ["x", "y", "w", "h"])
    elif all(key in raw for key in ["left", "top", "right", "bottom"]):
        left, top, right, bottom = (float(raw[key]) for key in ["left", "top", "right", "bottom"])
        x, y, w, h = left, top, right - left, bottom - top
    else:
        return None
    if max(x, y, w, h) > 1.5 or w <= 0 or h <= 0:
        return None
    return (clamp(x), clamp(y), clamp(w), clamp(h))


def summarize_gaze(gaze: dict) -> dict:
    frames = gaze.get("per_frame", [])
    if not frames:
        return {"direct_address_ratio": 0.0}
    direct = sum(1 for frame in frames if any(face.get("at_camera") for face in frame.get("faces") or []))
    return {"direct_address_ratio": round(direct / len(frames), 3)}


def summarize_shots(shot: dict) -> dict:
    shots = shot.get("shots", [])
    if not shots:
        return {"talking_head_ratio": 0.0, "unstable_motion_ratio": 0.0}
    talking = sum(1 for item in shots if item.get("type") in {"close-up", "medium", "medium-close"})
    unstable = sum(1 for item in shots if item.get("motion") in {"handheld", "fast-cut", "whip"})
    return {
        "talking_head_ratio": round(talking / len(shots), 3),
        "unstable_motion_ratio": round(unstable / len(shots), 3),
    }


def summarize_quality(quality: dict) -> dict:
    frames = quality.get("per_frame", [])
    if not frames:
        return {"sharp_ratio": 0.0}
    sharp = sum(1 for frame in frames if frame.get("is_sharp", False))
    return {"sharp_ratio": round(sharp / len(frames), 3)}


def summarize_composition(composition: dict) -> dict:
    best_space = {"side": None, "score": 0.0}
    framing = "unknown"
    for region in composition.get("regions", []):
        framing = str(region.get("framing", framing))
        space = region.get("negative_space") or {}
        score = float(space.get("score", 0.0))
        if score > best_space["score"]:
            best_space = {"side": space.get("side"), "score": round(score, 3)}
    return {"negative_space": best_space, "framing": framing}


def classify_candidate(
    *,
    face_stats: dict,
    gaze_stats: dict,
    shot_stats: dict,
    quality_stats: dict,
    transcript: dict,
) -> dict:
    issues = []
    if face_stats["coverage"] < 0.55:
        issues.append("no sustained face evidence")
    if transcript_word_count(transcript) < 8:
        issues.append("insufficient transcript evidence")
    score = (
        face_stats["coverage"] * 0.34
        + gaze_stats["direct_address_ratio"] * 0.22
        + shot_stats["talking_head_ratio"] * 0.18
        + quality_stats["sharp_ratio"] * 0.14
        + face_stats["stability"] * 0.12
    )
    return {
        "is_candidate": not issues and score >= 0.58,
        "score": round(score, 3),
        "issues": issues,
        "signals": {
            "face_coverage": face_stats["coverage"],
            "direct_address_ratio": gaze_stats["direct_address_ratio"],
            "talking_head_shot_ratio": shot_stats["talking_head_ratio"],
            "sharp_ratio": quality_stats["sharp_ratio"],
            "visual_stability": face_stats["stability"],
        },
    }


def choose_layout(
    *,
    source_width: int,
    source_height: int,
    face_stats: dict,
    composition_stats: dict,
    candidate: dict,
) -> dict:
    aspect = source_width / max(source_height, 1)
    face_center = face_stats.get("center") or {"x": 0.5, "y": 0.42}
    native_vertical = aspect < 0.8
    good_framing = (
        candidate["is_candidate"]
        and face_stats.get("headroom") is not None
        and 0.04 <= face_stats["headroom"] <= 0.2
        and face_stats["stability"] >= 0.6
    )
    strategy = "keep_native_vertical" if native_vertical and good_framing else "reframe_horizontal_to_9_16"
    if native_vertical and not good_framing:
        strategy = "repair_vertical_framing"
    reserved = composition_stats.get("negative_space") or {"side": None, "score": 0.0}
    caption_position = "bottom"
    face_box = face_stats.get("face_box")
    if face_box and band_overlaps_face("bottom", face_box) and reserved.get("side") in {"top", "upper"}:
        caption_position = "top"
    elif face_box and band_overlaps_face("bottom", face_box):
        caption_position = "top" if face_box["y"] > 0.46 else "bottom"
    return {
        "strategy": strategy,
        "aspect_ratio": "9:16",
        "source_aspect_ratio": round(aspect, 3),
        "subject_center": face_center,
        "caption_position": caption_position,
        "reserved_space": reserved,
        "safe_area": "mobile",
        "reason": layout_reason(strategy, reserved),
    }


def plan_pacing(transcript: dict, energy: dict, topic: dict) -> dict:
    cuts = []
    for silence in energy.get("silences", []):
        start = float(silence.get("start_s", 0.0))
        end = float(silence.get("end_s", start))
        duration = end - start
        if duration < 0.8:
            continue
        emotional_pause = near_topic_boundary(topic, start, end) and duration <= 1.4
        if emotional_pause:
            continue
        cuts.append({
            "start_s": round(start + 0.125, 3),
            "end_s": round(end - 0.125, 3),
            "reason": "dead_air",
        })
    return {
        "cuts": cuts,
        "filler_clusters": filler_clusters(transcript),
        "policy": {
            "dead_air_threshold_s": 0.8,
            "preserve_topic_boundary_pauses_under_s": 1.4,
            "avoid_robotic_overcutting_min_keep_s": 0.25,
        },
    }


def filler_clusters(transcript: dict) -> list[dict]:
    clusters = []
    seen_text: dict[str, dict] = {}
    for segment in transcript.get("segments", []):
        text = str(segment.get("text", ""))
        start = float(segment.get("start_s", segment.get("start", 0.0)))
        end = float(segment.get("end_s", segment.get("end", 0.0)))
        tokens = re.findall(r"[A-Za-z']+(?:\s+[A-Za-z']+)?", text)
        filler_count = sum(1 for token in tokens if FILLER_RE.match(token.strip()))
        if filler_count >= 2 or FALSE_START_RE.search(text):
            clusters.append({
                "start_s": start,
                "end_s": end,
                "reason": "filler_or_false_start_review",
            })
        normalized = normalize_repetition_text(text)
        if normalized and normalized in seen_text:
            clusters.append({
                "start_s": start,
                "end_s": end,
                "reason": "low_value_repetition_review",
                "matches_start_s": seen_text[normalized]["start_s"],
            })
        elif normalized:
            seen_text[normalized] = {"start_s": start}
    return clusters


def normalize_repetition_text(text: str) -> str:
    tokens = re.findall(r"[a-z0-9']+", text.lower())
    if len(tokens) < 5:
        return ""
    return " ".join(tokens[:10])


def band_overlaps_face(position: str, face_box: dict) -> bool:
    band = {"top": (0.08, 0.24), "bottom": (0.68, 0.88)}[position]
    face_top = face_box["y"]
    face_bottom = face_box["y"] + face_box["h"]
    return min(band[1], face_bottom) > max(band[0], face_top)


def layout_reason(strategy: str, reserved: dict) -> str:
    if strategy == "keep_native_vertical":
        return "Native vertical framing is stable with acceptable headroom."
    if reserved.get("score", 0.0) >= 0.6:
        return f"Subject-aware 9:16 crop with safe {reserved.get('side')} negative space available."
    return "Subject-aware 9:16 crop around the speaker."


def transcript_word_count(transcript: dict) -> int:
    if transcript.get("words"):
        return len(transcript["words"])
    text = " ".join(str(s.get("text", "")) for s in transcript.get("segments", []))
    return len(re.findall(r"[A-Za-z0-9']+", text))


def near_topic_boundary(topic: dict, start: float, end: float) -> bool:
    for item in topic.get("topics", []):
        topic_start = float(item.get("start_s", 0.0))
        topic_end = float(item.get("end_s", topic_start))
        if abs(start - topic_start) < 1.0 or abs(end - topic_end) < 1.0:
            return True
    return False


def clamp(value: float) -> float:
    if not math.isfinite(value):
        return 0.0
    return max(0.0, min(1.0, value))
