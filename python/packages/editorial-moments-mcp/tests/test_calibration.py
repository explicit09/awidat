"""Tests for the editorial-moments calibration harness.

The harness produces three calibration metrics — Pearson correlation,
top-K agreement, and calibration-bin means — over a labeled-clip
fixture set. We test those formulas on synthetic data so the metrics
are deterministic and don't require running the LLM scorer.
"""

from __future__ import annotations

import importlib.util
import json
import math
import os
import sys
import tempfile
import unittest
from pathlib import Path


PKG_ROOT = Path(__file__).resolve().parents[1]
SRC = PKG_ROOT / "src" / "editorial_moments_mcp" / "calibration.py"
SPEC = importlib.util.spec_from_file_location(
    "editorial_moments_mcp.calibration", SRC
)
assert SPEC is not None and SPEC.loader is not None
calibration = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(calibration)


class PearsonCorrelationTests(unittest.TestCase):
    def test_perfect_positive_correlation_returns_one(self) -> None:
        preds = [0.0, 0.25, 0.5, 0.75, 1.0]
        labels = [0.0, 0.25, 0.5, 0.75, 1.0]
        r = calibration.pearson_correlation(preds, labels)
        self.assertAlmostEqual(r, 1.0, places=6)

    def test_perfect_negative_correlation_returns_minus_one(self) -> None:
        preds = [0.0, 0.25, 0.5, 0.75, 1.0]
        labels = [1.0, 0.75, 0.5, 0.25, 0.0]
        r = calibration.pearson_correlation(preds, labels)
        self.assertAlmostEqual(r, -1.0, places=6)

    def test_orthogonal_inputs_return_near_zero(self) -> None:
        # Constant-mean orthogonal-ish: alternating +/- around a mean.
        preds = [1.0, -1.0, 1.0, -1.0, 1.0, -1.0]
        labels = [1.0, 1.0, -1.0, -1.0, 1.0, -1.0]
        r = calibration.pearson_correlation(preds, labels)
        self.assertLess(abs(r), 0.5)

    def test_constant_input_returns_nan(self) -> None:
        # Pearson is undefined when one series has zero variance. We
        # explicitly return NaN so the report writer can mark it as
        # "insufficient signal" rather than silently lying with a 0.
        preds = [0.5, 0.5, 0.5, 0.5]
        labels = [0.0, 0.25, 0.5, 0.75]
        r = calibration.pearson_correlation(preds, labels)
        self.assertTrue(math.isnan(r))

    def test_short_input_raises(self) -> None:
        with self.assertRaises(ValueError):
            calibration.pearson_correlation([0.5], [0.5])


class TopKAgreementTests(unittest.TestCase):
    def test_identical_ordering_full_agreement(self) -> None:
        # Labels and preds both rank items in the same order: top-3
        # overlap is 3/3 = 1.0.
        preds = [0.1, 0.9, 0.5, 0.7, 0.3]
        labels = [0.1, 0.9, 0.5, 0.7, 0.3]
        agreement = calibration.top_k_agreement(preds, labels, k=3)
        self.assertAlmostEqual(agreement, 1.0)

    def test_reversed_ordering_no_agreement(self) -> None:
        # Top-2 predicted are the bottom-2 labeled.
        preds = [0.1, 0.2, 0.3, 0.9, 1.0]
        labels = [0.9, 1.0, 0.3, 0.2, 0.1]
        agreement = calibration.top_k_agreement(preds, labels, k=2)
        self.assertAlmostEqual(agreement, 0.0)

    def test_k_exceeds_population_clamps(self) -> None:
        preds = [0.1, 0.4, 0.9]
        labels = [0.2, 0.5, 0.8]
        # With k>=n, the top-k of each is the whole population —
        # agreement is trivially 1.0.
        agreement = calibration.top_k_agreement(preds, labels, k=10)
        self.assertAlmostEqual(agreement, 1.0)

    def test_invalid_k_raises(self) -> None:
        with self.assertRaises(ValueError):
            calibration.top_k_agreement([0.1, 0.2], [0.1, 0.2], k=0)


class CalibrationBinsTests(unittest.TestCase):
    def test_well_calibrated_predictor_produces_monotonic_bin_means(self) -> None:
        # Synthetic well-calibrated case: predicted == label.
        preds = [i / 20 for i in range(21)]
        labels = list(preds)
        bins = calibration.calibration_bins(preds, labels, n_bins=4)
        self.assertEqual(len(bins), 4)
        means = [b["label_mean"] for b in bins if b["count"] > 0]
        self.assertEqual(means, sorted(means))
        # Each bin reports its predicted-score range and population count.
        for b in bins:
            self.assertIn("pred_lo", b)
            self.assertIn("pred_hi", b)
            self.assertIn("count", b)
            self.assertIn("label_mean", b)
            self.assertIn("pred_mean", b)

    def test_empty_bin_reports_zero_count_and_nan_means(self) -> None:
        # All preds clustered low — high-prediction bins should be empty.
        preds = [0.05, 0.1, 0.15, 0.2]
        labels = [0.0, 0.0, 0.0, 0.0]
        bins = calibration.calibration_bins(preds, labels, n_bins=4)
        # Only the lowest bin should have count > 0.
        non_empty = [b for b in bins if b["count"] > 0]
        self.assertEqual(len(non_empty), 1)
        for b in bins:
            if b["count"] == 0:
                self.assertTrue(math.isnan(b["label_mean"]))
                self.assertTrue(math.isnan(b["pred_mean"]))


class EvaluateFixturesTests(unittest.TestCase):
    """Integration of all three metrics behind the harness entrypoint."""

    def test_evaluate_returns_per_moment_type_report(self) -> None:
        # Five synthetic clips with hand-crafted preds/labels per kind.
        preds_by_clip = [
            {"hook": 0.9, "punchline": 0.1, "emotional_peak": 0.2},
            {"hook": 0.7, "punchline": 0.3, "emotional_peak": 0.4},
            {"hook": 0.5, "punchline": 0.5, "emotional_peak": 0.6},
            {"hook": 0.3, "punchline": 0.7, "emotional_peak": 0.8},
            {"hook": 0.1, "punchline": 0.9, "emotional_peak": 1.0},
        ]
        labels_by_clip = [
            {"hook": 1.0, "punchline": 0.0, "emotional_peak": 0.2},
            {"hook": 0.8, "punchline": 0.2, "emotional_peak": 0.4},
            {"hook": 0.5, "punchline": 0.5, "emotional_peak": 0.6},
            {"hook": 0.2, "punchline": 0.8, "emotional_peak": 0.8},
            {"hook": 0.0, "punchline": 1.0, "emotional_peak": 1.0},
        ]
        report = calibration.evaluate(
            predictions=preds_by_clip,
            ground_truth=labels_by_clip,
            top_k=2,
            n_bins=4,
        )
        self.assertIn("per_kind", report)
        for kind in ("hook", "punchline", "emotional_peak"):
            self.assertIn(kind, report["per_kind"])
            entry = report["per_kind"][kind]
            self.assertIn("pearson", entry)
            self.assertIn("top_k_agreement", entry)
            self.assertIn("top_k", entry)
            self.assertIn("bins", entry)
            self.assertIn("n", entry)
            # All three kinds are designed to correlate strongly.
            self.assertGreater(entry["pearson"], 0.95)
            self.assertAlmostEqual(entry["top_k_agreement"], 1.0)

    def test_evaluate_handles_missing_kinds_per_clip(self) -> None:
        # If a clip is missing a kind in either preds or labels, it is
        # excluded from that kind's metrics — not silently coerced to 0.
        preds = [
            {"hook": 0.9},
            {"hook": 0.5, "punchline": 0.4},
            {"hook": 0.1, "punchline": 0.8},
        ]
        labels = [
            {"hook": 1.0},
            {"hook": 0.5, "punchline": 0.3},
            {"hook": 0.0, "punchline": 0.9},
        ]
        report = calibration.evaluate(
            predictions=preds, ground_truth=labels, top_k=2, n_bins=2
        )
        self.assertEqual(report["per_kind"]["hook"]["n"], 3)
        self.assertEqual(report["per_kind"]["punchline"]["n"], 2)

    def test_evaluate_skips_kinds_with_insufficient_data(self) -> None:
        # Only one clip rates 'cta' — Pearson needs ≥2 pairs.
        preds = [{"cta": 0.2}, {"hook": 0.9}, {"hook": 0.5}]
        labels = [{"cta": 0.3}, {"hook": 1.0}, {"hook": 0.6}]
        report = calibration.evaluate(
            predictions=preds, ground_truth=labels, top_k=1, n_bins=2
        )
        # 'cta' has only 1 paired observation; the harness should
        # flag this with `skipped` rather than crashing.
        self.assertEqual(report["per_kind"]["cta"]["n"], 1)
        self.assertTrue(report["per_kind"]["cta"].get("skipped"))
        self.assertEqual(report["per_kind"]["hook"]["n"], 2)


class ReportWriterTests(unittest.TestCase):
    def test_writes_json_and_markdown_side_by_side(self) -> None:
        report = {
            "fixtures": "test.json",
            "n_clips": 2,
            "per_kind": {
                "hook": {
                    "n": 2,
                    "pearson": 1.0,
                    "top_k": 1,
                    "top_k_agreement": 1.0,
                    "bins": [
                        {
                            "pred_lo": 0.0,
                            "pred_hi": 0.5,
                            "count": 1,
                            "pred_mean": 0.3,
                            "label_mean": 0.3,
                        },
                        {
                            "pred_lo": 0.5,
                            "pred_hi": 1.0,
                            "count": 1,
                            "pred_mean": 0.9,
                            "label_mean": 1.0,
                        },
                    ],
                },
            },
        }
        with tempfile.TemporaryDirectory() as tmp:
            json_path = Path(tmp) / "report.json"
            calibration.write_report(report, json_path)
            md_path = json_path.with_suffix(".md")
            self.assertTrue(json_path.exists())
            self.assertTrue(md_path.exists())
            loaded = json.loads(json_path.read_text())
            self.assertEqual(loaded["n_clips"], 2)
            md_body = md_path.read_text()
            self.assertIn("hook", md_body)
            self.assertIn("Pearson", md_body)
            self.assertIn("top-k agreement", md_body.lower())


class FixtureManifestShapeTests(unittest.TestCase):
    def test_skeleton_manifest_is_valid_json_with_expected_shape(self) -> None:
        manifest_path = (
            PKG_ROOT / "fixtures" / "calibration" / "manifest.json"
        )
        doc = json.loads(manifest_path.read_text())
        self.assertIn("clips", doc)
        self.assertIsInstance(doc["clips"], list)
        self.assertGreaterEqual(len(doc["clips"]), 2)
        for entry in doc["clips"]:
            self.assertIn("clip_path", entry)
            self.assertIn("labels", entry)
            self.assertIn("source", entry)
            self.assertIsInstance(entry["labels"], dict)
            for v in entry["labels"].values():
                self.assertGreaterEqual(v, 0.0)
                self.assertLessEqual(v, 1.0)


if __name__ == "__main__":
    unittest.main()
