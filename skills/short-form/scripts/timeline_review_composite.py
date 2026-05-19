#!/usr/bin/env python3
"""Generate an agent-readable timeline review composite PNG.

The artifact is deterministic and dependency-free so it can run in the skill
environment without image libraries. It is evidence for review, not a renderer.
"""

from __future__ import annotations

import argparse
import json
import struct
import zlib
from pathlib import Path
from typing import Any


Color = tuple[int, int, int]
Image = list[list[Color]]


BACKGROUND = (18, 22, 28)
PANEL = (33, 40, 50)
TEXT = (235, 239, 245)
MUTED = (124, 140, 158)
SOURCE = (76, 150, 255)
OUTPUT = (90, 208, 151)
WAVE = (68, 204, 255)
WORD = (255, 218, 92)
SILENCE = (92, 101, 117)
CUT = (255, 93, 93)


FONT: dict[str, tuple[str, ...]] = {
    "A": ("111", "101", "111", "101", "101"),
    "B": ("110", "101", "110", "101", "110"),
    "C": ("111", "100", "100", "100", "111"),
    "D": ("110", "101", "101", "101", "110"),
    "E": ("111", "100", "110", "100", "111"),
    "F": ("111", "100", "110", "100", "100"),
    "G": ("111", "100", "101", "101", "111"),
    "H": ("101", "101", "111", "101", "101"),
    "I": ("111", "010", "010", "010", "111"),
    "J": ("001", "001", "001", "101", "111"),
    "K": ("101", "101", "110", "101", "101"),
    "L": ("100", "100", "100", "100", "111"),
    "M": ("101", "111", "111", "101", "101"),
    "N": ("101", "111", "111", "111", "101"),
    "O": ("111", "101", "101", "101", "111"),
    "P": ("111", "101", "111", "100", "100"),
    "Q": ("111", "101", "101", "111", "001"),
    "R": ("111", "101", "111", "110", "101"),
    "S": ("111", "100", "111", "001", "111"),
    "T": ("111", "010", "010", "010", "010"),
    "U": ("101", "101", "101", "101", "111"),
    "V": ("101", "101", "101", "101", "010"),
    "W": ("101", "101", "111", "111", "101"),
    "X": ("101", "101", "010", "101", "101"),
    "Y": ("101", "101", "010", "010", "010"),
    "Z": ("111", "001", "010", "100", "111"),
    "0": ("111", "101", "101", "101", "111"),
    "1": ("010", "110", "010", "010", "111"),
    "2": ("111", "001", "111", "100", "111"),
    "3": ("111", "001", "111", "001", "111"),
    "4": ("101", "101", "111", "001", "001"),
    "5": ("111", "100", "111", "001", "111"),
    "6": ("111", "100", "111", "101", "111"),
    "7": ("111", "001", "001", "001", "001"),
    "8": ("111", "101", "111", "101", "111"),
    "9": ("111", "101", "111", "001", "111"),
    "-": ("000", "000", "111", "000", "000"),
    ".": ("000", "000", "000", "000", "010"),
}


def _blank(width: int, height: int, color: Color) -> Image:
    return [[color for _ in range(width)] for _ in range(height)]


def _rect(image: Image, *, x: int, y: int, width: int, height: int, color: Color) -> None:
    image_height = len(image)
    image_width = len(image[0]) if image else 0
    for row in range(max(0, y), min(image_height, y + height)):
        for col in range(max(0, x), min(image_width, x + width)):
            image[row][col] = color


def _draw_text(image: Image, text: str, *, x: int, y: int, color: Color, scale: int = 2) -> None:
    cursor = x
    for char in text.upper()[:64]:
        if char == " ":
            cursor += 4 * scale
            continue
        glyph = FONT.get(char)
        if glyph is None:
            _rect(image, x=cursor, y=y + 4 * scale, width=2 * scale, height=scale, color=color)
            cursor += 4 * scale
            continue
        for gy, row in enumerate(glyph):
            for gx, bit in enumerate(row):
                if bit == "1":
                    _rect(
                        image,
                        x=cursor + gx * scale,
                        y=y + gy * scale,
                        width=scale,
                        height=scale,
                        color=color,
                    )
        cursor += 4 * scale


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
    payload = b"".join(
        [
            b"\x89PNG\r\n\x1a\n",
            _png_chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)),
            _png_chunk(b"IDAT", zlib.compress(bytes(raw_rows))),
            _png_chunk(b"IEND", b""),
        ]
    )
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(payload)


def _float(value: object, default: float = 0.0) -> float:
    try:
        return float(value)
    except (TypeError, ValueError):
        return default


def _time_x(seconds: float, *, duration_s: float, width: int) -> int:
    return round(max(0.0, min(duration_s, seconds)) / duration_s * width)


def _span_layer(item: dict[str, Any], *, duration_s: float, width: int) -> dict[str, Any]:
    start_s = _float(item.get("start_s"))
    end_s = max(start_s, _float(item.get("end_s"), start_s))
    x = _time_x(start_s, duration_s=duration_s, width=width)
    x_end = _time_x(end_s, duration_s=duration_s, width=width)
    return {
        "x": x,
        "width": max(1, x_end - x),
        "start_s": start_s,
        "end_s": end_s,
        "text": str(item.get("text", "")).strip(),
    }


def build_composite_plan(spec: dict[str, Any], *, width: int = 960, height: int = 540) -> dict[str, Any]:
    duration_s = _float(spec.get("duration_s"))
    if duration_s <= 0:
        return {
            "status": "blocked",
            "message": "duration_s must be positive",
            "artifact": None,
            "word_count": 0,
            "silence_count": 0,
            "cut_count": 0,
            "layers": {"words": [], "silences": [], "cuts": []},
        }

    words = [_span_layer(word, duration_s=duration_s, width=width) for word in spec.get("words", [])]
    silences = [
        _span_layer(silence, duration_s=duration_s, width=width)
        for silence in spec.get("silences", [])
    ]
    cuts = [
        {"x": _time_x(_float(cut), duration_s=duration_s, width=width), "time_s": _float(cut)}
        for cut in spec.get("cut_points_s", [])
    ]
    return {
        "status": "ready",
        "message": "inspect generated timeline review composite before final report",
        "duration_s": duration_s,
        "width": width,
        "height": height,
        "source_label": str(spec.get("source_label", "source")),
        "output_label": str(spec.get("output_label", "output")),
        "word_count": len(words),
        "silence_count": len(silences),
        "cut_count": len(cuts),
        "layers": {"words": words, "silences": silences, "cuts": cuts},
    }


def _draw_frames(image: Image, *, y: int, height: int, color: Color, count: int = 8) -> None:
    width = len(image[0])
    gap = 5
    frame_w = max(12, (width - gap * (count + 1)) // count)
    for index in range(count):
        x = gap + index * (frame_w + gap)
        shade = tuple(max(0, min(255, channel - index * 5)) for channel in color)
        _rect(image, x=x, y=y, width=frame_w, height=height, color=shade)
        _rect(image, x=x + 4, y=y + 4, width=max(1, frame_w - 8), height=height - 8, color=PANEL)


def _draw_waveform(image: Image, *, y: int, height: int) -> None:
    width = len(image[0])
    mid = y + height // 2
    _rect(image, x=0, y=y, width=width, height=height, color=(20, 32, 42))
    for x in range(0, width, 6):
        pulse = 8 + int(((x * 37) % 41) / 40 * (height / 2 - 8))
        _rect(image, x=x, y=mid - pulse, width=3, height=pulse * 2, color=WAVE)


def _draw_plan(image: Image, plan: dict[str, Any]) -> None:
    width = int(plan["width"])
    _draw_text(image, f"SOURCE {plan['source_label']}", x=12, y=14, color=TEXT, scale=2)
    _draw_frames(image, y=42, height=68, color=SOURCE)
    _draw_text(image, f"OUTPUT {plan['output_label']}", x=12, y=122, color=TEXT, scale=2)
    _draw_frames(image, y=150, height=68, color=OUTPUT)
    _draw_text(image, "WAVEFORM", x=12, y=232, color=TEXT, scale=2)
    _draw_waveform(image, y=260, height=88)
    _draw_text(image, "WORDS", x=12, y=364, color=TEXT, scale=2)
    _rect(image, x=0, y=392, width=width, height=72, color=(28, 33, 42))

    for silence in plan["layers"]["silences"]:
        _rect(image, x=silence["x"], y=42, width=silence["width"], height=422, color=SILENCE)
    for word in plan["layers"]["words"]:
        _rect(image, x=word["x"], y=398, width=max(18, word["width"]), height=24, color=WORD)
        _draw_text(image, word["text"], x=word["x"] + 3, y=427, color=TEXT, scale=1)
    for cut in plan["layers"]["cuts"]:
        _rect(image, x=cut["x"], y=42, width=3, height=422, color=CUT)
        _draw_text(image, f"{cut['time_s']:.1f}", x=cut["x"] + 5, y=50, color=TEXT, scale=1)
    _draw_text(image, "SILENCE BANDS GRAY  CUTS RED", x=12, y=500, color=MUTED, scale=2)


def write_review_composite(
    spec: dict[str, Any],
    *,
    output_path: Path,
    width: int = 960,
    height: int = 540,
) -> dict[str, Any]:
    plan = build_composite_plan(spec, width=width, height=height)
    plan["artifact"] = str(output_path)
    if plan["status"] != "ready":
        return plan
    image = _blank(width, height, BACKGROUND)
    _draw_plan(image, plan)
    _write_png(output_path, image)
    sidecar_path = output_path.with_suffix(".json")
    sidecar_path.write_text(json.dumps(plan, indent=2) + "\n")
    plan["sidecar"] = str(sidecar_path)
    return plan


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--spec", required=True)
    parser.add_argument("--out", required=True)
    parser.add_argument("--width", type=int, default=960)
    parser.add_argument("--height", type=int, default=540)
    args = parser.parse_args()

    spec = json.loads(Path(args.spec).read_text())
    report = write_review_composite(
        spec,
        output_path=Path(args.out),
        width=args.width,
        height=args.height,
    )
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
