#!/usr/bin/env python3
"""Tests for generated overlay asset verification."""

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


class OverlayAssetVerifyTests(unittest.TestCase):
    def test_ready_when_overlay_and_subject_matte_exist(self) -> None:
        verifier = load_script("overlay_asset_verify")
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "generated/overlays/title").mkdir(parents=True)
            (root / "generated/mattes").mkdir(parents=True)
            (root / "generated/overlays/title/overlay.webm").write_bytes(b"asset")
            (root / "generated/mattes/speaker.webm").write_bytes(b"matte")
            manifest = {
                "slots": [
                    {
                        "name": "Title Behind Speaker",
                        "duration_s": 2.0,
                        "asset_path": "generated/overlays/title/overlay.webm",
                        "edl_hint": "*** Begin EDL\n*** Insert PiP\n+ asset: generated/overlays/title/overlay.webm\n+ duration_s: 2.0\n*** End EDL",
                        "subject_aware": True,
                        "matte_path": "generated/mattes/speaker.webm",
                    }
                ]
            }

            report = verifier.verify_manifest(manifest, project_root=root)

        self.assertEqual(report["status"], "ready")
        self.assertEqual(report["issue_count"], 0)
        self.assertEqual(report["slots"][0]["status"], "ready")

    def test_missing_overlay_asset_blocks_insert(self) -> None:
        verifier = load_script("overlay_asset_verify")
        with tempfile.TemporaryDirectory() as tmp:
            manifest = {
                "slots": [
                    {
                        "name": "Missing Overlay",
                        "duration_s": 1.5,
                        "asset_path": "generated/overlays/missing/overlay.webm",
                    }
                ]
            }

            report = verifier.verify_manifest(manifest, project_root=Path(tmp))

        self.assertEqual(report["status"], "blocked")
        self.assertIn("missing_overlay_asset", {issue["code"] for issue in report["issues"]})

    def test_edl_hint_must_match_asset_and_duration_contract(self) -> None:
        verifier = load_script("overlay_asset_verify")
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "generated/overlays/title").mkdir(parents=True)
            (root / "generated/overlays/title/overlay.webm").write_bytes(b"asset")
            manifest = {
                "slots": [
                    {
                        "name": "Mismatched Hint",
                        "duration_s": 1.5,
                        "asset_path": "generated/overlays/title/overlay.webm",
                        "edl_hint": "*** Begin EDL\n*** Insert PiP\n+ asset: generated/overlays/other/overlay.webm\n+ duration_s: 2.0\n*** End EDL",
                    }
                ]
            }

            report = verifier.verify_manifest(manifest, project_root=root)

        codes = {issue["code"] for issue in report["issues"]}
        self.assertEqual(report["status"], "blocked")
        self.assertIn("edl_asset_mismatch", codes)
        self.assertIn("edl_duration_mismatch", codes)

    def test_subject_aware_slot_requires_existing_matte(self) -> None:
        verifier = load_script("overlay_asset_verify")
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "generated/overlays/title").mkdir(parents=True)
            (root / "generated/overlays/title/overlay.webm").write_bytes(b"asset")
            manifest = {
                "slots": [
                    {
                        "name": "Missing Matte",
                        "duration_s": 1.5,
                        "asset_path": "generated/overlays/title/overlay.webm",
                        "subject_aware": True,
                        "matte_path": "generated/mattes/missing.webm",
                    }
                ]
            }

            report = verifier.verify_manifest(manifest, project_root=root)

        self.assertEqual(report["status"], "blocked")
        self.assertIn("missing_subject_matte", {issue["code"] for issue in report["issues"]})

    def test_rejects_absolute_and_parent_traversal_paths(self) -> None:
        verifier = load_script("overlay_asset_verify")
        with tempfile.TemporaryDirectory() as tmp:
            manifest = {
                "slots": [
                    {
                        "name": "Unsafe Paths",
                        "duration_s": 1.0,
                        "asset_path": "/tmp/overlay.webm",
                        "subject_aware": True,
                        "matte_path": "../matte.webm",
                    }
                ]
            }

            report = verifier.verify_manifest(manifest, project_root=Path(tmp))

        codes = {issue["code"] for issue in report["issues"]}
        self.assertEqual(report["status"], "blocked")
        self.assertIn("unsafe_overlay_path", codes)
        self.assertIn("unsafe_matte_path", codes)


if __name__ == "__main__":
    unittest.main()
