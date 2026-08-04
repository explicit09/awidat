from __future__ import annotations

import json
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import bench_audio_energy as bench


class BenchAudioEnergyTests(unittest.TestCase):
    def test_process_table_parser_ignores_headers_and_malformed_rows(self) -> None:
        rows = bench.parse_ps_table(
            " PID PPID RSS\n"
            " 41 1 1024\n"
            " 42 41 2048\n"
            " missing fields\n"
            " 43 42 nope\n"
        )

        self.assertEqual(rows, [(41, 1, 1024), (42, 41, 2048)])

    def test_recursive_tree_rss_excludes_unrelated_processes(self) -> None:
        pids, rss_bytes = bench.aggregate_process_tree(
            10,
            [
                (10, 1, 4),
                (11, 10, 8),
                (12, 11, 16),
                (13, 1, 32),
            ],
        )

        self.assertEqual(pids, {10, 11, 12})
        self.assertEqual(rss_bytes, 28 * 1024)

    def test_df_parser_preserves_mount_paths_with_spaces(self) -> None:
        device, mount = bench.parse_df_posix(
            "Filesystem 1024-blocks Used Available Capacity Mounted on\n"
            "/dev/disk9s1 1000 400 600 40% /Volumes/My Passport for Mac\n"
        )

        self.assertEqual(device, "/dev/disk9s1")
        self.assertEqual(mount, "/Volumes/My Passport for Mac")

    def test_summary_uses_nearest_rank_p95_and_median_absolute_deviation(self) -> None:
        self.assertEqual(
            bench.summarize([10.0, 12.0, 11.0, 50.0, 13.0]),
            {
                "median": 12.0,
                "p95": 50.0,
                "mad": 1.0,
                "min": 10.0,
                "max": 50.0,
            },
        )

    def test_canonical_data_hash_is_stable_across_mapping_order(self) -> None:
        canonical, digest = bench.canonical_data({"b": [2, 1], "a": 1.25})

        self.assertEqual(canonical, b'{"a":1.25,"b":[2,1]}')
        self.assertEqual(
            digest,
            "843dc42a984a1c7f307f2d0b00159e385a1db0b7edf56989dcc00590fb8126fc",
        )

    def test_audio_temp_leak_check_ignores_uv_locks(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "uv-cache.lock").write_text("", encoding="utf-8")
            leaked = root / "montage-audio-leaked.f32"
            leaked.write_bytes(b"pcm")

            self.assertEqual(bench.find_leaked_audio_temp_files(root), [str(leaked)])

    def test_zero_temp_high_water_is_a_valid_streaming_result(self) -> None:
        bench.validate_sample_observations(
            "candidate",
            sampler_count=1,
            peak_rss_bytes=1024,
            temp_high_water_bytes=0,
        )

    def test_audio_runtime_provenance_resolves_the_executed_environment(self) -> None:
        runtime = bench.parse_audio_runtime_provenance(
            json.dumps(
                {
                    "python": sys.version,
                    "executable": sys.executable,
                    "module": bench.__file__,
                    "numpy": "2.4.4",
                    "scipy": "1.17.1",
                    "pyloudnorm": "0.2.0",
                }
            ),
            Path(bench.__file__).resolve(),
        )

        self.assertEqual(runtime["python_version"], sys.version)
        self.assertEqual(runtime["module"]["path"], str(Path(bench.__file__).resolve()))
        self.assertEqual(
            runtime["packages"],
            {"numpy": "2.4.4", "scipy": "1.17.1", "pyloudnorm": "0.2.0"},
        )
        self.assertEqual(runtime["executable"]["path"], str(Path(sys.executable).resolve()))


if __name__ == "__main__":
    unittest.main()
