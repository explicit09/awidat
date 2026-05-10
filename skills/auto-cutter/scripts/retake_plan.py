#!/usr/bin/env python3
"""Plan semantic retake/removal candidates from transcript context.

This is a planner, not an editor. It emits JSON plus optional EDL text
when a clip UUID is supplied; agents must still review/apply via
`apply_edl` and continuity checks.
"""

from __future__ import annotations

import argparse
import json
import re
from pathlib import Path
from typing import Any


STRONG_RESTART_PATTERNS = [
    ("cut_that", re.compile(r"\b(cut that|edit that out|we'?ll edit that out)\b", re.I)),
    ("start_over", re.compile(r"\b(start over|from the top|take two|scratch that)\b", re.I)),
    ("say_again", re.compile(r"\b(let me say that again|can i say that again|say that again)\b", re.I)),
    ("came_out_wrong", re.compile(r"\b(that came out wrong|that was wrong|wrong way to say)\b", re.I)),
    ("no_wait", re.compile(r"\b(no wait|wait no|hold on)\b", re.I)),
]
RESUME_PATTERNS = [
    ("where_was_i", re.compile(r"\b(where was i|back to what i was saying|okay continuing|to continue|anyway)\b", re.I)),
]
SIDECHAT_RE = re.compile(r"\b(are we rolling|is this recording|camera|mic|microphone|cut|editor|editing|pause)\b", re.I)
FILLER_RE = re.compile(r"\b(um+|uh+|er+|ah+|hmm|like|you know)\b", re.I)
WORD_RE = re.compile(r"[A-Za-z0-9']+")
STOP = {
    "the", "a", "an", "and", "or", "but", "to", "of", "in", "on", "for", "with",
    "is", "are", "was", "were", "it", "this", "that", "i", "we", "you", "they",
    "so", "just", "really", "kind", "like", "um", "uh", "you know",
}


def load_body(path: str | None) -> dict[str, Any]:
    if not path:
        return {}
    raw = json.loads(Path(path).read_text())
    return raw.get("data", raw)


def segments(body: dict[str, Any]) -> list[dict[str, Any]]:
    out = []
    for idx, seg in enumerate(body.get("segments", [])):
        start = float(seg.get("start_s", seg.get("start", 0.0)))
        end = float(seg.get("end_s", seg.get("end", start)))
        text = str(seg.get("text", "")).strip()
        if not text or end <= start:
            continue
        out.append(
            {
                "idx": idx,
                "start_s": start,
                "end_s": end,
                "speaker": str(seg.get("speaker_id") or seg.get("speaker") or "unknown"),
                "text": text,
            }
        )
    return out


def tokens(text: str) -> list[str]:
    return [w.lower() for w in WORD_RE.findall(text) if w.lower() not in STOP and len(w) > 2]


def overlap(a: str, b: str) -> float:
    sa, sb = set(tokens(a)), set(tokens(b))
    if not sa or not sb:
        return 0.0
    return len(sa & sb) / max(1, min(len(sa), len(sb)))


def filler_count(text: str) -> int:
    return len(FILLER_RE.findall(text))


def sentence_quality(seg: dict[str, Any]) -> float:
    words = tokens(seg["text"])
    score = min(1.0, len(words) / 14.0)
    score -= min(0.35, filler_count(seg["text"]) * 0.08)
    if re.search(r"[.!?]\s*$", seg["text"]):
        score += 0.1
    return max(0.0, min(1.0, score))


def continuity_risks(segs: list[dict[str, Any]], start_s: float, end_s: float) -> list[str]:
    touched = [s for s in segs if s["start_s"] < end_s and s["end_s"] > start_s]
    risks = []
    if len({s["speaker"] for s in touched}) > 1:
        risks.append("crosses_speaker_turns")
    text = " ".join(s["text"] for s in touched)
    if re.search(r"\b(as i said|as we said|earlier|remember|callback)\b", text, re.I):
        risks.append("possible_callback_or_reference")
    if len(touched) > 3:
        risks.append("broad_context_cut")
    return risks


def edl_for(
    candidate: dict[str, Any],
    clip_uuid: str | None,
    clip_source_start: float,
    clip_source_end: float | None,
) -> str:
    if not clip_uuid or candidate["requires_review"]:
        return ""

    bad_start = float(candidate["bad_take_start_s"])
    bad_end = float(candidate["bad_take_end_s"])
    fields: list[str]
    if bad_start <= clip_source_start + 0.05 and bad_end > clip_source_start:
        fields = [f"+ start: {bad_end:.3f}"]
    elif clip_source_end is not None and bad_end >= clip_source_end - 0.05 and bad_start < clip_source_end:
        fields = [f"+ end: {bad_start:.3f}"]
    else:
        # Interior retake removal needs split/delete/trim choreography on
        # actual timeline clip anchors. Keep it as a reviewed candidate rather
        # than emitting a fake graph op that apply_edl cannot represent.
        return ""

    return "\n".join(
        [
            "*** Begin EDL",
            "*** Trim Clip",
            f"@@ anchor: clip_uuid={clip_uuid}",
            *fields,
            "*** End EDL",
        ]
    )


def add_candidate(
    candidates: list[dict[str, Any]],
    segs: list[dict[str, Any]],
    candidate: dict[str, Any],
    clip_uuid: str | None,
    clip_source_start: float,
    clip_source_end: float | None,
) -> None:
    risks = continuity_risks(segs, candidate["bad_take_start_s"], candidate["bad_take_end_s"])
    confidence = float(candidate["confidence"])
    requires_review = confidence < 0.8 or bool(risks) or candidate.get("kind") == "repeated_idea"
    candidate["continuity_risks"] = risks
    candidate["requires_review"] = requires_review
    candidate["edl_fragment"] = edl_for(candidate, clip_uuid, clip_source_start, clip_source_end)
    candidate["edl_status"] = "ready" if candidate["edl_fragment"] else "review_or_split_required"
    candidates.append(candidate)


def detect_restart_candidates(
    segs: list[dict[str, Any]],
    clip_uuid: str | None,
    clip_source_start: float,
    clip_source_end: float | None,
) -> list[dict[str, Any]]:
    candidates: list[dict[str, Any]] = []
    for i, seg in enumerate(segs):
        text = seg["text"]
        for kind, pattern in STRONG_RESTART_PATTERNS:
            match = pattern.search(text)
            if not match:
                continue
            start = segs[max(0, i - 1)]["start_s"] if kind in {"cut_that", "start_over"} and i > 0 else seg["start_s"]
            end = seg["start_s"] + ((match.start() / max(1, len(text))) * (seg["end_s"] - seg["start_s"]))
            end = max(start + 0.2, min(end, seg["end_s"]))
            add_candidate(
                candidates,
                segs,
                {
                    "kind": kind,
                    "bad_take_start_s": round(start, 3),
                    "bad_take_end_s": round(end, 3),
                    "resume_start_s": round(seg["start_s"] + ((match.end() / max(1, len(text))) * (seg["end_s"] - seg["start_s"])), 3),
                    "confidence": 0.86 if kind != "cut_that" else 0.9,
                    "reason": f"restart language: {match.group(0)}",
                    "evidence": text,
                },
                clip_uuid,
                clip_source_start,
                clip_source_end,
            )
            break
    return candidates


def detect_resume_after_sidechat(
    segs: list[dict[str, Any]],
    clip_uuid: str | None,
    clip_source_start: float,
    clip_source_end: float | None,
) -> list[dict[str, Any]]:
    candidates: list[dict[str, Any]] = []
    for i, seg in enumerate(segs):
        match = next((m for _, p in RESUME_PATTERNS if (m := p.search(seg["text"]))), None)
        if not match or i == 0:
            continue
        back = []
        j = i - 1
        while j >= 0 and seg["start_s"] - segs[j]["end_s"] <= 45.0:
            if SIDECHAT_RE.search(segs[j]["text"]):
                back.append(segs[j])
                j -= 1
                continue
            break
        if not back:
            continue
        start = min(s["start_s"] for s in back)
        add_candidate(
            candidates,
            segs,
            {
                "kind": "side_chatter_before_resume",
                "bad_take_start_s": round(start, 3),
                "bad_take_end_s": round(seg["start_s"], 3),
                "resume_start_s": round(seg["start_s"], 3),
                "confidence": 0.82,
                "reason": "side chatter followed by resume language",
                "evidence": " | ".join([s["text"] for s in reversed(back)] + [seg["text"]]),
            },
            clip_uuid,
            clip_source_start,
            clip_source_end,
        )
    return candidates


def detect_repeated_ideas(
    segs: list[dict[str, Any]],
    clip_uuid: str | None,
    clip_source_start: float,
    clip_source_end: float | None,
) -> list[dict[str, Any]]:
    candidates: list[dict[str, Any]] = []
    for i, first in enumerate(segs):
        for later in segs[i + 1 : i + 7]:
            if later["start_s"] - first["end_s"] > 180.0:
                break
            if first["speaker"] != later["speaker"]:
                continue
            sim = overlap(first["text"], later["text"])
            if sim < 0.5:
                continue
            first_q = sentence_quality(first)
            later_q = sentence_quality(later)
            has_restart = any(p.search(first["text"]) for _, p in STRONG_RESTART_PATTERNS) or filler_count(first["text"]) > filler_count(later["text"])
            if later_q <= first_q + 0.12 and not has_restart:
                continue
            add_candidate(
                candidates,
                segs,
                {
                    "kind": "repeated_idea",
                    "bad_take_start_s": round(first["start_s"], 3),
                    "bad_take_end_s": round(first["end_s"], 3),
                    "resume_start_s": round(later["start_s"], 3),
                    "confidence": round(min(0.86, 0.56 + sim * 0.35 + max(0.0, later_q - first_q) * 0.2), 3),
                    "reason": "later nearby segment restates the same idea more cleanly",
                    "evidence": f"bad: {first['text']} | replacement: {later['text']}",
                    "overlap": round(sim, 3),
                },
                clip_uuid,
                clip_source_start,
                clip_source_end,
            )
            break
    return candidates


def dedupe(candidates: list[dict[str, Any]]) -> list[dict[str, Any]]:
    out: list[dict[str, Any]] = []
    for cand in sorted(candidates, key=lambda c: (c["bad_take_start_s"], -c["confidence"])):
        if any(abs(cand["bad_take_start_s"] - prev["bad_take_start_s"]) < 0.25 and abs(cand["bad_take_end_s"] - prev["bad_take_end_s"]) < 0.25 for prev in out):
            continue
        out.append(cand)
    return out


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--transcript", required=True)
    parser.add_argument("--audio-energy")
    parser.add_argument("--topic")
    parser.add_argument("--moments")
    parser.add_argument("--clip-uuid")
    parser.add_argument("--clip-source-start", type=float, default=0.0)
    parser.add_argument("--clip-source-end", type=float)
    args = parser.parse_args()

    transcript = load_body(args.transcript)
    segs = segments(transcript)
    candidates = []
    candidates.extend(detect_restart_candidates(segs, args.clip_uuid, args.clip_source_start, args.clip_source_end))
    candidates.extend(detect_resume_after_sidechat(segs, args.clip_uuid, args.clip_source_start, args.clip_source_end))
    candidates.extend(detect_repeated_ideas(segs, args.clip_uuid, args.clip_source_start, args.clip_source_end))
    candidates = dedupe(candidates)
    edl_fragments = [c["edl_fragment"] for c in candidates if c.get("edl_fragment")]
    print(
        json.dumps(
            {
                "status": "planned",
                "candidates": candidates,
                "edl_fragments": edl_fragments,
                "summary": {
                    "candidate_count": len(candidates),
                    "review_required_count": sum(1 for c in candidates if c["requires_review"]),
                    "auto_edl_count": len(edl_fragments),
                },
            },
            indent=2,
        )
    )


if __name__ == "__main__":
    main()
