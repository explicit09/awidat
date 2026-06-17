#!/usr/bin/env python3
# /// script
# requires-python = ">=3.10"
# dependencies = ["lottie[all]"]
# ///
"""Render a Lottie .json animation to a transparent PNG sequence and
assemble it into an alpha .mov with ffmpeg.

Run via uv:

    uv run --with 'lottie[all]' scripts/lottie_render.py \
        --input asset.json --out generated/drawn/asset.mov

Options:
    --frames-dir DIR   keep the PNG sequence here (default: temp dir,
                       deleted after assembly; pass a dir to keep frames
                       for MotionScene image-layer use)
    --fps N            override the animation's native fps
    --scale F          uniform scale factor (default 1.0)
    --codec            prores4444 (default) | qtrle
    --png-only         skip the .mov, just emit the PNG sequence

Sourcing assets: lottiefiles.com hosts free Lottie JSON animations.
Each asset carries its own license (Lottie Simple License for free
assets) — the USER must verify the license terms per asset before it
lands in a published edit. Download the "Lottie JSON" format, not
dotLottie (.lottie); if you only have a .lottie file, unzip it and use
the animations/*.json inside.
"""

from __future__ import annotations

import argparse
import json
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

FFMPEG = shutil.which("ffmpeg") or "/opt/homebrew/bin/ffmpeg"

CODEC_ARGS = {
    "prores4444": ["-c:v", "prores_ks", "-profile:v", "4444",
                   "-pix_fmt", "yuva444p10le"],
    "qtrle": ["-c:v", "qtrle"],
}


def _fail(msg: str) -> None:
    print(f"lottie_render.py: {msg}", file=sys.stderr)
    sys.exit(1)


def load_animation(path: Path):
    from lottie.parsers.tgs import parse_tgs

    if not path.is_file():
        _fail(f"input not found: {path}")
    try:
        return parse_tgs(str(path))
    except Exception as exc:  # noqa: BLE001 - surface parser detail
        _fail(f"could not parse Lottie file {path}: {exc}")


def render_frames(anim, frames_dir: Path, fps: float, scale: float) -> int:
    from lottie.exporters.cairo import export_png

    frames_dir.mkdir(parents=True, exist_ok=True)
    native_fps = float(anim.frame_rate)
    start, end = float(anim.in_point), float(anim.out_point)
    total = max(1, round((end - start) * fps / native_fps))
    dpi = 96 * scale
    for i in range(total):
        anim_frame = start + i * native_fps / fps
        anim_frame = min(anim_frame, end)
        out = frames_dir / f"frame_{i:05d}.png"
        with open(out, "wb") as fh:
            export_png(anim, fh, frame=anim_frame, dpi=dpi)
    return total


def assemble_mov(frames_dir: Path, out_path: Path, fps: float, codec: str) -> None:
    out_path.parent.mkdir(parents=True, exist_ok=True)
    cmd = [
        FFMPEG, "-y", "-v", "error",
        "-framerate", str(fps),
        "-i", str(frames_dir / "frame_%05d.png"),
        *CODEC_ARGS[codec],
        str(out_path),
    ]
    result = subprocess.run(cmd, capture_output=True, text=True)
    if result.returncode != 0:
        _fail(f"ffmpeg failed: {result.stderr.strip()}")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--input", required=True, help="Lottie .json file")
    parser.add_argument("--out", help=".mov output path (required unless --png-only)")
    parser.add_argument("--frames-dir", help="keep PNG sequence here")
    parser.add_argument("--fps", type=float, help="override native fps")
    parser.add_argument("--scale", type=float, default=1.0)
    parser.add_argument("--codec", choices=sorted(CODEC_ARGS), default="prores4444")
    parser.add_argument("--png-only", action="store_true")
    args = parser.parse_args()

    if not args.png_only and not args.out:
        _fail("--out is required unless --png-only")
    if args.png_only and not args.frames_dir:
        _fail("--png-only requires --frames-dir")

    anim = load_animation(Path(args.input))
    fps = args.fps or float(anim.frame_rate)

    temp_dir = None
    if args.frames_dir:
        frames_dir = Path(args.frames_dir)
    else:
        temp_dir = tempfile.mkdtemp(prefix="lottie_frames_")
        frames_dir = Path(temp_dir)

    try:
        total = render_frames(anim, frames_dir, fps, args.scale)
        report = {
            "status": "ok",
            "frames": total,
            "fps": fps,
            "duration_s": round(total / fps, 3),
        }
        if not args.png_only:
            assemble_mov(frames_dir, Path(args.out), fps, args.codec)
            report["output"] = args.out
            report["codec"] = args.codec
        if args.frames_dir:
            report["frames_dir"] = str(frames_dir)
        print(json.dumps(report))
    finally:
        if temp_dir:
            shutil.rmtree(temp_dir, ignore_errors=True)


if __name__ == "__main__":
    main()
