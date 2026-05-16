import json
import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import smoke_safe


def write_model_sidecar(project_root: Path, asset_id: str, confidence: float = 0.91) -> None:
    path = project_root / "index" / "composition-model" / f"{asset_id}.json"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        json.dumps(
            {
                "indexer": "composition-model",
                "indexer_version": "0.1.0",
                "schema_version": "1",
                "asset_id": asset_id,
                "asset_sha256": "0" * 64,
                "produced_at": "2026-05-16T00:00:00+00:00",
                "data": {
                    "regions": [
                        {
                            "start_s": 0.0,
                            "end_s": 2.0,
                            "composition_source": "model:composition-v2",
                            "composition_confidence": confidence,
                            "subject_role": "primary_speaker",
                            "depth_layer": "foreground",
                            "framing": "single_close",
                        }
                    ]
                },
            }
        )
    )


def write_project_marker(project_root: Path) -> None:
    project_root.mkdir(parents=True, exist_ok=True)
    (project_root / "project.otio.json").write_text("{}")


class CompositionModelProjectTreeTests(unittest.TestCase):
    def test_safe_smoke_checks_eval_workflow_contract(self) -> None:
        smoke_safe.check_eval_workflow_contract()

    def test_project_tree_counts_valid_model_regions(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            project_root = Path(tmp)
            write_model_sidecar(project_root, "raw/a.mp4")
            write_model_sidecar(project_root, "raw/nested/b.mp4")

            files, regions, invalid_regions = smoke_safe.validate_composition_model_project_tree(
                project_root, min_regions=2
            )

            self.assertEqual(files, 2)
            self.assertEqual(regions, 2)
            self.assertEqual(invalid_regions, 0)

    def test_project_tree_rejects_invalid_model_sidecars(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            project_root = Path(tmp)
            write_model_sidecar(project_root, "raw/a.mp4", confidence=1.5)

            with self.assertRaisesRegex(ValueError, "composition_confidence"):
                smoke_safe.validate_composition_model_project_tree(project_root)

    def test_project_tree_rejects_unknown_model_label_values(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            project_root = Path(tmp)
            write_model_sidecar(project_root, "raw/a.mp4")
            path = project_root / "index" / "composition-model" / "raw/a.mp4.json"
            sidecar = json.loads(path.read_text())
            sidecar["data"]["regions"][0]["subject_role"] = "banana"
            path.write_text(json.dumps(sidecar))

            with self.assertRaisesRegex(ValueError, "subject_role"):
                smoke_safe.validate_composition_model_project_tree(project_root)

    def test_project_tree_error_summarizes_valid_and_invalid_regions(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            project_root = Path(tmp)
            write_model_sidecar(project_root, "raw/valid.mp4")
            write_model_sidecar(project_root, "raw/invalid.mp4", confidence=1.5)

            with self.assertRaisesRegex(
                ValueError, "1 valid model region\\(s\\), 1 invalid model region\\(s\\)"
            ):
                smoke_safe.validate_composition_model_project_tree(project_root)

    def test_project_tree_allows_invalid_regions_when_explicitly_relaxed(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            project_root = Path(tmp)
            write_model_sidecar(project_root, "raw/valid.mp4")
            write_model_sidecar(project_root, "raw/invalid.mp4", confidence=1.5)

            result = smoke_safe.validate_composition_model_project_tree(
                project_root, min_regions=1, max_invalid_regions=1
            )

            self.assertEqual(result, (2, 1, 1))

    def test_env_project_tree_check_uses_min_region_threshold(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            project_root = Path(tmp)
            write_model_sidecar(project_root, "raw/a.mp4")

            with patch.dict(
                os.environ,
                {
                    "AWIDAT_COMPOSITION_MODEL_PROJECT": str(project_root),
                    "AWIDAT_COMPOSITION_MODEL_MIN_REGIONS": "1",
                },
                clear=False,
            ):
                result = smoke_safe.check_composition_model_project_tree_from_env()

            self.assertEqual(result, (1, 1, 0))

    def test_env_project_tree_check_requires_project_when_region_threshold_is_set(self) -> None:
        with patch.dict(
            os.environ,
            {
                "AWIDAT_COMPOSITION_MODEL_PROJECT": "",
                "AWIDAT_COMPOSITION_MODEL_MIN_REGIONS": "2",
            },
            clear=False,
        ):
            with self.assertRaisesRegex(SystemExit, "AWIDAT_COMPOSITION_MODEL_PROJECT"):
                smoke_safe.check_composition_model_project_tree_from_env()

    def test_env_project_tree_check_treats_empty_optional_thresholds_as_defaults(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            project_root = Path(tmp)
            write_model_sidecar(project_root, "raw/a.mp4")

            with patch.dict(
                os.environ,
                {
                    "AWIDAT_COMPOSITION_MODEL_PROJECT": str(project_root),
                    "AWIDAT_COMPOSITION_MODEL_MIN_REGIONS": "",
                    "AWIDAT_COMPOSITION_MODEL_MAX_INVALID_REGIONS": "",
                },
                clear=False,
            ):
                result = smoke_safe.check_composition_model_project_tree_from_env()

            self.assertEqual(result, (1, 1, 0))

    def test_env_project_tree_check_uses_real_visual_fallback_when_primary_is_blank(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            project_root = Path(tmp)
            write_model_sidecar(project_root, "raw/valid.mp4")
            write_model_sidecar(project_root, "raw/invalid.mp4", confidence=1.5)

            with patch.dict(
                os.environ,
                {
                    "AWIDAT_COMPOSITION_MODEL_PROJECT": str(project_root),
                    "AWIDAT_COMPOSITION_MODEL_MIN_REGIONS": "1",
                    "AWIDAT_COMPOSITION_MODEL_MAX_INVALID_REGIONS": "",
                    "AWIDAT_REAL_VISUAL_MAX_INVALID_COMPOSITION_MODEL_REGIONS": "1",
                },
                clear=False,
            ):
                result = smoke_safe.check_composition_model_project_tree_from_env()

            self.assertEqual(result, (2, 1, 1))

    def test_env_project_tree_check_uses_real_visual_min_region_fallback(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            project_root = Path(tmp)
            write_model_sidecar(project_root, "raw/a.mp4")

            with patch.dict(
                os.environ,
                {
                    "AWIDAT_COMPOSITION_MODEL_PROJECT": str(project_root),
                    "AWIDAT_COMPOSITION_MODEL_MIN_REGIONS": "",
                    "AWIDAT_REAL_VISUAL_MIN_COMPOSITION_MODEL_REGIONS": "2",
                },
                clear=False,
            ):
                with self.assertRaisesRegex(SystemExit, "below required minimum 2"):
                    smoke_safe.check_composition_model_project_tree_from_env()

    def test_env_project_tree_check_uses_real_corpus_as_project_fallback(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            project_root = Path(tmp)
            write_model_sidecar(project_root, "raw/a.mp4")

            with patch.dict(
                os.environ,
                {
                    "AWIDAT_REAL_CORPUS": str(project_root),
                    "AWIDAT_REAL_VISUAL_MIN_COMPOSITION_MODEL_REGIONS": "1",
                },
                clear=True,
            ):
                result = smoke_safe.check_composition_model_project_tree_from_env()

            self.assertEqual(result, (1, 1, 0))

    def test_env_project_tree_check_ignores_real_corpus_when_min_region_gate_is_disabled(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            project_root = Path(tmp)

            with patch.dict(
                os.environ,
                {
                    "AWIDAT_REAL_CORPUS": str(project_root),
                    "AWIDAT_REAL_VISUAL_MIN_COMPOSITION_MODEL_REGIONS": "0",
                },
                clear=True,
            ):
                result = smoke_safe.check_composition_model_project_tree_from_env()

            self.assertIsNone(result)

    def test_real_corpus_fixture_gate_fails_when_undercovered(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            project_root = Path(tmp)
            write_project_marker(project_root)

            with patch.dict(
                os.environ,
                {
                    "AWIDAT_REAL_CORPUS": str(project_root),
                    "AWIDAT_REAL_MIN_TRANSITION_PLANNER_FIXTURES": "1",
                },
                clear=True,
            ):
                with self.assertRaisesRegex(
                    SystemExit, "expected at least 1 transition-planner fixture"
                ):
                    smoke_safe.check_real_corpus_fixture_gates_from_env()

    def test_real_corpus_fixture_gate_counts_default_and_directory_fixtures(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            project_root = Path(tmp)
            write_project_marker(project_root)
            eval_root = project_root / ".awidat" / "eval"
            eval_root.mkdir(parents=True)
            (eval_root / "transition-planner-flow.json").write_text("{}")
            directory = eval_root / "transition-planners"
            directory.mkdir()
            (directory / "sample.json").write_text("{}")
            (directory / "notes.txt").write_text("ignore")

            with patch.dict(
                os.environ,
                {
                    "AWIDAT_REAL_CORPUS": str(project_root),
                    "AWIDAT_REAL_MIN_TRANSITION_PLANNER_FIXTURES": "2",
                },
                clear=True,
            ):
                counts = smoke_safe.check_real_corpus_fixture_gates_from_env()

            self.assertEqual(counts["transition-planner"], 2)

    def test_real_corpus_fixture_gate_counts_json_extensions_case_insensitively(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            project_root = Path(tmp)
            write_project_marker(project_root)
            directory = project_root / ".awidat" / "eval" / "transition-planners"
            directory.mkdir(parents=True)
            (directory / "sample.JSON").write_text("{}")

            with patch.dict(
                os.environ,
                {
                    "AWIDAT_REAL_CORPUS": str(project_root),
                    "AWIDAT_REAL_MIN_TRANSITION_PLANNER_FIXTURES": "1",
                },
                clear=True,
            ):
                counts = smoke_safe.check_real_corpus_fixture_gates_from_env()

            self.assertEqual(counts["transition-planner"], 1)


if __name__ == "__main__":
    unittest.main()
