#!/usr/bin/env python3
"""Contract tests for the optional Motion Canvas template."""

from __future__ import annotations

import json
import unittest
from pathlib import Path


SKILL_ROOT = Path(__file__).resolve().parents[1]
TEMPLATE_ROOT = SKILL_ROOT / "motion-canvas" / "template"


class MotionCanvasTemplateTests(unittest.TestCase):
    def test_template_has_opt_in_project_scaffold(self) -> None:
        package_json = TEMPLATE_ROOT / "package.json"
        self.assertTrue(package_json.is_file(), "missing Motion Canvas package.json")

        package = json.loads(package_json.read_text())
        self.assertTrue(package["private"])
        self.assertEqual(package["type"], "module")
        self.assertEqual(package["scripts"]["serve"], "vite --host 127.0.0.1 --port 9000")
        self.assertEqual(package["scripts"]["start"], "npm run serve")
        self.assertIn("export:frames", package["scripts"])
        self.assertEqual(package["dependencies"]["@motion-canvas/core"], "^3.17.2")
        self.assertEqual(package["dependencies"]["@motion-canvas/2d"], "^3.17.2")
        self.assertEqual(package["devDependencies"]["@motion-canvas/vite-plugin"], "^3.17.2")

    def test_template_uses_motion_canvas_scene_contract(self) -> None:
        project = (TEMPLATE_ROOT / "src" / "project.ts").read_text()
        scene = (TEMPLATE_ROOT / "src" / "scenes" / "brand-card.tsx").read_text()
        vite_config = (TEMPLATE_ROOT / "vite.config.ts").read_text()

        self.assertIn("makeProject", project)
        self.assertIn("./scenes/brand-card?scene", project)
        self.assertIn("motionCanvasPlugin.default ?? motionCanvasPlugin", vite_config)
        self.assertIn("motionCanvas()", vite_config)
        self.assertIn("makeScene2D", scene)
        self.assertIn("#C8A84E", scene)
        self.assertIn("#070D17", scene)
        self.assertIn("#F2EDE3", scene)
        self.assertIn("waitFor", scene)

    def test_docs_describe_optional_setup_and_montage_handoff(self) -> None:
        skill = (SKILL_ROOT / "SKILL.md").read_text()
        readme = (SKILL_ROOT / "motion-canvas" / "README.md").read_text()

        self.assertIn("Motion Canvas optional template", skill)
        self.assertNotIn("Motion Canvas — deferred", skill)
        self.assertIn("npm install", readme)
        self.assertIn("npm run serve", readme)
        self.assertIn("export:frames", readme)
        self.assertIn("generated/drawn/<slug>.mov", readme)
        self.assertIn("Insert PiP", readme)


if __name__ == "__main__":
    unittest.main()
