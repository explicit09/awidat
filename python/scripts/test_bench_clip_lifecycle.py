from __future__ import annotations

import hashlib
import json
import os
import signal
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path
from unittest import mock


sys.path.insert(0, str(Path(__file__).resolve().parent))
import bench_clip_lifecycle as bench


def clip_sidecar(
    asset_id: str = "external/a.mp4", asset_sha256: str = "a" * 64
) -> dict[str, object]:
    return {
        "indexer": "clip",
        "indexer_version": "0.1.0",
        "schema_version": "1",
        "asset_id": asset_id,
        "asset_sha256": asset_sha256,
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
        record = bench.validate_clip_sidecar(clip_sidecar(), "external/a.mp4", "a" * 64)

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
        fingerprints = {asset_id: "a" * 64 for asset_id in expected_ids}
        with tempfile.TemporaryDirectory() as directory:
            output_root = Path(directory)
            malformed = output_root / "malformed-indexing-performance.json"
            malformed.write_text(json.dumps({"command": {}}), encoding="utf-8")
            with self.assertRaisesRegex(bench.BenchError, "malformed dispatcher report"):
                bench.validate_dispatcher_output(output_root, "malformed", expected_ids, fingerprints)

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
                bench.validate_dispatcher_output(output_root, "partial", expected_ids, fingerprints)

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

    def test_model_provenance_derives_hf_home_without_replacing_home(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            hf_home = Path(directory) / "huggingface"
            blob = hf_home / "hub/models--timm--vit_base_patch32_clip_224.openai/blobs/test-blob"
            blob.parent.mkdir(parents=True)
            blob.write_bytes(b"pinned weights")
            snapshot = (
                hf_home
                / "hub/models--timm--vit_base_patch32_clip_224.openai/snapshots"
                / "a6f597a30f7b82c51704746581f9a4e41421e878/open_clip_model.safetensors"
            )
            snapshot.parent.mkdir(parents=True)
            snapshot.symlink_to("../../blobs/test-blob")
            digest = hashlib.sha256(b"pinned weights").hexdigest()

            with mock.patch.object(bench, "MODEL_SHA256", digest):
                weights, derived_hf_home, provenance = bench.model_provenance(snapshot)
            with mock.patch.dict(os.environ, {"HOME": "/preserve-this-home"}, clear=False):
                environment = bench.controlled_environment(
                    sample_root=Path(directory) / "sample",
                    python_root=Path(directory) / "python",
                    hf_home=derived_hf_home,
                    uv=Path(directory) / "uv",
                    ffmpeg=Path(directory) / "ffmpeg",
                    ffprobe=Path(directory) / "ffprobe",
                )

        self.assertEqual(weights, snapshot)
        self.assertEqual(derived_hf_home, hf_home)
        self.assertEqual(provenance["snapshot_path"], str(snapshot))
        self.assertEqual(provenance["resolved_blob"]["sha256"], digest)
        self.assertEqual(environment["HF_HOME"], str(hf_home))
        self.assertEqual(environment["HOME"], "/preserve-this-home")

    def test_asset_fingerprint_matches_the_rust_metadata_algorithm_and_binds_sidecar(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            asset = Path(directory) / "clip-fixture.mp4"
            asset.write_bytes(b"clip fixture")
            os.utime(asset, ns=(1_700_000_000_123_456_789, 1_700_000_000_123_456_789))

            fingerprint = bench.asset_fingerprint(asset)
            self.assertEqual(
                fingerprint,
                "b5c8849787067a0ee5c049eb456f7dfbe7a0e8bfd0680b1cbe8b3b7c8165f289",
            )
            bench.validate_clip_sidecar(clip_sidecar(asset_sha256=fingerprint), "external/a.mp4", fingerprint)
            with self.assertRaisesRegex(bench.BenchError, "asset fingerprint"):
                bench.validate_clip_sidecar(clip_sidecar(), "external/a.mp4", fingerprint)
            pre_epoch = Path(directory) / "pre-epoch.mp4"
            pre_epoch.write_bytes(b"x")
            os.utime(pre_epoch, ns=(-1, -1))
            self.assertEqual(
                bench.asset_fingerprint(pre_epoch),
                "6225f9979a3c51a55215b48867e90d1628058db1d913e2cc9fa4441825a285b3",
            )

    def test_runtime_preflight_rejects_a_dependency_resolved_outside_the_supplied_venv(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            python_root = Path(directory) / "python"
            clip_module = python_root / "packages/clip-mcp/src/clip_mcp/__init__.py"
            montage_module = python_root / "packages/montage-mcp/src/montage_mcp/__init__.py"
            for module in (clip_module, montage_module):
                module.parent.mkdir(parents=True, exist_ok=True)
                module.write_text("", encoding="utf-8")
            venv_root = python_root / ".venv"
            package_paths = {
                "clip-mcp": clip_module,
                "montage-mcp": montage_module,
                "open-clip-torch": venv_root / "lib/python3.11/site-packages/open_clip/__init__.py",
                "torch": venv_root / "lib/python3.11/site-packages/torch/__init__.py",
                "torchvision": venv_root / "lib/python3.11/site-packages/torchvision/__init__.py",
                "numpy": venv_root / "lib/python3.11/site-packages/numpy/__init__.py",
            }
            for path in package_paths.values():
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text("", encoding="utf-8")
            launcher = venv_root / "bin/python"
            launcher.parent.mkdir(parents=True, exist_ok=True)
            launcher.symlink_to(Path(sys.executable))
            self.assertEqual(bench.python_launcher(python_root), launcher)
            weights = Path(directory) / "open_clip_model.safetensors"
            weights.write_bytes(b"offline model")
            digest = hashlib.sha256(b"offline model").hexdigest()
            payload = {
                "python": "3.11.0",
                "executable": str(launcher),
                "packages": {
                    name: {"version": "1.0", "module_path": str(path)}
                    for name, path in package_paths.items()
                },
                "pretrained": {
                    "hf_hub": "timm/vit_base_patch32_clip_224.openai/",
                    "resolved_path": str(weights),
                    "sha256": digest,
                },
            }
            with mock.patch.object(bench, "MODEL_SHA256", digest):
                result = bench.parse_runtime_preflight(
                    json.dumps(payload), python_root, weights
                )
                self.assertEqual(
                    result["packages"]["clip-mcp"]["path"], str(clip_module.resolve())
                )
                self.assertEqual(result["offline_artifact_resolver"]["sha256"], digest)
                outside = Path(directory) / "outside-torch.py"
                outside.write_text("", encoding="utf-8")
                payload["packages"]["torch"]["module_path"] = str(outside)
                with self.assertRaisesRegex(bench.BenchError, "outside supplied .venv"):
                    bench.parse_runtime_preflight(
                        json.dumps(payload), python_root, weights
                    )

    def test_process_group_cleanup_polls_and_terminates_only_the_dispatcher_group(self) -> None:
        process = subprocess.Popen(
            [sys.executable, "-c", "import time; time.sleep(60)"], start_new_session=True
        )
        try:
            members: list[tuple[int, int, int, int]] = []
            for _ in range(50):
                members = bench.process_group_members(process.pid)
                if any(member[0] == process.pid for member in members):
                    break
                time.sleep(0.02)
            self.assertIn(process.pid, {member[0] for member in members})
            self.assertEqual(bench.terminate_process_group(process.pid, process), [])
            self.assertIsNotNone(process.wait(timeout=3))
            self.assertEqual(bench.process_group_members(process.pid), [])
        finally:
            if process.poll() is None:
                os.killpg(process.pid, signal.SIGKILL)
                process.wait(timeout=3)

    def test_dispatcher_output_retains_pair_and_sidecar_timing_attribution(self) -> None:
        expected_ids = [f"external/{index}.mp4" for index in range(6)]
        fingerprints = {asset_id: f"{index:064x}" for index, asset_id in enumerate(expected_ids)}
        with tempfile.TemporaryDirectory() as directory:
            output_root = Path(directory)
            pairs = [
                {
                    "indexer": "clip",
                    "asset_id": asset_id,
                    "outcome": "wrote",
                    "queued_ms": 1,
                    "launch_init_ms": 2,
                    "tool_ms": 3,
                    "write_ms": 4,
                    "total_ms": 10,
                    "peak_rss_bytes": 4096,
                    "sidecar": {"perf": {"model_load_ms": 5, "inference_ms": 6}},
                }
                for asset_id in expected_ids
            ]
            (output_root / "complete-indexing-performance.json").write_text(
                json.dumps(
                    {
                        "command": {
                            "included_indexers": ["clip"],
                            "concurrency": 2,
                            "assets": expected_ids,
                        },
                        "report": {
                            "pair_count": 6,
                            "wrote": 6,
                            "skipped": 0,
                            "failed": 0,
                            "dep_skipped": 0,
                            "pairs": pairs,
                        },
                    }
                ),
                encoding="utf-8",
            )
            for asset_id in expected_ids:
                sidecar = output_root / "index-run-test/clip" / f"{asset_id}.json"
                sidecar.parent.mkdir(parents=True, exist_ok=True)
                sidecar.write_text(
                    json.dumps(clip_sidecar(asset_id, fingerprints[asset_id])), encoding="utf-8"
                )

            _records, dispatcher = bench.validate_dispatcher_output(
                output_root, "complete", expected_ids, fingerprints
            )

        self.assertEqual(
            dispatcher["pair_telemetry"][0],
            {
                "asset_id": "external/0.mp4",
                "queued_ms": 1,
                "launch_init_ms": 2,
                "tool_ms": 3,
                "write_ms": 4,
                "total_ms": 10,
                "direct_peak_rss_bytes": 4096,
                "sidecar_perf_ms": {"model_load_ms": 5, "inference_ms": 6},
            },
        )


if __name__ == "__main__":
    unittest.main()
