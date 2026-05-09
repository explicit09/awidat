#!/usr/bin/env python3
"""Convert color-analysis policy into graph-native EDL text."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
from typing import Any


FIELDS = [
    "exposure_ev",
    "contrast",
    "saturation",
    "temperature",
    "tint",
    "shadows",
    "highlights",
]


def load_body(path: str) -> dict[str, Any]:
    raw = json.loads(Path(path).read_text())
    return raw.get("data", raw)


def fmt_num(value: Any) -> str:
    return f"{float(value):.6f}".rstrip("0").rstrip(".")


def build_edl(clip_uuid: str, correction: dict[str, Any]) -> str:
    lines = [
        "*** Begin EDL",
        "*** Set Color Correction",
        f"@@ anchor: clip_uuid={clip_uuid}",
    ]
    for field in FIELDS:
        if field in correction and correction[field] is not None:
            lines.append(f"+ {field}: {fmt_num(correction[field])}")
    lines.append("*** End EDL")
    return "\n".join(lines)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--color-index", required=True)
    parser.add_argument("--clip-uuid", required=True)
    parser.add_argument("--scene-id")
    args = parser.parse_args()

    data = load_body(args.color_index)
    target = data.get("summary", {})
    if args.scene_id:
        scenes = data.get("scenes", [])
        target = next((s for s in scenes if s.get("scene_id") == args.scene_id), {})

    policy = target.get("policy", {})
    action = policy.get("recommended_action", "review_only")
    correction = target.get("recommended_correction", {})
    ops = []
    edl_text = ""
    review = {
        "requires_review": bool(policy.get("requires_review", True)),
        "requires_contact_sheet": bool(policy.get("requires_contact_sheet", True)),
        "reason": policy.get("reason", target.get("issue_tags", [])),
    }

    if action == "auto_correct" and correction:
        edl_text = build_edl(args.clip_uuid, correction)
        ops.append(
            {
                "kind": "Set Color Correction",
                "anchor": {"clip_uuid": args.clip_uuid},
                "fields": {k: correction[k] for k in FIELDS if k in correction},
            }
        )
        review["requires_review"] = False
    elif action == "no_op":
        review["requires_contact_sheet"] = False

    print(
        json.dumps(
            {
                "status": "planned",
                "scene_id": args.scene_id,
                "policy": policy,
                "action": action,
                "ops": ops,
                "edl_text": edl_text,
                "review": review,
            },
            indent=2,
        )
    )


if __name__ == "__main__":
    main()
