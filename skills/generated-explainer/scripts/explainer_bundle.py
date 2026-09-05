#!/usr/bin/env python3
"""Create and verify editable source bundles for generated explainers."""

from __future__ import annotations

import argparse
import json
import math
import re
import shutil
import subprocess
import sys
from pathlib import Path
from typing import Any

SCHEMA_VERSION = 1
KIND = "montage.generated-explainer"
BACKENDS = ("motion-scene", "manim", "motion-canvas")
OUTPUT_PROFILES = {
    "standard-1080p30": {
        "name": "standard-1080p30",
        "aspect_ratio": "16:9",
        "width": 1920,
        "height": 1080,
        "fps": 30.0,
        "upscale_policy": "reject",
    },
    "explainer-1440p60": {
        "name": "explainer-1440p60",
        "aspect_ratio": "16:9",
        "width": 2560,
        "height": 1440,
        "fps": 60.0,
        "upscale_policy": "reject",
    },
    "vertical-1080p60": {
        "name": "vertical-1080p60",
        "aspect_ratio": "9:16",
        "width": 1080,
        "height": 1920,
        "fps": 60.0,
        "upscale_policy": "reject",
    },
}
DEFAULT_OUTPUT_PROFILE = "explainer-1440p60"
SLUG_RE = re.compile(r"^[a-z0-9]+(?:-[a-z0-9]+)*$")


def _require_slug(value: str, label: str) -> None:
    if not SLUG_RE.fullmatch(value):
        raise ValueError(f"{label} must be lowercase kebab-case")


def _write_json(path: Path, value: Any) -> None:
    temporary = path.with_suffix(f"{path.suffix}.tmp")
    temporary.write_text(f"{json.dumps(value, indent=2)}\n", encoding="utf-8")
    temporary.replace(path)


def _read_manifest(bundle_dir: Path) -> dict[str, Any]:
    path = bundle_dir / "manifest.json"
    try:
        manifest = json.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError as error:
        raise ValueError(f"missing explainer manifest: {path}") from error
    if manifest.get("schema_version") != SCHEMA_VERSION or manifest.get("kind") != KIND:
        raise ValueError(f"unsupported explainer manifest: {path}")
    return manifest


def initialize_bundle(
    *,
    project_root: Path,
    slug: str,
    title: str,
    script_text: str,
    narration_path: Path | None = None,
    output_profile: str = DEFAULT_OUTPUT_PROFILE,
) -> Path:
    """Create a generated explainer bundle without overwriting prior work."""
    _require_slug(slug, "slug")
    if not title.strip():
        raise ValueError("title must not be empty")
    if not script_text.strip():
        raise ValueError("script must not be empty")
    if not project_root.is_dir():
        raise ValueError(f"project root does not exist: {project_root}")
    if narration_path is not None and not narration_path.is_file():
        raise ValueError(f"narration file does not exist: {narration_path}")
    if output_profile not in OUTPUT_PROFILES:
        raise ValueError(f"unknown output profile: {output_profile}")

    bundle_dir = project_root / "generated" / "explainers" / slug
    if (bundle_dir / "manifest.json").exists():
        raise ValueError(f"explainer bundle already exists: {bundle_dir}")
    (bundle_dir / "scenes").mkdir(parents=True, exist_ok=True)
    (bundle_dir / "renders").mkdir(exist_ok=True)
    (bundle_dir / "assets").mkdir(exist_ok=True)
    (bundle_dir / "script.md").write_text(script_text, encoding="utf-8")
    narration = None
    if narration_path is not None:
        suffix = narration_path.suffix.lower()
        if not suffix:
            raise ValueError("narration file must have an extension")
        narration = f"assets/narration{suffix}"
        shutil.copyfile(narration_path, bundle_dir / narration)
    _write_json(
        bundle_dir / "manifest.json",
        {
            "schema_version": SCHEMA_VERSION,
            "kind": KIND,
            "slug": slug,
            "title": title.strip(),
            "script": "script.md",
            "narration": narration,
            "output_profile": OUTPUT_PROFILES[output_profile],
            "scenes": [],
        },
    )
    return bundle_dir


def _source_for(backend: str, scene_id: str) -> tuple[str, str | None, str]:
    if backend == "manim":
        return (
            f"scenes/{scene_id}/scene.py",
            f"renders/{scene_id}.mov",
            """from manim import Scene, Text, config

config.pixel_width = {width}
config.pixel_height = {height}
config.frame_rate = {fps}

class GeneratedScene(Scene):
    def construct(self) -> None:
        # Replace this scaffold with the scene's explanatory animation.
        self.add(Text({title!r}))
""",
        )
    if backend == "motion-scene":
        return (f"scenes/{scene_id}/motion-scene.json", None, "")
    if backend == "motion-canvas":
        return (
            f"scenes/{scene_id}/scene.tsx",
            f"renders/{scene_id}.mov",
            """import {makeScene2D} from '@motion-canvas/2d';

export default makeScene2D(function* () {
  // Replace this scaffold with the scene's explanatory animation.
});
""",
        )
    raise ValueError(f"backend must be one of: {', '.join(BACKENDS)}")


def add_scene(
    *,
    bundle_dir: Path,
    scene_id: str,
    title: str,
    backend: str,
    narration_start_s: float,
    narration_end_s: float,
) -> dict[str, Any]:
    """Add one ordered scene and scaffold its editable backend source."""
    _require_slug(scene_id, "scene id")
    if not title.strip():
        raise ValueError("scene title must not be empty")
    if backend not in BACKENDS:
        raise ValueError(f"backend must be one of: {', '.join(BACKENDS)}")
    if (not math.isfinite(narration_start_s) or not math.isfinite(narration_end_s)
            or narration_start_s < 0 or narration_end_s <= narration_start_s):
        raise ValueError("narration range must have non-negative start and end > start")

    manifest = _read_manifest(bundle_dir)
    profile = manifest.get("output_profile")
    if not isinstance(profile, dict):
        raise ValueError("manifest output_profile must be an object")
    scenes = manifest.get("scenes")
    if not isinstance(scenes, list):
        raise ValueError("manifest scenes must be a list")
    if any(scene.get("id") == scene_id for scene in scenes):
        raise ValueError(f"scene id already exists: {scene_id}")
    for scene in scenes:
        start = float(scene["narration_start_s"])
        end = float(scene["narration_end_s"])
        if narration_start_s < end and narration_end_s > start:
            raise ValueError(f"scene narration overlaps existing scene: {scene['id']}")

    source, render, template = _source_for(backend, scene_id)
    duration_s = narration_end_s - narration_start_s
    source_path = bundle_dir / source
    source_path.parent.mkdir(parents=True, exist_ok=False)
    if backend == "motion-scene":
        _write_json(
            source_path,
            {
                "id": scene_id,
                "duration_s": duration_s,
                "fps": profile["fps"],
                "width": profile["width"],
                "height": profile["height"],
                "layers": [],
            },
        )
    else:
        source_text = (
            template.format(
                title=title.strip(),
                width=profile["width"],
                height=profile["height"],
                fps=profile["fps"],
            )
            if backend == "manim"
            else template
        )
        source_path.write_text(source_text, encoding="utf-8")

    scene = {
        "id": scene_id,
        "title": title.strip(),
        "backend": backend,
        "narration_start_s": narration_start_s,
        "narration_end_s": narration_end_s,
        "source": source,
        "render": render,
    }
    scenes.append(scene)
    scenes.sort(key=lambda item: (item["narration_start_s"], item["id"]))
    _write_json(bundle_dir / "manifest.json", manifest)
    return scene


def _probe_render(path: Path) -> tuple[int, int, float, float]:
    try:
        result = subprocess.run(
            [
                "ffprobe",
                "-v",
                "error",
                "-select_streams",
                "v:0",
                "-show_entries",
                "stream=width,height,avg_frame_rate,duration",
                "-of",
                "json",
                str(path),
            ],
            check=True,
            capture_output=True,
            text=True,
        )
    except (FileNotFoundError, subprocess.CalledProcessError) as error:
        raise ValueError(f"unable to probe render: {path}") from error
    streams = json.loads(result.stdout).get("streams", [])
    if not streams:
        raise ValueError(f"render has no video stream: {path}")
    stream = streams[0]
    numerator, denominator = str(stream["avg_frame_rate"]).split("/", 1)
    fps = float(numerator) / float(denominator)
    duration = float(stream["duration"])
    if not math.isfinite(duration) or duration <= 0:
        raise ValueError(f"render has no finite positive duration: {path}")
    return int(stream["width"]), int(stream["height"]), fps, duration


def _format_fps(value: float) -> str:
    return str(int(value)) if value.is_integer() else f"{value:.3f}".rstrip("0").rstrip(".")


def verify_bundle(bundle_dir: Path, *, require_renders: bool = False) -> list[str]:
    """Return deterministic source/render contract violations."""
    try:
        manifest = _read_manifest(bundle_dir)
    except (ValueError, json.JSONDecodeError) as error:
        return [str(error)]

    issues: list[str] = []
    script = manifest.get("script")
    if not isinstance(script, str) or not (bundle_dir / script).is_file():
        issues.append(f"missing script: {script}")
    narration = manifest.get("narration")
    if isinstance(narration, str) and not (bundle_dir / narration).is_file():
        issues.append(f"missing narration: {narration}")
    profile = manifest.get("output_profile")
    if not isinstance(profile, dict):
        issues.append("missing output profile")
        profile = None
    previous_end = 0.0
    for scene in manifest.get("scenes", []):
        source = scene.get("source")
        if not isinstance(source, str) or not (bundle_dir / source).is_file():
            issues.append(f"missing source: {source}")
        start = float(scene.get("narration_start_s", -1.0))
        end = float(scene.get("narration_end_s", -1.0))
        if start < previous_end:
            issues.append(f"overlapping narration: {scene.get('id')}")
        if not math.isfinite(start) or not math.isfinite(end) or start < 0 or end <= start:
            issues.append(f"invalid narration range: {scene.get('id')}")
        previous_end = max(previous_end, end)
        render = scene.get("render")
        if require_renders and scene.get("backend") != "motion-scene" and (
            not isinstance(render, str) or not render.strip()
        ):
            issues.append(f"missing render path: {scene.get('id')}")
        if require_renders and isinstance(render, str) and render.strip():
            render_path = bundle_dir / render
            if not render_path.is_file():
                issues.append(f"missing render: {render}")
            elif profile is not None:
                try:
                    width, height, fps, duration = _probe_render(render_path)
                except (ValueError, json.JSONDecodeError, KeyError, ZeroDivisionError) as error:
                    issues.append(str(error))
                    continue
                required_width = int(profile["width"])
                required_height = int(profile["height"])
                required_fps = float(profile["fps"])
                if width < required_width or height < required_height:
                    issues.append(
                        f"render resolution below profile: {scene.get('id')} is {width}x{height}, "
                        f"requires at least {required_width}x{required_height}"
                    )
                if duration + 0.001 < end - start:
                    issues.append(
                        f"render duration below narration range: {scene.get('id')} is {duration:.3f}s, "
                        f"requires at least {end - start:.3f}s"
                    )
                if fps + 0.01 < required_fps:
                    issues.append(
                        f"render frame rate below profile: {scene.get('id')} is {_format_fps(fps)} fps, "
                        f"requires at least {_format_fps(required_fps)} fps"
                    )
    return issues


def _build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)

    init = subparsers.add_parser("init", help="initialize an explainer bundle")
    init.add_argument("--project-root", type=Path, required=True)
    init.add_argument("--slug", required=True)
    init.add_argument("--title", required=True)
    init.add_argument("--script-file", type=Path, required=True)
    init.add_argument("--narration-file", type=Path)
    init.add_argument(
        "--profile",
        choices=OUTPUT_PROFILES,
        default=DEFAULT_OUTPUT_PROFILE,
    )

    scene = subparsers.add_parser("add-scene", help="add an editable scene source")
    scene.add_argument("--bundle", type=Path, required=True)
    scene.add_argument("--id", required=True)
    scene.add_argument("--title", required=True)
    scene.add_argument("--backend", choices=BACKENDS, required=True)
    scene.add_argument("--start", type=float, required=True)
    scene.add_argument("--end", type=float, required=True)

    verify = subparsers.add_parser("verify", help="verify the bundle contract")
    verify.add_argument("--bundle", type=Path, required=True)
    verify.add_argument("--require-renders", action="store_true")
    return parser


def main() -> int:
    args = _build_parser().parse_args()
    try:
        if args.command == "init":
            result = initialize_bundle(
                project_root=args.project_root,
                slug=args.slug,
                title=args.title,
                script_text=args.script_file.read_text(encoding="utf-8"),
                narration_path=args.narration_file,
                output_profile=args.profile,
            )
            print(result)
        elif args.command == "add-scene":
            result = add_scene(
                bundle_dir=args.bundle,
                scene_id=args.id,
                title=args.title,
                backend=args.backend,
                narration_start_s=args.start,
                narration_end_s=args.end,
            )
            print(json.dumps(result, indent=2))
        else:
            issues = verify_bundle(args.bundle, require_renders=args.require_renders)
            if issues:
                print("\n".join(issues), file=sys.stderr)
                return 1
            print("explainer bundle verified")
    except (OSError, ValueError, json.JSONDecodeError) as error:
        print(error, file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
