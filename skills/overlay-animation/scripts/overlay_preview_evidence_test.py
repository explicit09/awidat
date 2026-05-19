#!/usr/bin/env python3
"""Tests for subject-aware overlay preview evidence."""

from __future__ import annotations

import importlib.util
import sys
import tempfile
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


class OverlayPreviewEvidenceTests(unittest.TestCase):
    def test_subject_safe_preview_outputs_before_after_and_scorecard(self) -> None:
        preview = load_script("overlay_preview_evidence")
        manifest = {
            "slots": [
                {
                    "name": "Text Behind Speaker",
                    "slug": "text-behind-speaker",
                    "subject_aware": True,
                    "text_layers": [
                        {
                            "text": "LAUNCH",
                            "font_color": "#F8F7F1",
                            "opacity": 1.0,
                            "position": {"x": 0.5, "y": 0.48},
                        }
                    ],
                }
            ]
        }
        fixture = {
            "slots": {
                "text-behind-speaker": {
                    "frame": {"width": 320, "height": 180, "background": "#263238"},
                    "subject": {
                        "bbox": {"x": 120, "y": 38, "width": 82, "height": 112},
                        "color": "#D8B38A",
                    },
                }
            }
        }

        with tempfile.TemporaryDirectory() as tmp:
            report = preview.build_preview_report(
                manifest,
                fixture=fixture,
                output_root=Path(tmp) / "generated/previews",
                project_root=Path(tmp),
            )

            slot = report["slots"][0]
            self.assertEqual(report["status"], "ready")
            self.assertEqual(slot["status"], "ready")
            self.assertEqual(slot["evidence_kind"], "subject_safe_overlay_preview")
            self.assertGreater(slot["ordinary_subject_overlap_pixels"], 0)
            self.assertEqual(slot["safe_subject_overlap_pixels"], 0)
            self.assertTrue(slot["subject_occludes_overlay"])

            for key in ("ordinary_preview_path", "subject_safe_preview_path", "scorecard_path"):
                path = Path(tmp) / slot[key]
                self.assertTrue(path.is_file(), key)
                if path.suffix == ".png":
                    self.assertEqual(path.read_bytes()[:8], b"\x89PNG\r\n\x1a\n")

    def test_subject_aware_preview_requires_subject_bounds(self) -> None:
        preview = load_script("overlay_preview_evidence")
        manifest = {
            "slots": [
                {
                    "name": "Missing Bounds",
                    "slug": "missing-bounds",
                    "subject_aware": True,
                    "text_layers": [{"text": "TITLE", "position": {"x": 0.5, "y": 0.5}}],
                }
            ]
        }

        with tempfile.TemporaryDirectory() as tmp:
            report = preview.build_preview_report(
                manifest,
                fixture={"slots": {"missing-bounds": {}}},
                output_root=Path(tmp) / "generated/previews",
                project_root=Path(tmp),
            )

        self.assertEqual(report["status"], "blocked")
        self.assertEqual(report["issue_count"], 1)
        self.assertEqual(report["issues"][0]["code"], "missing_subject_bounds")


if __name__ == "__main__":
    unittest.main()
