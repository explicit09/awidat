#!/usr/bin/env python3
"""Tests for the generated-explainer source bundle helper."""

from __future__ import annotations

import json
import shutil
import subprocess
import tempfile
import unittest
from unittest.mock import patch
from pathlib import Path

import explainer_bundle


class ExplainerBundleTest(unittest.TestCase):
    def test_initialize_bundle_preserves_script_and_editable_layout(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            project_root = Path(temp_dir)

            bundle_dir = explainer_bundle.initialize_bundle(
                project_root=project_root,
                slug="complex-exponents",
                title="Why Complex Exponents Matter",
                script_text="# Opening\n\nRotation is change with direction.\n",
            )

            manifest = json.loads((bundle_dir / "manifest.json").read_text())
            self.assertEqual(manifest["schema_version"], 1)
            self.assertEqual(manifest["kind"], "montage.generated-explainer")
            self.assertEqual(manifest["slug"], "complex-exponents")
            self.assertEqual(manifest["script"], "script.md")
            self.assertIsNone(manifest["narration"])
            self.assertEqual(
                manifest["output_profile"],
                {
                    "name": "explainer-1440p60",
                    "aspect_ratio": "16:9",
                    "width": 2560,
                    "height": 1440,
                    "fps": 60.0,
                    "upscale_policy": "reject",
                },
            )
            self.assertEqual(manifest["scenes"], [])
            self.assertEqual(
                (bundle_dir / "script.md").read_text(),
                "# Opening\n\nRotation is change with direction.\n",
            )
            self.assertTrue((bundle_dir / "scenes").is_dir())
            self.assertTrue((bundle_dir / "renders").is_dir())

    def test_initialize_bundle_copies_narration_into_project_assets(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            project_root = Path(temp_dir)
            narration_path = project_root / "recording.wav"
            narration_path.write_bytes(b"RIFF-test-audio")

            bundle_dir = explainer_bundle.initialize_bundle(
                project_root=project_root,
                slug="narrated",
                title="Narrated Explainer",
                script_text="Show the idea while it is spoken.\n",
                narration_path=narration_path,
            )

            manifest = json.loads((bundle_dir / "manifest.json").read_text())
            self.assertEqual(manifest["narration"], "assets/narration.wav")
            self.assertEqual(
                (bundle_dir / manifest["narration"]).read_bytes(),
                b"RIFF-test-audio",
            )
            (bundle_dir / manifest["narration"]).unlink()
            self.assertEqual(
                explainer_bundle.verify_bundle(bundle_dir),
                ["missing narration: assets/narration.wav"],
            )

    def test_add_scene_creates_backend_source_and_stable_render_slot(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            bundle_dir = explainer_bundle.initialize_bundle(
                project_root=Path(temp_dir),
                slug="oscillator",
                title="The Harmonic Oscillator",
                script_text="A mass moves back toward equilibrium.\n",
            )

            scene = explainer_bundle.add_scene(
                bundle_dir=bundle_dir,
                scene_id="scene-001",
                title="Equilibrium",
                backend="manim",
                narration_start_s=0.0,
                narration_end_s=8.5,
            )

            self.assertEqual(scene["source"], "scenes/scene-001/scene.py")
            self.assertEqual(scene["render"], "renders/scene-001.mov")
            source = bundle_dir / scene["source"]
            source_text = source.read_text()
            self.assertIn("class GeneratedScene(Scene):", source_text)
            self.assertIn("config.pixel_width = 2560", source_text)
            self.assertIn("config.pixel_height = 1440", source_text)
            self.assertIn("config.frame_rate = 60.0", source_text)
            manifest = json.loads((bundle_dir / "manifest.json").read_text())
            self.assertEqual(manifest["scenes"], [scene])

    def test_add_scene_rejects_duplicate_ids_and_overlapping_narration(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            bundle_dir = explainer_bundle.initialize_bundle(
                project_root=Path(temp_dir),
                slug="waves",
                title="Waves",
                script_text="A wave carries a changing state.\n",
            )
            explainer_bundle.add_scene(
                bundle_dir=bundle_dir,
                scene_id="scene-001",
                title="Wave",
                backend="motion-scene",
                narration_start_s=0.0,
                narration_end_s=4.0,
            )

            with self.assertRaisesRegex(ValueError, "already exists"):
                explainer_bundle.add_scene(
                    bundle_dir=bundle_dir,
                    scene_id="scene-001",
                    title="Duplicate",
                    backend="manim",
                    narration_start_s=4.0,
                    narration_end_s=6.0,
                )
            with self.assertRaisesRegex(ValueError, "overlaps"):
                explainer_bundle.add_scene(
                    bundle_dir=bundle_dir,
                    scene_id="scene-002",
                    title="Overlap",
                    backend="manim",
                    narration_start_s=3.5,
                    narration_end_s=7.0,
                )

    def test_backend_contracts_only_require_external_render_slots_when_needed(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            bundle_dir = explainer_bundle.initialize_bundle(
                project_root=Path(temp_dir),
                slug="backend-routing",
                title="Backend Routing",
                script_text="Use the smallest adequate renderer.\n",
            )

            native = explainer_bundle.add_scene(
                bundle_dir=bundle_dir,
                scene_id="scene-001",
                title="Native callout",
                backend="motion-scene",
                narration_start_s=0.0,
                narration_end_s=2.0,
            )
            canvas = explainer_bundle.add_scene(
                bundle_dir=bundle_dir,
                scene_id="scene-002",
                title="Custom diagram",
                backend="motion-canvas",
                narration_start_s=2.0,
                narration_end_s=5.0,
            )

            self.assertIsNone(native["render"])
            native_source = json.loads((bundle_dir / native["source"]).read_text())
            self.assertEqual(native_source["id"], "scene-001")
            self.assertEqual(native_source["duration_s"], 2.0)
            self.assertEqual(native_source["fps"], 60.0)
            self.assertEqual(native_source["width"], 2560)
            self.assertEqual(native_source["height"], 1440)
            self.assertEqual(canvas["render"], "renders/scene-002.mov")
            self.assertIn("makeScene2D", (bundle_dir / canvas["source"]).read_text())
            self.assertEqual(
                explainer_bundle.verify_bundle(bundle_dir, require_renders=True),
                ["missing render: renders/scene-002.mov"],
            )

    def test_verify_bundle_requires_sources_and_optionally_renders(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            bundle_dir = explainer_bundle.initialize_bundle(
                project_root=Path(temp_dir),
                slug="vectors",
                title="Vectors",
                script_text="Vectors have direction and magnitude.\n",
            )
            scene = explainer_bundle.add_scene(
                bundle_dir=bundle_dir,
                scene_id="scene-001",
                title="Vector",
                backend="manim",
                narration_start_s=0.0,
                narration_end_s=3.0,
            )

            self.assertEqual(explainer_bundle.verify_bundle(bundle_dir), [])
            self.assertEqual(
                explainer_bundle.verify_bundle(bundle_dir, require_renders=True),
                ["missing render: renders/scene-001.mov"],
            )
            (bundle_dir / scene["source"]).unlink()
            self.assertEqual(
                explainer_bundle.verify_bundle(bundle_dir),
                ["missing source: scenes/scene-001/scene.py"],
            )

    def test_non_finite_ranges_are_rejected_before_writing(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            bundle = explainer_bundle.initialize_bundle(
                project_root=Path(temp_dir), slug="ranges", title="Ranges", script_text="Test")
            for start, end in [(float("nan"), 1), (0, float("nan")), (0, float("inf")), (-float("inf"), 1)]:
                with self.subTest(start=start, end=end), self.assertRaises(ValueError):
                    explainer_bundle.add_scene(bundle_dir=bundle, scene_id="scene", title="Scene",
                        backend="manim", narration_start_s=start, narration_end_s=end)
            self.assertEqual(list((bundle / "scenes").iterdir()), [])
            explainer_bundle.add_scene(bundle_dir=bundle, scene_id="scene", title="Scene",
                backend="manim", narration_start_s=0, narration_end_s=1)
            manifest = json.loads((bundle / "manifest.json").read_text())
            manifest["scenes"][0]["narration_end_s"] = float("nan")
            (bundle / "manifest.json").write_text(json.dumps(manifest))
            self.assertIn("invalid narration range: scene", explainer_bundle.verify_bundle(bundle))

    def test_external_renders_require_paths_and_full_duration(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            bundle = explainer_bundle.initialize_bundle(
                project_root=Path(temp_dir), slug="renders", title="Renders", script_text="Test")
            scene = explainer_bundle.add_scene(bundle_dir=bundle, scene_id="scene", title="Scene",
                backend="manim", narration_start_s=0, narration_end_s=2)
            manifest = json.loads((bundle / "manifest.json").read_text())
            for backend in ["manim", "motion-canvas"]:
                for render in [None, "", 42]:
                    manifest["scenes"][0].update(backend=backend, render=render)
                    (bundle / "manifest.json").write_text(json.dumps(manifest))
                    self.assertIn("missing render path: scene",
                        explainer_bundle.verify_bundle(bundle, require_renders=True))
            manifest["scenes"][0] = scene
            (bundle / "manifest.json").write_text(json.dumps(manifest))
            (bundle / scene["render"]).touch()
            with patch.object(explainer_bundle, "_probe_render", return_value=(2560, 1440, 60, 1)):
                self.assertEqual(explainer_bundle.verify_bundle(bundle, require_renders=True),
                    ["render duration below narration range: scene is 1.000s, requires at least 2.000s"])
            with patch.object(explainer_bundle, "_probe_render", return_value=(2560, 1440, 60, 2)):
                self.assertEqual(explainer_bundle.verify_bundle(bundle, require_renders=True), [])

    @unittest.skipUnless(shutil.which("ffmpeg") and shutil.which("ffprobe"), "ffmpeg required")
    def test_verify_bundle_rejects_render_below_output_profile(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            bundle_dir = explainer_bundle.initialize_bundle(
                project_root=Path(temp_dir),
                slug="quality-floor",
                title="Quality Floor",
                script_text="Keep generated diagrams crisp.\n",
            )
            scene = explainer_bundle.add_scene(
                bundle_dir=bundle_dir,
                scene_id="scene-001",
                title="Low resolution scene",
                backend="manim",
                narration_start_s=0.0,
                narration_end_s=1.0,
            )
            render_path = bundle_dir / scene["render"]
            subprocess.run(
                [
                    shutil.which("ffmpeg") or "ffmpeg",
                    "-y",
                    "-v",
                    "error",
                    "-f",
                    "lavfi",
                    "-i",
                    "color=size=1280x720:rate=30:duration=0.1",
                    "-frames:v",
                    "1",
                    str(render_path),
                ],
                check=True,
            )

            self.assertEqual(
                explainer_bundle.verify_bundle(bundle_dir, require_renders=True),
                [
                    "render resolution below profile: scene-001 is 1280x720, requires at least 2560x1440",
                    "render duration below narration range: scene-001 is 0.033s, requires at least 1.000s",
                    "render frame rate below profile: scene-001 is 30 fps, requires at least 60 fps",
                ],
            )


if __name__ == "__main__":
    unittest.main()
