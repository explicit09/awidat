#!/usr/bin/env python3
"""Plan generated overlay animation slots for Awidat timelines."""

from __future__ import annotations

import argparse
import json
import re
from pathlib import Path


def slugify(value: str) -> str:
    slug = re.sub(r"[^a-z0-9]+", "-", value.lower()).strip("-")
    return slug or "overlay"


def normalize_slot(raw: dict, *, output_root: Path) -> dict:
    name = str(raw.get("name", "Overlay")).strip() or "Overlay"
    slug = slugify(name)
    duration_s = float(raw.get("duration_s", 0.0))
    if duration_s <= 0.0:
        raise ValueError(f"{name}: duration_s must be positive")
    engine = str(raw.get("engine", "canvas")).strip().lower() or "canvas"
    mode = str(raw.get("mode", "full_frame")).strip().lower() or "full_frame"
    slot_dir = output_root / slug
    asset_path = slot_dir / "overlay.webm"
    brief_path = slot_dir / "brief.md"
    subject_aware = bool(raw.get("subject_aware", False))
    matte_path = str(
        raw.get("matte_path") or (slot_dir / "subject-matte.webm")
    ).strip()
    return {
        "name": name,
        "slug": slug,
        "anchor": str(raw.get("anchor", "")).strip(),
        "start_s": round(float(raw.get("start_s", 0.0)), 3),
        "duration_s": round(duration_s, 3),
        "engine": engine,
        "mode": mode,
        "prompt": str(raw.get("prompt", "")).strip(),
        "asset_path": asset_path.as_posix(),
        "brief_path": brief_path.as_posix(),
        "subject_aware": subject_aware,
        "subject_prompt": str(raw.get("subject_prompt", "main subject")).strip()
        or "main subject",
        "matte_path": matte_path,
        "layer_order": (
            ["base_video", "overlay_asset", "subject_matte"]
            if subject_aware
            else ["base_video", "overlay_asset"]
        ),
        "fallback": str(raw.get("fallback", "")).strip()
        or "render the overlay as a normal foreground asset if the subject matte is unavailable",
    }


def build_edl_hint(slot: dict) -> str:
    corner = "top_right" if slot["mode"] == "pip" else "center"
    scale = 0.35 if slot["mode"] == "pip" else 1.0
    return "\n".join([
        "*** Begin EDL",
        "*** Insert PiP",
        f"@@ anchor: {slot['anchor'] or 'clip_uuid=<target-clip>'}",
        f"+ asset: {slot['asset_path']}",
        f"+ duration_s: {slot['duration_s']}",
        f"+ corner: {corner}",
        f"+ scale: {scale}",
        "+ margin_pct: 0.04",
        "*** End EDL",
    ])


def build_slot_brief(slot: dict) -> str:
    lines = [
        f"# {slot['name']}",
        "",
        f"Duration: {slot['duration_s']:.3f}s",
        f"Engine: {slot['engine']}",
        f"Mode: {slot['mode']}",
        f"Deliver: {slot['asset_path']}",
        "",
        "Contract:",
        "- Match the exact duration.",
        "- Use a transparent or keyed background unless the slot is intentionally full-frame.",
        "- Keep typography inside mobile safe areas when the target format is vertical.",
        "- Export a web-compatible video asset that Awidat can place as a media overlay.",
    ]
    if slot.get("subject_aware"):
        lines.extend([
            "- Subject-aware compositing required.",
            f"- Subject prompt: {slot['subject_prompt']}",
            f"- Required matte/cutout artifact: {slot['matte_path']}",
            "- Layer order: base video -> overlay asset -> subject matte.",
            f"- Fallback: {slot['fallback']}",
        ])
    lines.extend([
        "",
        "Creative brief:",
        slot["prompt"] or "Generate a focused motion graphic that supports the edit beat.",
    ])
    return "\n".join(lines) + "\n"


def build_animation_manifest(slots: list[dict], *, output_root: Path) -> dict:
    normalized = []
    for raw in slots:
        slot = normalize_slot(raw, output_root=output_root)
        slot["brief"] = build_slot_brief(slot)
        slot["edl_hint"] = build_edl_hint(slot)
        normalized.append(slot)
    return {
        "version": 1,
        "output_root": output_root.as_posix(),
        "slots": normalized,
    }


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--slots-json", required=True)
    parser.add_argument("--output-root", default="generated/overlays")
    args = parser.parse_args()

    raw = json.loads(Path(args.slots_json).read_text())
    slots = raw.get("slots", raw if isinstance(raw, list) else [])
    print(json.dumps(
        build_animation_manifest(slots, output_root=Path(args.output_root)),
        indent=2,
    ))


if __name__ == "__main__":
    main()
