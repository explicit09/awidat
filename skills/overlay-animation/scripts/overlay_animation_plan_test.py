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

    def test_subject_aware_slots_record_layer_order_and_matte_requirements(self) -> None:
        planner = load_script("overlay_animation_plan")
        manifest = planner.build_animation_manifest(
            [
                {
                    "name": "Text Behind Speaker",
                    "anchor": "clip_uuid=clip-speaker",
                    "start_s": 1.2,
                    "duration_s": 2.0,
                    "engine": "canvas",
                    "mode": "full_frame",
                    "prompt": "Large title passes behind the speaker",
                    "subject_aware": True,
                    "subject_prompt": "main speaker",
                    "matte_path": "generated/mattes/speaker-alpha.webm",
                    "fallback": "render ordinary overlay above video if the matte is unavailable",
                }
            ],
            output_root=Path("generated/overlays"),
        )

        slot = manifest["slots"][0]

        self.assertTrue(slot["subject_aware"])
        self.assertEqual(slot["subject_prompt"], "main speaker")
        self.assertEqual(slot["matte_path"], "generated/mattes/speaker-alpha.webm")
        self.assertEqual(slot["layer_order"], ["base_video", "overlay_asset", "subject_matte"])
        self.assertIn("generated/mattes/speaker-alpha.webm", slot["brief"])
        self.assertIn("base video -> overlay asset -> subject matte", slot["brief"])
        self.assertIn("fallback", slot["brief"].lower())

    def test_subject_aware_slots_record_text_layers_detection_and_preview_evidence(self) -> None:
        planner = load_script("overlay_animation_plan")
        manifest = planner.build_animation_manifest(
            [
                {
                    "name": "Behind Product Title",
                    "anchor": "clip_uuid=clip-product",
                    "duration_s": 1.5,
                    "subject_aware": True,
                    "subject_prompt": "main product",
                    "text_layers": [
                        {
                            "text": "LAUNCH",
                            "font_family": "Inter",
                            "font_size": 148,
                            "font_color": "#F8F7F1",
                            "opacity": 0.92,
                            "rotation": -8,
                            "x": 0.5,
                            "y": 0.42,
                            "weight": "black",
                        }
                    ],
                    "detection": {
                        "object_classes": ["person", "product"],
                        "confidence_threshold": 0.55,
                        "iou_threshold": 0.7,
                        "mask_threshold": 0.3,
                        "preview_frame_path": "generated/previews/product-frame.png",
                        "occlusion_preview_path": "generated/previews/product-occlusion.png",
                    },
                }
            ],
            output_root=Path("generated/overlays"),
        )

        slot = manifest["slots"][0]

        self.assertEqual(slot["text_layers"][0]["text"], "LAUNCH")
        self.assertEqual(slot["text_layers"][0]["font_color"], "#F8F7F1")
        self.assertEqual(slot["text_layers"][0]["position"], {"x": 0.5, "y": 0.42})
        self.assertEqual(slot["detection"]["object_classes"], ["person", "product"])
        self.assertEqual(slot["detection"]["confidence_threshold"], 0.55)
        self.assertEqual(slot["detection"]["iou_threshold"], 0.7)
        self.assertEqual(slot["detection"]["mask_threshold"], 0.3)
        self.assertEqual(
            slot["detection"]["occlusion_preview_path"],
            "generated/previews/product-occlusion.png",
        )
        self.assertIn("Text layers:", slot["brief"])
        self.assertIn("LAUNCH", slot["brief"])
        self.assertIn("object classes: person, product", slot["brief"])

    def test_rejects_invalid_subject_aware_text_layer_values(self) -> None:
        planner = load_script("overlay_animation_plan")

        with self.assertRaisesRegex(ValueError, "opacity"):
            planner.build_animation_manifest(
                [
                    {
                        "name": "Bad Text",
                        "duration_s": 1.0,
                        "subject_aware": True,
                        "text_layers": [{"text": "Bad", "opacity": 1.4}],
                    }
                ],
                output_root=Path("generated/overlays"),
            )

        with self.assertRaisesRegex(ValueError, "mask_threshold"):
            planner.build_animation_manifest(
                [
                    {
                        "name": "Bad Detection",
                        "duration_s": 1.0,
                        "subject_aware": True,
                        "detection": {"mask_threshold": -0.1},
                    }
                ],
                output_root=Path("generated/overlays"),
            )


if __name__ == "__main__":
    unittest.main()
