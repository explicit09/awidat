#!/usr/bin/env python3
"""Tests for generated overlay animation planning."""

from __future__ import annotations

import importlib.util
import sys
import unittest
from pathlib import Path


SCRIPT_DIR = Path(__file__).resolve().parent
sys.path.insert(0, str(SCRIPT_DIR))


def load_script(name: str):
    spec = importlib.util.spec_from_file_location(name, SCRIPT_DIR / f"{name}.py")
    if spec is None or spec.loader is None:
        raise RuntimeError(f"could not load {name}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class OverlayAnimationPlanTests(unittest.TestCase):
    def test_manifest_assigns_stable_paths_and_insert_pip_hint(self) -> None:
        planner = load_script("overlay_animation_plan")
        manifest = planner.build_animation_manifest(
            [
                {
                    "name": "Launch Callout!",
                    "anchor": "clip_uuid=clip-a",
                    "start_s": 2.0,
                    "duration_s": 1.8,
                    "engine": "remotion",
                    "mode": "full_frame",
                    "prompt": "Animated product callout",
                }
            ],
            output_root=Path("generated/overlays"),
        )

        slot = manifest["slots"][0]
        self.assertEqual(slot["slug"], "launch-callout")
        self.assertEqual(slot["asset_path"], "generated/overlays/launch-callout/overlay.webm")
        self.assertEqual(slot["brief_path"], "generated/overlays/launch-callout/brief.md")
        self.assertIn("*** Insert PiP", slot["edl_hint"])
        self.assertIn("+ asset: generated/overlays/launch-callout/overlay.webm", slot["edl_hint"])
        self.assertIn("+ duration_s: 1.8", slot["edl_hint"])

    def test_build_brief_names_duration_engine_and_delivery_contract(self) -> None:
        planner = load_script("overlay_animation_plan")
        slot = planner.normalize_slot(
            {
                "name": "Stat Burst",
                "anchor": "clip_uuid=clip-b",
                "start_s": 4.0,
                "duration_s": 2.5,
                "engine": "canvas",
                "mode": "pip",
                "prompt": "Three metric cards pop in",
            },
            output_root=Path("generated/overlays"),
        )

        brief = planner.build_slot_brief(slot)

        self.assertIn("# Stat Burst", brief)
        self.assertIn("Duration: 2.500s", brief)
        self.assertIn("Engine: canvas", brief)
        self.assertIn("Deliver: generated/overlays/stat-burst/overlay.webm", brief)
        self.assertIn("transparent or keyed background", brief)


if __name__ == "__main__":
    unittest.main()
