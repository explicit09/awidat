#!/usr/bin/env python3
"""Generate a visual review image around a rendered cut boundary."""

from __future__ import annotations

import argparse
import shutil
import subprocess
from pathlib import Path


def window_start(center_s: float, window_s: float) -> float:
    return max(0.0, center_s - window_s / 2.0)


def build_ffmpeg_args(
    *,
    input_path: Path,
    output_path: Path,
    center_s: float,
    window_s: float = 3.0,
    frame_count: int = 6,
    width: int = 1280,
    wave_height: int = 160,
    has_audio: bool = True,
) -> list[str]:
    start = window_start(center_s, window_s)
    fps = max(1.0, frame_count / max(0.001, window_s))
    thumb_w = max(1, width // max(1, frame_count))
    film = (
        f"[0:v]trim=start={start:.3f}:duration={window_s:.3f},"
        f"setpts=PTS-STARTPTS,fps={fps:.3f},scale={thumb_w}:-1,"
        f"tile={frame_count}x1:padding=4:margin=4,scale={width}:-1[film]"
    )
    filters = [film]
    map_label = "[film]"
    if has_audio:
        wave = (
            f"[0:a]atrim=start={start:.3f}:duration={window_s:.3f},"
            f"asetpts=PTS-STARTPTS,showwavespic=s={width}x{wave_height}:"
            "colors=#44CCFF[wave]"
        )
        filters.append(wave)
        filters.append("[film][wave]vstack=inputs=2[out]")
        map_label = "[out]"

    return [
        "ffmpeg",
        "-y",
        "-v",
        "error",
        "-i",
        str(input_path),
        "-filter_complex",
        ";".join(filters),
        "-map",
        map_label,
        "-frames:v",
        "1",
        str(output_path),
    ]


def render_cut_window(
    *,
    input_path: Path,
    output_path: Path,
    center_s: float,
    window_s: float = 3.0,
    has_audio: bool = True,
) -> dict:
    ffmpeg = shutil.which("ffmpeg")
    if not ffmpeg:
        return {"ok": False, "error": "ffmpeg not found", "file": str(output_path)}
    output_path.parent.mkdir(parents=True, exist_ok=True)
    args = build_ffmpeg_args(
        input_path=input_path,
        output_path=output_path,
        center_s=center_s,
        window_s=window_s,
        has_audio=has_audio,
    )
    args[0] = ffmpeg
    proc = subprocess.run(
        args,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if proc.returncode != 0:
        return {"ok": False, "error": proc.stderr.strip(), "file": str(output_path)}
    return {"ok": True, "file": str(output_path)}


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--file", required=True)
    parser.add_argument("--center-s", required=True, type=float)
    parser.add_argument("--out", required=True)
    parser.add_argument("--window-s", type=float, default=3.0)
    parser.add_argument("--no-audio", action="store_true")
    args = parser.parse_args()

    result = render_cut_window(
        input_path=Path(args.file),
        output_path=Path(args.out),
        center_s=args.center_s,
        window_s=args.window_s,
        has_audio=not args.no_audio,
    )
    if not result["ok"]:
        raise SystemExit(result["error"])


if __name__ == "__main__":
    main()
