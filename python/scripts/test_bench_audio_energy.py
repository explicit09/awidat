from __future__ import annotations

import json
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import bench_audio_energy as bench


def audio_energy_data() -> dict[str, object]:
    return {
        "sample_rate": 48_000,
        "duration_s": 2.0,
        "window_ms": 100,
        "windows": [{"start_s": 0.0, "rms_db": -12.0}],
        "loudness_integrated_lufs": -18.0,
        "true_peak_dbfs": -1.0,
        "loudness_short_term": [{"start_s": 0.0, "lufs": -18.0}],
        "silences": [{"start_s": 0.5, "end_s": 1.0}],
        "silence_relative_lu": -30.0,
    }


def write_dispatcher_output(
    output_root: Path,
    data: dict[str, object],
    *,
    pair_overrides: dict[str, object] | None = None,
) -> None:
    pair: dict[str, object] = {
        "indexer": "audio-energy",
        "outcome": "wrote",
        "tool_ms": 10,
        "total_ms": 12,
        "peak_rss_bytes": 4096,
    }
    pair.update(pair_overrides or {})
    (output_root / "sample-indexing-performance.json").write_text(
        json.dumps(
            {
                "command": {"included_indexers": ["audio-energy"]},
                "report": {
                    "pair_count": 1,
                    "wrote": 1,
                    "failed": 0,
                    "dep_skipped": 0,
                    "pairs": [pair],
                },
            }
        ),
        encoding="utf-8",
    )
    sidecar = output_root / "index-run-test/audio-energy/external/fixture.m4a.json"
    sidecar.parent.mkdir(parents=True)
    sidecar.write_text(
        json.dumps({"indexer": "audio-energy", "data": data}),
        encoding="utf-8",
    )


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

    def test_canonical_data_reports_nonfinite_json_as_a_benchmark_error(self) -> None:
        with self.assertRaisesRegex(bench.BenchError, "canonical JSON"):
            bench.canonical_data({"value": float("nan")})

    def test_dispatcher_output_rejects_malformed_audio_numeric_fields(self) -> None:
        cases = {
            "boolean duration": lambda data: data.__setitem__("duration_s", True),
            "boolean peak": lambda data: data.__setitem__("true_peak_dbfs", True),
            "nonfinite loudness": lambda data: data.__setitem__(
                "loudness_integrated_lufs", float("inf")
            ),
            "boolean window size": lambda data: data.__setitem__("window_ms", True),
            "non-object window": lambda data: data.__setitem__("windows", [True]),
            "nonfinite window value": lambda data: data.__setitem__(
                "windows", [{"start_s": 0.0, "rms_db": float("nan")}]
            ),
            "nonfinite short-term value": lambda data: data.__setitem__(
                "loudness_short_term",
                [{"start_s": 0.0, "lufs": float("inf")}],
            ),
            "nonfinite silence value": lambda data: data.__setitem__(
                "silences", [{"start_s": 0.5, "end_s": float("nan")}]
            ),
            "boolean silence threshold": lambda data: data.__setitem__(
                "silence_relative_lu", True
            ),
        }
        for name, mutate in cases.items():
            with self.subTest(name=name), tempfile.TemporaryDirectory() as directory:
                data = audio_energy_data()
                mutate(data)
                output_root = Path(directory)
                write_dispatcher_output(output_root, data)

                with self.assertRaises(bench.BenchError):
                    bench.validate_dispatcher_output(
                        output_root, "sample", {"duration_seconds": 2.0}
                    )

    def test_dispatcher_output_rejects_invalid_timing_and_rss_provenance(self) -> None:
        cases = {
            "boolean tool time": {"tool_ms": True},
            "negative total time": {"total_ms": -1},
            "string peak RSS": {"peak_rss_bytes": "4096"},
            "negative peak RSS": {"peak_rss_bytes": -1},
        }
        for name, overrides in cases.items():
            with self.subTest(name=name), tempfile.TemporaryDirectory() as directory:
                output_root = Path(directory)
                write_dispatcher_output(
                    output_root, audio_energy_data(), pair_overrides=overrides
                )

                with self.assertRaises(bench.BenchError):
                    bench.validate_dispatcher_output(
                        output_root, "sample", {"duration_seconds": 2.0}
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

    def test_temp_directory_observation_includes_nonzero_warmup_baseline(self) -> None:
        observation = bench.temp_directory_observation(
            [
                {
                    "temp_directory_high_water_bytes": 4096,
                    "sampler": {
                        "gap_ms": {"max": 40.0},
                        "observed_rate_hz": 38.5,
                    },
                    "cleanup": {
                        "passed": True,
                        "leaked_audio_temp_files": [],
                        "remaining_temp_files": ["/tmp/uv-cache.lock"],
                    },
                },
                {
                    "temp_directory_high_water_bytes": 2048,
                    "sampler": {
                        "gap_ms": {"max": 55.0},
                        "observed_rate_hz": 20.0,
                    },
                    "cleanup": {
                        "passed": True,
                        "leaked_audio_temp_files": [],
                        "remaining_temp_files": [],
                    },
                },
            ]
        )

        self.assertEqual(
            observation,
            {
                "method": "periodic recursive byte-size polling of isolated TMPDIRs",
                "target_interval_ms": 25.0,
                "maximum_observed_high_water_bytes": 4096,
                "maximum_observed_sampler_gap_ms": 55.0,
                "minimum_observed_rate_hz": 20.0,
                "post_run_audio_temp_leak_check": {
                    "passed": True,
                    "leaked_audio_temp_files": [],
                },
                "limitation": (
                    "Periodic polling cannot exclude transient files between samples "
                    "or prove decoder transport."
                ),
            },
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

    def test_sampler_timing_allows_a_recorded_swap_stall_with_dense_coverage(self) -> None:
        timing = bench.validate_sampler_timing(
            "baseline",
            wall_seconds=3.64,
            sampler_count=101,
            sample_gaps_seconds=[0.025] * 100 + [1.14],
        )

        self.assertAlmostEqual(timing["observed_rate_hz"], 101 / 3.64)
        self.assertEqual(timing["gap_ms"]["max"], 1140.0)

    def test_sampler_timing_rejects_sparse_coverage(self) -> None:
        with self.assertRaisesRegex(bench.BenchError, "sample rate"):
            bench.validate_sampler_timing(
                "baseline",
                wall_seconds=10.0,
                sampler_count=10,
                sample_gaps_seconds=[1.0] * 9,
            )


if __name__ == "__main__":
    unittest.main()
