#!/usr/bin/env python3
"""Create a before/after contact sheet from rendered videos.

This is a verification helper, not an editor. It reads final rendered
artifacts and writes a simple binary PPM sheet so it does not need Pillow.
The script always exits 0 and reports failures in JSON.
"""

from __future__ import annotations

import argparse
import json
import shutil
import subprocess
from pathlib import Path


def probe_size(path: Path) -> tuple[int, int]:
    proc = subprocess.run(
        [
            "ffprobe",
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height",
            "-of",
            "csv=p=0",
            str(path),
        ],
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if proc.returncode != 0:
        raise RuntimeError(proc.stderr.strip())
    w, h = (int(part) for part in proc.stdout.strip().split(","))
    return w, h


def scaled_height(path: Path, width: int) -> int:
    src_w, src_h = probe_size(path)
    h = int(round(src_h * width / src_w))
    return h + (h % 2)


def extract_rgb(path: Path, t_s: float, width: int, height: int) -> bytes:
    proc = subprocess.run(
        [
            "ffmpeg",
            "-nostdin",
            "-v",
            "error",
            "-ss",
            f"{t_s:.3f}",
            "-i",
            str(path),
            "-frames:v",
            "1",
            "-vf",
            f"scale={width}:{height}",
            "-pix_fmt",
            "rgb24",
            "-f",
            "rawvideo",
            "-",
        ],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    expected = width * height * 3
    if proc.returncode != 0:
        raise RuntimeError(proc.stderr.decode("utf-8", errors="replace").strip())
    if len(proc.stdout) != expected:
        raise RuntimeError(f"expected {expected} RGB bytes, got {len(proc.stdout)}")
    return proc.stdout


def paste(canvas: bytearray, canvas_w: int, tile: bytes, tile_w: int, tile_h: int, x: int, y: int) -> None:
    row_bytes = tile_w * 3
    canvas_row_bytes = canvas_w * 3
    for row in range(tile_h):
        src_start = row * row_bytes
        dst_start = ((y + row) * canvas_row_bytes) + (x * 3)
        canvas[dst_start : dst_start + row_bytes] = tile[src_start : src_start + row_bytes]


def make_sheet(before: Path, after: Path, output: Path, times: list[float], width: int) -> dict:
    before_h = scaled_height(before, width)
    after_h = scaled_height(after, width)
    tile_h = max(before_h, after_h)
    sheet_w = width * 2
    sheet_h = tile_h * len(times)
    canvas = bytearray([24, 24, 24] * sheet_w * sheet_h)

    for idx, t_s in enumerate(times):
        row_y = idx * tile_h
        before_rgb = extract_rgb(before, t_s, width, before_h)
        after_rgb = extract_rgb(after, t_s, width, after_h)
        paste(canvas, sheet_w, before_rgb, width, before_h, 0, row_y)
        paste(canvas, sheet_w, after_rgb, width, after_h, width, row_y)

    output.parent.mkdir(parents=True, exist_ok=True)
    with output.open("wb") as fh:
        fh.write(f"P6\n{sheet_w} {sheet_h}\n255\n".encode("ascii"))
        fh.write(canvas)
    return {"ok": True, "output": str(output), "width": sheet_w, "height": sheet_h, "times_s": times}


def parse_times(raw: str) -> list[float]:
    return [float(part.strip()) for part in raw.split(",") if part.strip()]


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--before-render", required=True)
    parser.add_argument("--after-render", required=True)
    parser.add_argument("--output", required=True)
    parser.add_argument("--times", default="0,5,10")
    parser.add_argument("--tile-width", type=int, default=320)
    args = parser.parse_args()

    before = Path(args.before_render)
    after = Path(args.after_render)
    errors = []
    if not before.exists():
        errors.append(f"before render not found: {before}")
    if not after.exists():
        errors.append(f"after render not found: {after}")
    if not shutil.which("ffmpeg") or not shutil.which("ffprobe"):
        errors.append("ffmpeg/ffprobe not found")

    result = None
    if not errors:
        try:
            result = make_sheet(before, after, Path(args.output), parse_times(args.times), args.tile_width)
        except Exception as exc:  # noqa: BLE001 - report as JSON for agent repair loops.
            errors.append(str(exc))

    print(
        json.dumps(
            {
                "status": "passed" if not errors else "failed",
                "contact_sheet": result,
                "errors": errors,
            },
            indent=2,
        )
    )


if __name__ == "__main__":
    main()
