from __future__ import annotations

import json
import sys
import tempfile
import unittest
from pathlib import Path


sys.path.insert(0, str(Path(__file__).resolve().parent))
import bench_clip_lifecycle as bench


def clip_sidecar(asset_id: str = "external/a.mp4") -> dict[str, object]:
    return {
        "indexer": "clip",
        "indexer_version": "0.1.0",
        "schema_version": "1",
        "asset_id": asset_id,
        "asset_sha256": "a" * 64,
        "produced_at": "2026-08-04T12:00:00+00:00",
        "data": {
            "model": "ViT-B-32/openai",
            "embedding_dim": 2,
            "embedding_dtype": "float16",
            "embedding_encoding": "base64",
            "frame_rate_sampled": 0.5,
            "duration_s": 2.0,
            "frame_count": 1,
            "timestamps_s": [0.0],
            "embeddings_b64": "ADwAQA==",
            "perf": {"model_load_ms": 12, "inference_ms": 3},
        },
    }


class BenchClipLifecycleTests(unittest.TestCase):
    def test_clip_sidecar_records_exact_float16_embedding_and_stable_metadata(self) -> None:
        record = bench.validate_clip_sidecar(clip_sidecar(), "external/a.mp4")

        self.assertEqual(
            record,
            {
                "asset_id": "external/a.mp4",
                "timestamps_s": [0.0],
                "frame_count": 1,
                "model": "ViT-B-32/openai",
                "embedding_dtype": "float16",
                "embedding_encoding": "base64",
                "embedding_dim": 2,
                "embedding_bytes_sha256": (
                    "6b60ada811679b7b8aeee509afcdd89fde521eef8c3686639eb8c923d109c639"
                ),
                "stable_semantic_metadata": {
                    "indexer": "clip",
                    "indexer_version": "0.1.0",
                    "schema_version": "1",
                    "asset_id": "external/a.mp4",
                    "asset_sha256": "a" * 64,
                    "model": "ViT-B-32/openai",
                    "embedding_dim": 2,
                    "embedding_dtype": "float16",
                    "embedding_encoding": "base64",
                    "frame_rate_sampled": 0.5,
                    "duration_s": 2.0,
                    "frame_count": 1,
                    "timestamps_s": [0.0],
                    "embedding_bytes_sha256": (
                        "6b60ada811679b7b8aeee509afcdd89fde521eef8c3686639eb8c923d109c639"
                    ),
                },
            },
        )

    def test_dispatcher_output_rejects_malformed_or_partial_six_asset_results(self) -> None:
        expected_ids = [f"external/{index}.mp4" for index in range(6)]
        with tempfile.TemporaryDirectory() as directory:
            output_root = Path(directory)
            malformed = output_root / "malformed-indexing-performance.json"
            malformed.write_text(json.dumps({"command": {}}), encoding="utf-8")
            with self.assertRaisesRegex(bench.BenchError, "malformed dispatcher report"):
                bench.validate_dispatcher_output(output_root, "malformed", expected_ids)

            partial = output_root / "partial-indexing-performance.json"
            partial.write_text(
                json.dumps(
                    {
                        "command": {
                            "included_indexers": ["clip"],
                            "concurrency": 2,
                            "assets": expected_ids,
                        },
                        "report": {
                            "pair_count": 5,
                            "wrote": 5,
                            "skipped": 0,
                            "failed": 0,
                            "dep_skipped": 0,
                            "pairs": [
                                {
                                    "indexer": "clip",
                                    "asset_id": asset_id,
                                    "outcome": "wrote",
                                }
                                for asset_id in expected_ids[:-1]
                            ],
                        },
                    }
                ),
                encoding="utf-8",
            )
            with self.assertRaisesRegex(bench.BenchError, "six CLIP writes"):
                bench.validate_dispatcher_output(output_root, "partial", expected_ids)

    def test_summary_uses_nearest_rank_p95_and_median_absolute_deviation(self) -> None:
        self.assertEqual(
            bench.summarize([10.0, 12.0, 11.0, 50.0, 13.0]),
            {"median": 12.0, "p95": 50.0, "mad": 1.0, "min": 10.0, "max": 50.0},
        )

    def test_sampler_reports_the_gap_between_adjacent_process_tree_samples(self) -> None:
        evidence = bench.sampler_evidence(
            "sample",
            0.1,
            [
                {"elapsed_ms": 0.0, "pids": [10], "process_count": 1, "rss_bytes": 1024},
                {"elapsed_ms": 25.0, "pids": [10, 11], "process_count": 2, "rss_bytes": 2048},
            ],
        )

        self.assertEqual(evidence["samples"], 2)
        self.assertEqual(evidence["gap_ms"]["max"], 25.0)

    def test_process_tree_rss_includes_descendants_but_not_unrelated_processes(self) -> None:
        pids, rss_bytes = bench.aggregate_process_tree(
            10,
            [(10, 1, 4), (11, 10, 8), (12, 11, 16), (13, 1, 32)],
        )

        self.assertEqual(pids, {10, 11, 12})
        self.assertEqual(rss_bytes, 28 * 1024)


if __name__ == "__main__":
    unittest.main()
