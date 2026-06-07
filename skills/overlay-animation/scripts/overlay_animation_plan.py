#!/usr/bin/env python3
"""Plan generated overlay animation slots for Montage timelines."""

from __future__ import annotations

import argparse
import json
import re
from pathlib import Path


def _unit(value: object, *, label: str, default: float | None = None) -> float | None:
    if value is None:
        return default
    number = float(value)
    if number < 0.0 or number > 1.0:
        raise ValueError(f"{label} must be in 0..=1")
    return round(number, 3)


def _hex_color(value: object, *, default: str = "#FFFFFF") -> str:
    color = str(value or default).strip() or default
    if not re.fullmatch(r"#[0-9a-fA-F]{6}", color):
        raise ValueError("font_color must be a #RRGGBB value")
    return color.upper()


def slugify(value: str) -> str:
    slug = re.sub(r"[^a-z0-9]+", "-", value.lower()).strip("-")
    return slug or "overlay"


def normalize_text_layer(raw: dict, *, slot_name: str, index: int) -> dict:
    text = str(raw.get("text", "")).strip()
    if not text:
        raise ValueError(f"{slot_name}: text_layers[{index}] text is required")
    font_size = int(raw.get("font_size", 96))
    if font_size <= 0:
        raise ValueError(f"{slot_name}: text_layers[{index}] font_size must be positive")
    rotation = float(raw.get("rotation", 0.0))
    if rotation < -180.0 or rotation > 180.0:
        raise ValueError(f"{slot_name}: text_layers[{index}] rotation must be in -180..=180")
    x_value = raw.get("x", raw.get("x_pos", 0.5))
    y_value = raw.get("y", raw.get("y_pos", 0.5))
    return {
        "text": text,
        "font_family": str(raw.get("font_family", "Inter")).strip() or "Inter",
        "font_size": font_size,
        "font_color": _hex_color(raw.get("font_color", raw.get("color"))),
        "opacity": _unit(raw.get("opacity", 1.0), label=f"{slot_name}: text_layers[{index}] opacity"),
        "rotation": round(rotation, 3),
        "weight": str(raw.get("weight", raw.get("font_weight", "normal"))).strip() or "normal",
        "position": {
            "x": _unit(x_value, label=f"{slot_name}: text_layers[{index}] x"),
            "y": _unit(y_value, label=f"{slot_name}: text_layers[{index}] y"),
        },
    }


def normalize_detection(raw: dict | None, *, slot_name: str) -> dict:
    raw = raw or {}
    object_classes = [
        str(item).strip()
        for item in raw.get("object_classes", raw.get("classes", []))
        if str(item).strip()
    ]
    return {
        "object_classes": object_classes,
        "confidence_threshold": _unit(
            raw.get("confidence_threshold", raw.get("confidence", 0.5)),
            label=f"{slot_name}: confidence_threshold",
        ),
        "iou_threshold": _unit(
            raw.get("iou_threshold", raw.get("iou", 0.7)),
            label=f"{slot_name}: iou_threshold",
        ),
        "mask_threshold": _unit(
            raw.get("mask_threshold", 0.3),
            label=f"{slot_name}: mask_threshold",
        ),
        "preview_frame_path": str(raw.get("preview_frame_path", "")).strip() or None,
        "occlusion_preview_path": str(raw.get("occlusion_preview_path", "")).strip() or None,
    }


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
    text_layers = [
        normalize_text_layer(layer, slot_name=name, index=index)
        for index, layer in enumerate(raw.get("text_layers", []))
    ]
    detection = normalize_detection(raw.get("detection"), slot_name=name)
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
        "text_layers": text_layers,
        "detection": detection,
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
        "- Export a web-compatible video asset that Montage can place as a media overlay.",
    ]
    if slot.get("subject_aware"):
        lines.extend([
            "- Subject-aware compositing required.",
            f"- Subject prompt: {slot['subject_prompt']}",
            f"- Required matte/cutout artifact: {slot['matte_path']}",
            "- Layer order: base video -> overlay asset -> subject matte.",
            f"- Fallback: {slot['fallback']}",
        ])
        detection = slot.get("detection") or {}
        classes = detection.get("object_classes") or []
        if classes:
            lines.append(f"- Detection object classes: {', '.join(classes)}")
        lines.append(
            "- Detection thresholds: "
            f"confidence={detection.get('confidence_threshold')}, "
            f"iou={detection.get('iou_threshold')}, "
            f"mask={detection.get('mask_threshold')}"
        )
        if detection.get("preview_frame_path") or detection.get("occlusion_preview_path"):
            lines.append(
                "- Preview evidence: "
                f"frame={detection.get('preview_frame_path') or 'pending'}, "
                f"occlusion={detection.get('occlusion_preview_path') or 'pending'}"
            )
    if slot.get("text_layers"):
        lines.extend(["", "Text layers:"])
        for index, layer in enumerate(slot["text_layers"], start=1):
            position = layer["position"]
            lines.append(
                f"- {index}. {layer['text']} | {layer['font_family']} "
                f"{layer['font_size']}px {layer['weight']} | {layer['font_color']} | "
                f"opacity={layer['opacity']} rotation={layer['rotation']} "
                f"position=({position['x']}, {position['y']})"
            )
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
