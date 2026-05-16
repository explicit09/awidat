import json
import tempfile
import unittest
from pathlib import Path

from composition_model_from_sidecars import generate_for_asset


class CompositionModelFromSidecarsTests(unittest.TestCase):
    def test_generate_for_asset_writes_model_sidecar_and_refreshes_handoff(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            asset_id = "raw/example.mov"
            asset_path = root / asset_id
            asset_path.parent.mkdir(parents=True)
            asset_path.write_bytes(b"fake")

            self._write_sidecar(
                root,
                "composition",
                asset_id,
                {
                    "regions": [
                        {
                            "start_s": 0.0,
                            "end_s": 2.0,
                            "composition_source": "heuristic:composition-v1",
                            "composition_confidence": 0.7,
                            "subject_role": "primary_speaker",
                            "depth_layer": "foreground",
                            "framing": "single_close",
                        }
                    ]
                },
            )
            self._write_sidecar(
                root,
                "scenedetect",
                asset_id,
                {"shots": [{"index": 0, "start_s": 0.0, "end_s": 2.0}]},
            )
            self._write_sidecar(root, "face", asset_id, {"per_frame": []})
            self._write_sidecar(root, "gaze", asset_id, {"per_frame": []})
            self._write_sidecar(
                root,
                "clip",
                asset_id,
                {
                    "frame_count": 2,
                    "embedding_dim": 2,
                    "embedding_dtype": "f16",
                    "embedding_encoding": "base64",
                    "embeddings_b64": "ADwAAAA8",
                    "timestamps_s": [0.0, 3.0],
                },
            )
            self._write_sidecar(
                root,
                "shot",
                asset_id,
                {
                    "shots": [
                        {
                            "index": 0,
                            "start_s": 0.0,
                            "end_s": 2.0,
                            "composition_source": "heuristic:composition-v1",
                            "composition_confidence": 0.7,
                            "subject_role": "primary_speaker",
                            "depth_layer": "foreground",
                            "framing": "single_close",
                            "match_candidates": [],
                        }
                    ],
                    "depends_on": ["composition"],
                },
            )

            summary = generate_for_asset(root, asset_id, refresh_handoff=True)

            self.assertEqual(summary.model_regions, 1)
            self.assertEqual(summary.model_shots, 1)
            model = self._read_sidecar(root, "composition-model", asset_id)
            region = model["data"]["regions"][0]
            self.assertEqual(region["composition_source"], "model:face-gaze-clip-composition-v1")
            self.assertEqual(region["subject_role"], "primary_speaker")
            self.assertGreaterEqual(region["composition_confidence"], 0.0)
            self.assertLessEqual(region["composition_confidence"], 1.0)

            composition = self._read_sidecar(root, "composition", asset_id)
            self.assertEqual(
                composition["data"]["regions"][0]["composition_source"],
                "model:face-gaze-clip-composition-v1",
            )
            self.assertEqual(
                composition["data"]["regions"][0]["heuristic_composition_source"],
                "heuristic:composition-v1",
            )

            shot = self._read_sidecar(root, "shot", asset_id)
            self.assertEqual(
                shot["data"]["shots"][0]["composition_source"],
                "model:face-gaze-clip-composition-v1",
            )

    def _write_sidecar(
        self, root: Path, indexer: str, asset_id: str, data: dict
    ) -> None:
        path = root / "index" / indexer / f"{asset_id}.json"
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(
            json.dumps(
                {
                    "indexer": indexer,
                    "indexer_version": "test",
                    "schema_version": "1",
                    "asset_id": asset_id,
                    "asset_sha256": "abc123",
                    "produced_at": "2026-05-16T00:00:00Z",
                    "data": data,
                }
            )
            + "\n"
        )

    def _read_sidecar(self, root: Path, indexer: str, asset_id: str) -> dict:
        return json.loads((root / "index" / indexer / f"{asset_id}.json").read_text())


if __name__ == "__main__":
    unittest.main()
