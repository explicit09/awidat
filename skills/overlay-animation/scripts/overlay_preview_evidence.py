#!/usr/bin/env python3
"""Generate deterministic subject-aware overlay preview evidence."""

from __future__ import annotations

import argparse
import json
import struct
import zlib
from pathlib import Path
from typing import Any


Color = tuple[int, int, int]
Image = list[list[Color]]


def _hex_color(value: object, *, default: str = "#FFFFFF") -> Color:
    raw = str(value or default).strip() or default
    if len(raw) != 7 or raw[0] != "#":
        raise ValueError(f"invalid #RRGGBB color: {raw}")
    try:
        return (int(raw[1:3], 16), int(raw[3:5], 16), int(raw[5:7], 16))
    except ValueError as exc:
        raise ValueError(f"invalid #RRGGBB color: {raw}") from exc


def _blank(width: int, height: int, color: Color) -> Image:
    return [[color for _ in range(width)] for _ in range(height)]


def _rect(image: Image, *, x: int, y: int, width: int, height: int, color: Color) -> None:
    image_height = len(image)
    image_width = len(image[0]) if image else 0
    x0 = max(0, x)
    y0 = max(0, y)
    x1 = min(image_width, x + width)
    y1 = min(image_height, y + height)
    for row in range(y0, y1):
        for col in range(x0, x1):
            image[row][col] = color


def _text_block(layer: dict[str, Any], *, frame_width: int, frame_height: int) -> dict[str, int | Color]:
    text = str(layer.get("text", "")).strip()
    if not text:
        raise ValueError("text layer text is required")
    position = layer.get("position") if isinstance(layer.get("position"), dict) else {}
    center_x = float(position.get("x", layer.get("x", 0.5)))
    center_y = float(position.get("y", layer.get("y", 0.5)))
    font_size = int(layer.get("font_size", 48))
    width = max(24, min(frame_width, int(max(1, len(text)) * font_size * 0.62)))
    height = max(16, min(frame_height, int(font_size * 0.72)))
    return {
        "x": int((frame_width * center_x) - (width / 2)),
        "y": int((frame_height * center_y) - (height / 2)),
        "width": width,
        "height": height,
        "color": _hex_color(layer.get("font_color"), default="#F8F7F1"),
    }


def _subject_bbox(raw: dict[str, Any]) -> dict[str, int] | None:
    subject = raw.get("subject") if isinstance(raw.get("subject"), dict) else {}
    bbox = subject.get("bbox") if isinstance(subject.get("bbox"), dict) else None
    if bbox is None:
        return None
    parsed = {
        "x": int(bbox.get("x", 0)),
        "y": int(bbox.get("y", 0)),
        "width": int(bbox.get("width", 0)),
        "height": int(bbox.get("height", 0)),
    }
    if parsed["width"] <= 0 or parsed["height"] <= 0:
        return None
    return parsed


def _png_chunk(kind: bytes, payload: bytes) -> bytes:
    return (
        struct.pack(">I", len(payload))
        + kind
        + payload
        + struct.pack(">I", zlib.crc32(kind + payload) & 0xFFFFFFFF)
    )


def _write_png(path: Path, image: Image) -> None:
    height = len(image)
    width = len(image[0]) if image else 0
    raw_rows = bytearray()
    for row in image:
        raw_rows.append(0)
        for red, green, blue in row:
            raw_rows.extend((red, green, blue))
    payload = b"".join([
        b"\x89PNG\r\n\x1a\n",
        _png_chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)),
        _png_chunk(b"IDAT", zlib.compress(bytes(raw_rows))),
        _png_chunk(b"IEND", b""),
    ])
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(payload)


def _project_relative(path: Path, *, project_root: Path) -> str:
    try:
        return path.resolve().relative_to(project_root.resolve()).as_posix()
    except ValueError:
        return path.as_posix()


def _issue(slot_name: str, code: str, message: str) -> dict[str, str]:
    return {"slot": slot_name, "code": code, "message": message}


def _count_color_in_bbox(image: Image, *, bbox: dict[str, int], colors: set[Color]) -> int:
    count = 0
    for row in range(max(0, bbox["y"]), min(len(image), bbox["y"] + bbox["height"])):
        for col in range(max(0, bbox["x"]), min(len(image[row]), bbox["x"] + bbox["width"])):
            if image[row][col] in colors:
                count += 1
    return count


def _is_inside_bbox(*, row: int, col: int, bbox: dict[str, int]) -> bool:
    return (
        bbox["x"] <= col < bbox["x"] + bbox["width"]
        and bbox["y"] <= row < bbox["y"] + bbox["height"]
    )


def _count_changed_pixels(
    before: Image,
    after: Image,
    *,
    bbox: dict[str, int],
    inside_subject: bool,
) -> int:
    count = 0
    for row, before_row in enumerate(before):
        for col, before_color in enumerate(before_row):
            if _is_inside_bbox(row=row, col=col, bbox=bbox) != inside_subject:
                continue
            if after[row][col] != before_color:
                count += 1
    return count


def _background_treatment(slot: dict[str, Any]) -> dict[str, Any] | None:
    treatment = slot.get("background_treatment")
    if not isinstance(treatment, dict):
        return None
    mode = str(treatment.get("mode", "color")).strip().lower()
    return {**treatment, "mode": mode}


def _render_background_replacement_preview(
    *,
    slot: dict[str, Any],
    fixture: dict[str, Any],
    bbox: dict[str, int],
    output_root: Path,
    project_root: Path,
) -> dict[str, Any]:
    name = str(slot.get("name", "Overlay")).strip() or "Overlay"
    slug = str(slot.get("slug", name.lower().replace(" ", "-"))).strip() or "overlay"
    frame = fixture.get("frame") if isinstance(fixture.get("frame"), dict) else {}
    width = int(frame.get("width", 1280))
    height = int(frame.get("height", 720))
    if width <= 0 or height <= 0:
        raise ValueError(f"{name}: frame dimensions must be positive")
    background = _hex_color(frame.get("background"), default="#20242A")
    subject = fixture.get("subject") if isinstance(fixture.get("subject"), dict) else {}
    subject_color = _hex_color(subject.get("color"), default="#D8B38A")
    treatment = _background_treatment(slot) or {}
    mode = str(treatment.get("mode", "color"))
    if mode not in {"color", "transparent"}:
        return {
            "name": name,
            "slug": slug,
            "status": "blocked",
            "issues": [_issue(name, "unsupported_background_mode", f"unsupported background mode: {mode}")],
        }

    ordinary = _blank(width, height, background)
    _rect(ordinary, **bbox, color=subject_color)
    replacement = (0, 0, 0) if mode == "transparent" else _hex_color(treatment.get("color"), default="#101820")
    background_preview = _blank(width, height, replacement)
    _rect(background_preview, **bbox, color=subject_color)

    slot_dir = output_root / slug
    ordinary_path = slot_dir / "ordinary-background.png"
    background_path = slot_dir / "background-treatment.png"
    scorecard_path = slot_dir / "background-scorecard.json"
    _write_png(ordinary_path, ordinary)
    _write_png(background_path, background_preview)

    subject_changed = _count_changed_pixels(
        ordinary,
        background_preview,
        bbox=bbox,
        inside_subject=True,
    )
    background_replaced = _count_changed_pixels(
        ordinary,
        background_preview,
        bbox=bbox,
        inside_subject=False,
    )
    scorecard = {
        "version": 1,
        "slot": name,
        "evidence_kind": "background_replacement_preview",
        "background_mode": mode,
        "frame": {"width": width, "height": height},
        "subject_bbox": bbox,
        "background_replaced_pixels": background_replaced,
        "subject_changed_pixels": subject_changed,
        "subject_preserved": subject_changed == 0 and background_replaced > 0,
    }
    scorecard_path.write_text(json.dumps(scorecard, indent=2) + "\n")

    return {
        "name": name,
        "slug": slug,
        "status": "ready",
        "evidence_kind": "background_replacement_preview",
        "background_mode": mode,
        "ordinary_preview_path": _project_relative(ordinary_path, project_root=project_root),
        "background_preview_path": _project_relative(background_path, project_root=project_root),
        "scorecard_path": _project_relative(scorecard_path, project_root=project_root),
        "background_replaced_pixels": background_replaced,
        "subject_changed_pixels": subject_changed,
        "subject_preserved": subject_changed == 0 and background_replaced > 0,
        "issues": [],
    }


def render_slot_preview(
    slot: dict[str, Any],
    *,
    fixture: dict[str, Any],
    output_root: Path,
    project_root: Path,
) -> dict[str, Any]:
    name = str(slot.get("name", "Overlay")).strip() or "Overlay"
    slug = str(slot.get("slug", name.lower().replace(" ", "-"))).strip() or "overlay"
    if not bool(slot.get("subject_aware", False)):
        return {"name": name, "slug": slug, "status": "skipped", "issues": []}

    bbox = _subject_bbox(fixture)
    if bbox is None:
        return {
            "name": name,
            "slug": slug,
            "status": "blocked",
            "issues": [_issue(name, "missing_subject_bounds", "subject-aware preview requires fixture subject bbox")],
        }
    if _background_treatment(slot) is not None:
        return _render_background_replacement_preview(
            slot=slot,
            fixture=fixture,
            bbox=bbox,
            output_root=output_root,
            project_root=project_root,
        )

    frame = fixture.get("frame") if isinstance(fixture.get("frame"), dict) else {}
    width = int(frame.get("width", 1280))
    height = int(frame.get("height", 720))
    if width <= 0 or height <= 0:
        raise ValueError(f"{name}: frame dimensions must be positive")
    background = _hex_color(frame.get("background"), default="#20242A")
    subject = fixture.get("subject") if isinstance(fixture.get("subject"), dict) else {}
    subject_color = _hex_color(subject.get("color"), default="#D8B38A")
    layers = [layer for layer in slot.get("text_layers", []) if isinstance(layer, dict)]
    if not layers:
        return {
            "name": name,
            "slug": slug,
            "status": "blocked",
            "issues": [_issue(name, "missing_text_layers", "subject-aware preview requires at least one text layer")],
        }

    text_blocks = [_text_block(layer, frame_width=width, frame_height=height) for layer in layers]
    text_colors = {block["color"] for block in text_blocks if isinstance(block["color"], tuple)}

    ordinary = _blank(width, height, background)
    _rect(ordinary, **bbox, color=subject_color)
    for block in text_blocks:
        _rect(ordinary, x=int(block["x"]), y=int(block["y"]), width=int(block["width"]), height=int(block["height"]), color=block["color"])  # type: ignore[arg-type]

    subject_safe = _blank(width, height, background)
    for block in text_blocks:
        _rect(subject_safe, x=int(block["x"]), y=int(block["y"]), width=int(block["width"]), height=int(block["height"]), color=block["color"])  # type: ignore[arg-type]
    _rect(subject_safe, **bbox, color=subject_color)

    slot_dir = output_root / slug
    ordinary_path = slot_dir / "ordinary-overlay.png"
    safe_path = slot_dir / "subject-safe-overlay.png"
    scorecard_path = slot_dir / "scorecard.json"
    _write_png(ordinary_path, ordinary)
    _write_png(safe_path, subject_safe)

    ordinary_overlap = _count_color_in_bbox(ordinary, bbox=bbox, colors=text_colors)
    safe_overlap = _count_color_in_bbox(subject_safe, bbox=bbox, colors=text_colors)
    scorecard = {
        "version": 1,
        "slot": name,
        "evidence_kind": "subject_safe_overlay_preview",
        "frame": {"width": width, "height": height},
        "subject_bbox": bbox,
        "ordinary_subject_overlap_pixels": ordinary_overlap,
        "safe_subject_overlap_pixels": safe_overlap,
        "subject_occludes_overlay": ordinary_overlap > 0 and safe_overlap == 0,
    }
    scorecard_path.write_text(json.dumps(scorecard, indent=2) + "\n")

    return {
        "name": name,
        "slug": slug,
        "status": "ready",
        "evidence_kind": "subject_safe_overlay_preview",
        "ordinary_preview_path": _project_relative(ordinary_path, project_root=project_root),
        "subject_safe_preview_path": _project_relative(safe_path, project_root=project_root),
        "scorecard_path": _project_relative(scorecard_path, project_root=project_root),
        "ordinary_subject_overlap_pixels": ordinary_overlap,
        "safe_subject_overlap_pixels": safe_overlap,
        "subject_occludes_overlay": ordinary_overlap > 0 and safe_overlap == 0,
        "issues": [],
    }


def build_preview_report(
    manifest: dict[str, Any],
    *,
    fixture: dict[str, Any],
    output_root: Path,
    project_root: Path,
) -> dict[str, Any]:
    fixtures = fixture.get("slots", {}) if isinstance(fixture.get("slots"), dict) else {}
    slot_reports = []
    for slot in manifest.get("slots", []):
        if not isinstance(slot, dict):
            continue
        slug = str(slot.get("slug", "")).strip()
        name = str(slot.get("name", "")).strip()
        slot_fixture = fixtures.get(slug) or fixtures.get(name) or {}
        slot_reports.append(
            render_slot_preview(
                slot,
                fixture=slot_fixture,
                output_root=output_root,
                project_root=project_root,
            )
        )
    issues = [issue for report in slot_reports for issue in report.get("issues", [])]
    return {
        "version": 1,
        "status": "blocked" if issues else "ready",
        "issue_count": len(issues),
        "issues": issues,
        "slots": slot_reports,
    }


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--manifest", required=True)
    parser.add_argument("--fixture-json", required=True)
    parser.add_argument("--output-root", default="generated/previews")
    parser.add_argument("--project-root", default=".")
    args = parser.parse_args()

    report = build_preview_report(
        json.loads(Path(args.manifest).read_text()),
        fixture=json.loads(Path(args.fixture_json).read_text()),
        output_root=Path(args.output_root),
        project_root=Path(args.project_root),
    )
    print(json.dumps(report, indent=2))
    if report["status"] != "ready":
        raise SystemExit(1)


if __name__ == "__main__":
    main()
