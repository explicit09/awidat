#!/usr/bin/env python3
"""Plan a vertical talking-head draft from Montage evidence sidecars."""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path


SCRIPT_DIR = Path(__file__).resolve().parent
SHORT_FORM_SCRIPTS = SCRIPT_DIR.parent.parent / "short-form" / "scripts"
VIRAL_SCRIPTS = SCRIPT_DIR.parent.parent / "viral-clip-extractor" / "scripts"
for script_dir in [SCRIPT_DIR, SHORT_FORM_SCRIPTS, VIRAL_SCRIPTS]:
    if str(script_dir) not in sys.path:
        sys.path.insert(0, str(script_dir))

from caption_plan import build_caption_phrases, build_geometry_scorecard, build_readability_scorecard, words  # noqa: E402
from score_moments import score_candidates  # noqa: E402
from talking_head_analysis import (  # noqa: E402
    band_overlaps_face,
    choose_layout,
    classify_candidate,
    plan_pacing,
    summarize_composition,
    summarize_face,
    summarize_gaze,
    summarize_quality,
    summarize_shots,
)


def load_body(path: str | None) -> dict:
    if not path:
        return {}
    raw = json.loads(Path(path).read_text())
    return raw.get("data", raw)


def build_talking_head_plan(
    inputs: dict,
    *,
    asset_id: str,
    clip_id: str,
    source_width: int,
    source_height: int,
    target_duration_s: float = 60.0,
) -> dict:
    transcript = inputs.get("transcript", {})
    energy = inputs.get("audio_energy", {})
    face_stats = summarize_face(inputs.get("face", {}))
    gaze_stats = summarize_gaze(inputs.get("gaze", {}))
    shot_stats = summarize_shots(inputs.get("shot", {}))
    quality_stats = summarize_quality(inputs.get("frame_quality", {}))
    composition_stats = summarize_composition(inputs.get("composition", {}))
    candidate = classify_candidate(
        face_stats=face_stats,
        gaze_stats=gaze_stats,
        shot_stats=shot_stats,
        quality_stats=quality_stats,
        transcript=transcript,
    )
    hook = choose_hook(
        moments=inputs.get("moments", {}),
        energy=energy,
        transcript=transcript,
        shot=inputs.get("shot", {}),
        gaze=inputs.get("gaze", {}),
        quality=inputs.get("frame_quality", {}),
        topic=inputs.get("topic", {}),
    )
    layout = choose_layout(
        source_width=source_width,
        source_height=source_height,
        face_stats=face_stats,
        composition_stats=composition_stats,
        candidate=candidate,
    )
    captions = plan_captions(transcript, layout, face_stats, hook)
    pacing = plan_pacing(transcript, energy, inputs.get("topic", {}))
    motion = plan_emphasis_motion(clip_id, hook, candidate)
    edl = build_edl(
        clip_id=clip_id,
        layout=layout,
        caption_plan=captions,
        pacing_plan=pacing,
        motion_plan=motion,
        source_width=source_width,
        source_height=source_height,
    )
    return {
        "asset_id": asset_id,
        "clip_id": clip_id,
        "target": {"aspect_ratio": "9:16", "duration_s": target_duration_s},
        "candidate": candidate,
        "visual_analysis": {
            "face": face_stats,
            "gaze": gaze_stats,
            "shot": shot_stats,
            "composition": composition_stats,
            "frame_quality": quality_stats,
        },
        "layout": layout,
        "hook": hook,
        "pacing_plan": pacing,
        "caption_plan": captions,
        "motion_plan": motion,
        "readiness": verify_readiness(
            candidate=candidate,
            hook=hook,
            layout=layout,
            caption_plan=captions,
            edl=edl,
        ),
        "edl": edl,
    }


def choose_hook(
    *,
    moments: dict,
    energy: dict,
    transcript: dict,
    shot: dict,
    gaze: dict,
    quality: dict,
    topic: dict,
) -> dict:
    candidates = []
    if moments.get("moments") or moments.get("results"):
        candidates = score_candidates(
            moments_body=moments,
            energy_body=energy,
            transcript_body=transcript,
            shot_body=shot,
            gaze_body=gaze,
            quality_body=quality,
            topic_body=topic,
            limit=5,
            max_overlap_ratio=0.5,
        )
    if candidates:
        best = candidates[0]
        return {
            "source": "editorial_moments",
            "start_s": best["start_s"],
            "end_s": best["end_s"],
            "score": best["score"],
            "reason": best["score_breakdown"],
        }
    segment = (transcript.get("segments") or [{}])[0]
    start = float(segment.get("start_s", segment.get("start", 0.0)))
    end = float(segment.get("end_s", segment.get("end", start + 3.0)))
    return {
        "source": "transcript_first_segment",
        "start_s": round(start, 3),
        "end_s": round(min(end, start + 3.0), 3),
        "score": 0.0,
        "reason": {"fallback": True},
    }


def plan_captions(transcript: dict, layout: dict, face_stats: dict, hook: dict) -> dict:
    phrases = build_caption_phrases(
        words(transcript),
        max_words=4,
        max_gap_s=0.45,
        phrase_preset="medium",
        hot_start_s=hook.get("start_s"),
        hot_end_s=hook.get("end_s"),
        max_chars_per_line=24,
        style="classic",
    )
    for phrase in phrases:
        phrase["position"] = layout["caption_position"]
    face_overlap = "none"
    if face_stats.get("face_box") and any(
        band_overlaps_face(phrase.get("position", "bottom"), face_stats["face_box"])
        for phrase in phrases
    ):
        face_overlap = "risk"
    elif face_stats.get("face_box"):
        face_overlap = "avoided"
    return {
        "phrases": phrases,
        "readability": build_readability_scorecard(phrases, max_chars_per_line=24),
        "geometry": build_geometry_scorecard(phrases),
        "face_overlap_risk": face_overlap,
        "emphasis_keywords": emphasis_keywords(transcript),
    }


def plan_emphasis_motion(clip_id: str, hook: dict, candidate: dict) -> dict:
    if not candidate["is_candidate"]:
        return {"animations": [], "policy": "disabled_until_candidate_passes"}
    start = max(0.0, float(hook.get("start_s", 0.0)))
    return {
        "animations": [{
            "id": f"talking_head_{safe_id(clip_id)}_hook_punch",
            "target": {"kind": "clip_parameter", "clip_id": clip_id, "parameter": "clip.transform.scale"},
            "keyframes": [
                {"time_s": round(start, 3), "value": 1.0},
                {"time_s": round(start + 0.22, 3), "value": 1.065},
                {"time_s": round(start + 0.72, 3), "value": 1.0},
            ],
            "rationale": "Brief punch-in on the hook; reset quickly to avoid excessive motion.",
        }],
        "policy": "single hook punch-in plus subtle reset; no repeated motion by default",
    }


def build_edl(
    *,
    clip_id: str,
    layout: dict,
    caption_plan: dict,
    pacing_plan: dict,
    motion_plan: dict,
    source_width: int,
    source_height: int,
) -> str:
    lines = output_format_edl()
    if layout["strategy"] in {"reframe_horizontal_to_9_16", "repair_vertical_framing"}:
        lines.extend(reframe_edl(clip_id, layout, source_width, source_height))
    for cut in pacing_plan["cuts"]:
        lines.extend(split_boundary_edl(clip_id, cut))
    for phrase in caption_plan["phrases"]:
        lines.extend(caption_edl(phrase))
    for animation in motion_plan.get("animations", []):
        lines.extend(["*** Set Parameter Animation", "+ animation_json: " + json.dumps(animation, separators=(",", ":"))])
    lines.append("*** End EDL")
    return "\n".join(lines) + "\n"


def output_format_edl() -> list[str]:
    return [
        "*** Begin EDL",
        "*** Set Output Format",
        "+ aspect_ratio: 9:16",
        "+ safe_area: mobile",
        "+ platform: vertical_short",
    ]


def reframe_edl(clip_id: str, layout: dict, source_width: int, source_height: int) -> list[str]:
    center = layout.get("subject_center") or {"x": 0.5, "y": 0.42}
    source_aspect = source_width / max(source_height, 1)
    target_aspect = 9 / 16
    zoom = max(1.05, source_aspect / target_aspect) if source_aspect > target_aspect else 1.05
    return [
        "*** Set Effect",
        f"@@ anchor: clip_uuid={clip_id}",
        "+ effect: montage.reframe",
        "+ params_json: "
        + json.dumps({
            "zoom": round(min(zoom, 3.0), 3),
            "x": round((center["x"] - 0.5) * 2.0, 3),
            "y": round((center["y"] - 0.5) * 2.0, 3),
        }),
        "+ rationale: Subject-aware vertical talking-head reframe",
    ]


def split_boundary_edl(clip_id: str, cut: dict) -> list[str]:
    return [
        "*** Split Clip",
        f"@@ anchor: clip_uuid={clip_id}",
        f"+ at_s: {cut['start_s']}",
        "*** Split Clip",
        f"@@ anchor: clip_uuid={clip_id}",
        f"+ at_s: {cut['end_s']}",
    ]


def caption_edl(phrase: dict) -> list[str]:
    return [
        "*** Insert Caption",
        f"+ start_s: {phrase['start_s']}",
        f"+ end_s: {phrase['end_s']}",
        "+ text: " + json.dumps(phrase["text"]),
        f"+ position: {phrase.get('position', 'bottom')}",
        f"+ font_size: {phrase.get('font_size', 56)}",
        f"+ color: {phrase.get('color', '#FFFFFF')}",
        "+ safe_area: mobile",
        "+ word_timings_json: " + json.dumps(phrase.get("word_timings", [])),
    ]


def verify_readiness(*, candidate: dict, hook: dict, layout: dict, caption_plan: dict, edl: str) -> dict:
    issues = []
    if not candidate["is_candidate"]:
        issues.extend(candidate["issues"])
    if hook.get("start_s", 99.0) > 3.0:
        issues.append("hook does not appear in first 3 seconds")
    if not caption_plan["phrases"]:
        issues.append("captions missing")
    if caption_plan["readability"]["status"] != "ready":
        issues.append("caption readability needs review")
    if caption_plan["face_overlap_risk"] == "risk":
        issues.append("caption may block face")
    if layout.get("safe_area") != "mobile":
        issues.append("vertical safe area missing")
    if "Set Output Format" not in edl or "9:16" not in edl:
        issues.append("vertical output EDL missing")
    return {
        "status": "ready_for_review" if not issues else "blocked",
        "issues": issues,
        "checks": {
            "hook_first_3s": hook.get("start_s", 99.0) <= 3.0,
            "captions_present": bool(caption_plan["phrases"]),
            "captions_readable": caption_plan["readability"]["status"] == "ready",
            "face_not_blocked": caption_plan["face_overlap_risk"] != "risk",
            "safe_area": layout.get("safe_area") == "mobile",
            "black_frames_audio_gaps": "defer_to_render_verify",
        },
    }


def emphasis_keywords(transcript: dict) -> list[str]:
    text = " ".join(str(segment.get("text", "")) for segment in transcript.get("segments", []))
    seen = []
    for token in re.findall(r"[A-Za-z][A-Za-z']+", text):
        lower = token.lower()
        if len(lower) >= 7 and lower not in seen:
            seen.append(lower)
    return seen[:6]


def safe_id(value: str) -> str:
    return re.sub(r"[^A-Za-z0-9]+", "_", value).strip("_") or "clip"


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--asset-id", required=True)
    parser.add_argument("--clip-id", required=True)
    parser.add_argument("--source-width", type=int, required=True)
    parser.add_argument("--source-height", type=int, required=True)
    parser.add_argument("--transcript", required=True)
    parser.add_argument("--audio-energy", required=True)
    parser.add_argument("--moments")
    parser.add_argument("--face")
    parser.add_argument("--gaze")
    parser.add_argument("--shot")
    parser.add_argument("--composition")
    parser.add_argument("--frame-quality")
    parser.add_argument("--topic")
    return parser.parse_args()


def main() -> None:
    args = parse_args()
    print(json.dumps(build_talking_head_plan({
        "transcript": load_body(args.transcript),
        "audio_energy": load_body(args.audio_energy),
        "moments": load_body(args.moments),
        "face": load_body(args.face),
        "gaze": load_body(args.gaze),
        "shot": load_body(args.shot),
        "composition": load_body(args.composition),
        "frame_quality": load_body(args.frame_quality),
        "topic": load_body(args.topic),
    }, asset_id=args.asset_id, clip_id=args.clip_id, source_width=args.source_width, source_height=args.source_height), indent=2))


if __name__ == "__main__":
    main()
