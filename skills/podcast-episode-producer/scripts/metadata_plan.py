#!/usr/bin/env python3
"""Create a podcast publishing metadata package from montage sidecars."""

from __future__ import annotations

import argparse
import json
import re
from pathlib import Path


def load_body(path: str | None) -> dict:
    if not path:
        return {}
    raw = json.loads(Path(path).read_text())
    return raw.get("data", raw)


def clean_words(text: str) -> list[str]:
    return [w for w in re.findall(r"[A-Za-z0-9][A-Za-z0-9'-]*", text) if len(w) > 3]


def main() -> None:
    p = argparse.ArgumentParser()
    p.add_argument("--transcript", required=True)
    p.add_argument("--topic")
    p.add_argument("--moments")
    p.add_argument("--frame-quality")
    args = p.parse_args()

    transcript = load_body(args.transcript)
    topic = load_body(args.topic)
    moments = load_body(args.moments)
    quality = load_body(args.frame_quality)
    segments = transcript.get("segments", [])
    full_text = " ".join(str(s.get("text", "")) for s in segments)
    topics = topic.get("topics", [])
    beats = moments.get("moments", [])
    hook = next((m for m in beats if m.get("kind") == "hook"), beats[0] if beats else {})
    hook_note = hook.get("note") or (segments[0].get("text", "") if segments else "Podcast episode")
    title_words = clean_words(hook_note)[:8]
    title = " ".join(title_words).title() or "Podcast Episode"
    chapters = [
        {
            "start_s": round(float(t.get("start_s", 0.0)), 3),
            "label": str(t.get("label", "Chapter"))[:48],
        }
        for t in topics
    ]
    counts: dict[str, int] = {}
    for w in clean_words(full_text.lower()):
        counts[w] = counts.get(w, 0) + 1
    tags = [w for w, _ in sorted(counts.items(), key=lambda kv: kv[1], reverse=True)[:12]]
    qframes = quality.get("per_frame", [])
    thumbs = sorted(
        [
            {
                "t_s": round(float(f.get("t_s", 0.0)), 3),
                "blur": f.get("blur"),
                "brightness": f.get("brightness"),
                "contrast": f.get("contrast"),
            }
            for f in qframes
            if f.get("is_sharp")
        ],
        key=lambda f: (float(f.get("contrast") or 0), float(f.get("brightness") or 0)),
        reverse=True,
    )[:5]
    print(json.dumps({
        "title": title,
        "description": hook_note.strip(),
        "chapters": chapters,
        "tags": tags,
        "thumbnail_candidates": thumbs,
    }, indent=2))


if __name__ == "__main__":
    main()
