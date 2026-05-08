#!/usr/bin/env python3
"""Group word-level transcript timing into short caption phrases."""

from __future__ import annotations

import argparse
import json
import re
from pathlib import Path


def load_body(path: str) -> dict:
    raw = json.loads(Path(path).read_text())
    return raw.get("data", raw)


def words(body: dict) -> list[dict]:
    if body.get("words"):
        return body["words"]
    out = []
    for seg in body.get("segments", []):
        text = str(seg.get("text", ""))
        toks = re.findall(r"[A-Za-z0-9']+|[.,!?]", text)
        start = float(seg.get("start_s", seg.get("start", 0.0)))
        end = float(seg.get("end_s", seg.get("end", start)))
        for i, tok in enumerate([t for t in toks if re.match(r"[A-Za-z0-9']", t)]):
            span = max(0.01, end - start)
            out.append({
                "word": tok,
                "start_s": start + span * i / max(1, len(toks)),
                "end_s": start + span * (i + 1) / max(1, len(toks)),
            })
    return out


def main() -> None:
    p = argparse.ArgumentParser()
    p.add_argument("--transcript", required=True)
    p.add_argument("--max-words", type=int, default=4)
    p.add_argument("--hot-start-s", type=float)
    p.add_argument("--hot-end-s", type=float)
    args = p.parse_args()

    items = words(load_body(args.transcript))
    phrases = []
    for i in range(0, len(items), max(1, args.max_words)):
        chunk = items[i : i + args.max_words]
        if not chunk:
            continue
        start = float(chunk[0].get("start_s", 0.0))
        end = float(chunk[-1].get("end_s", start))
        hot = (
            args.hot_start_s is not None
            and args.hot_end_s is not None
            and end >= args.hot_start_s
            and start <= args.hot_end_s
        )
        phrases.append({
            "text": " ".join(str(w.get("word", "")).strip() for w in chunk).strip(),
            "start_s": round(start, 3),
            "end_s": round(max(end, start + 0.6), 3),
            "position": "bottom",
            "font_size": 56,
            "color": "#FFD400" if hot else "#FFFFFF",
            "font_weight": "bold" if hot else "normal",
            "animation": "fade_in_out",
        })
    print(json.dumps({"phrases": phrases}, indent=2))


if __name__ == "__main__":
    main()
