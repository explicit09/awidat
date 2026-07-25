#!/usr/bin/env python3
"""Tests for align.py against synthetic raw/published pairs with KNOWN
ground truth — the aligner must recover a planted cut and a planted
speed-up, and must not hallucinate speed from timestamp jitter.

Run: python3 -m unittest discover tools/taste-corpus
"""

from __future__ import annotations

import json
import os
import random
import subprocess
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(__file__))
from align import align, vtt_cues  # noqa: E402

# Distinct content vocabulary so 15s windows are separable. Each sentence
# is unique and content-word heavy (no stopword collapse).
TOPICS = [
    "quantum radar calibrates niobium resonators beneath glacier observatories",
    "fermentation chemists culture heirloom koji spores inside cedar chambers",
    "orbital debris sweepers magnetize tungsten nets around derelict boosters",
    "coral geneticists splice thermal tolerance into staghorn polyp colonies",
    "volcanic sommeliers age basalt-filtered vintages in obsidian cellars",
    "typeface historians digitize medieval ligatures from vellum manuscripts",
    "submarine agronomists harvest kelp lattices near hydrothermal meadows",
    "auroral photographers chase magnetospheric substorms across tundra plateaus",
    "clockwork restorers regrind escapement pallets for observatory regulators",
    "mycelium architects grow load-bearing chitin trusses for pavilion roofs",
    "desert glaciologists core fossil aquifer ice beneath dune escarpments",
    "railway acousticians tune viaduct resonance with tuned mass dampers",
    "papermakers marble indigo suminagashi sheets in cypress vats",
    "beekeeping cartographers map alpine forage corridors for nomad apiaries",
    "meteorite curators classify pallasite olivine under polarized microscopes",
    "tidal engineers phase lagoon turbines against equinox spring currents",
]


def index_word(i: int, salt: int) -> str:
    """A unique, letters-only content word per (index, salt) — digits are
    filtered by the aligner's [a-z']{3,} tokenizer, so uniqueness must be
    alphabetic."""
    n = i * 5 + salt
    letters = []
    for _ in range(4):
        letters.append(chr(ord("a") + n % 26))
        n //= 26
    return "zz" + "".join(letters)


def sentence(i: int) -> str:
    # A cycling topic base for texture, plus THREE globally-unique
    # content words so no two sentences share a full window signature.
    # (Real transcripts don't repeat verbatim topics every ~80s; a fixture
    # with cyclic-only content lets the matcher lock-pick through cuts.)
    base = TOPICS[i % len(TOPICS)]
    return f"{base} {index_word(i, 0)} {index_word(i, 1)} {index_word(i, 2)}"


def write_vtt(path: str, cues: list[tuple[float, str]]) -> None:
    with open(path, "w", encoding="utf-8") as f:
        f.write("WEBVTT\n\n")
        for start, text in cues:
            end = start + 4.0

            def ts(t: float) -> str:
                h, rem = divmod(t, 3600)
                m, s = divmod(rem, 60)
                return f"{int(h):02d}:{int(m):02d}:{int(s):02d}.{int((t % 1) * 1000):03d}"

            f.write(f"{ts(start)} --> {ts(end)}\n{text}\n\n")


def build_raw(n_sentences: int = 240, spacing: float = 5.0) -> list[tuple[float, str]]:
    """Raw show: one distinct sentence every `spacing` seconds (20 min)."""
    return [(i * spacing, sentence(i)) for i in range(n_sentences)]


class AlignRecoversPlantedEdit(unittest.TestCase):
    """Published = raw with sentences 60..119 removed (a 300s mid-show cut)
    and sentences 150..209 compressed 1.5x (a 300s->200s speed-up)."""

    @classmethod
    def setUpClass(cls):
        cls.dir = tempfile.TemporaryDirectory()
        raw = build_raw()
        cls.raw_path = os.path.join(cls.dir.name, "raw.vtt")
        write_vtt(cls.raw_path, raw)

        pub: list[tuple[float, str]] = []
        t = 0.0
        for i, (_, text) in enumerate(raw):
            if 60 <= i < 120:
                continue  # the planted cut: raw 300s..600s
            step = 5.0 / 1.5 if 150 <= i < 210 else 5.0
            pub.append((t, text))
            t += step
        cls.pub_path = os.path.join(cls.dir.name, "pub.vtt")
        write_vtt(cls.pub_path, pub)

        cls.result = align(cls.raw_path, cls.pub_path)

    @classmethod
    def tearDownClass(cls):
        cls.dir.cleanup()

    def test_coverage_is_high(self):
        self.assertGreaterEqual(self.result["alignment"]["coverage"], 0.85)

    def test_planted_cut_is_recovered_in_show(self):
        cuts = [
            s
            for s in self.result["segments"]
            if s["action"] == "cut" and s.get("scope") == "in_show"
        ]
        self.assertTrue(cuts, "expected an in-show cut region")
        # The planted cut removes raw 300s..600s; window quantization
        # allows one 15s window of slack per side.
        best = min(cuts, key=lambda s: abs(s["raw_span"][0] - 300.0))
        self.assertLessEqual(abs(best["raw_span"][0] - 300.0), 30.0)
        self.assertLessEqual(abs(best["raw_span"][1] - 600.0), 30.0)

    def test_planted_speedup_is_recovered(self):
        sped = [
            s
            for s in self.result["segments"]
            if s["action"] == "keep" and s.get("speed", 1.0) > 1.0
        ]
        self.assertTrue(sped, "expected a sped keep run")
        # Planted region: raw 750s..1050s at 1.5x. Window-ratio
        # measurement is coarse (15s buckets), so accept 1.2..1.9.
        overlapping = [
            s for s in sped if s["raw_span"][1] > 750.0 and s["raw_span"][0] < 1050.0
        ]
        self.assertTrue(overlapping, f"no sped run overlaps planted region: {sped}")
        for s in overlapping:
            self.assertGreaterEqual(s["speed"], 1.2)
            self.assertLessEqual(s["speed"], 1.9)

    def test_unedited_regions_stay_at_speed_one(self):
        # Runs fully inside raw 0..300s (before any planted edit) must not
        # report speed: jitter below the trust threshold snaps to 1.0.
        early = [
            s
            for s in self.result["segments"]
            if s["action"] == "keep" and s["raw_span"][1] <= 300.0
        ]
        self.assertTrue(early, "expected keep coverage before the cut")
        for s in early:
            self.assertEqual(s["speed"], 1.0)


class AlignLocksThroughLongPreShow(unittest.TestCase):
    """A raw with a pre-show longer than the banded lookahead (10 min)
    must still lock: unlocked search is global. This is the exact failure
    the real BRINK pair exposed (24-min pre-show discard -> 8% coverage
    with an always-banded matcher; 100% with global re-lock)."""

    def test_preshow_longer_than_band_still_aligns(self):
        with tempfile.TemporaryDirectory() as d:
            # 15 min of pre-show junk (sentences 1000..1179 — unique words
            # never seen in the published cut), then the 10-min show.
            pre = [(i * 5.0, sentence(1000 + i)) for i in range(180)]
            show = [(900.0 + i * 5.0, sentence(i)) for i in range(120)]
            raw_path = os.path.join(d, "raw.vtt")
            write_vtt(raw_path, pre + show)
            pub_path = os.path.join(d, "pub.vtt")
            write_vtt(pub_path, [(i * 5.0, sentence(i)) for i in range(120)])

            result = align(raw_path, pub_path)
            self.assertGreaterEqual(result["alignment"]["coverage"], 0.9)
            pre_cuts = [
                s
                for s in result["segments"]
                if s["action"] == "cut" and s.get("scope") == "pre_show"
            ]
            self.assertTrue(pre_cuts, "expected a pre_show cut segment")
            self.assertLessEqual(abs(pre_cuts[0]["raw_span"][1] - 900.0), 30.0)


class AlignHandlesIdenticalPair(unittest.TestCase):
    def test_identity_pair_is_one_keep_run_no_cuts(self):
        with tempfile.TemporaryDirectory() as d:
            raw = build_raw(120)
            p = os.path.join(d, "raw.vtt")
            write_vtt(p, raw)
            result = align(p, p)
            self.assertGreaterEqual(result["alignment"]["coverage"], 0.95)
            cuts = [s for s in result["segments"] if s["action"] == "cut"]
            self.assertEqual(cuts, [], "identity pair must have no cut regions")
            for s in result["segments"]:
                self.assertEqual(s["speed"], 1.0)


class CliEmitsSchemaConformingJson(unittest.TestCase):
    def test_cli_output_validates_against_schema_shape(self):
        with tempfile.TemporaryDirectory() as d:
            raw = build_raw(120)
            raw_path = os.path.join(d, "raw.vtt")
            write_vtt(raw_path, raw)
            out = os.path.join(d, "pair.json")
            proc = subprocess.run(
                [
                    sys.executable,
                    os.path.join(os.path.dirname(__file__), "align.py"),
                    "--raw-vtt",
                    raw_path,
                    "--published-vtt",
                    raw_path,
                    "--pair-id",
                    "identity-test",
                    "--house",
                    "testhouse",
                    "--raw-ref",
                    "raw.vtt",
                    "--published-ref",
                    "raw.vtt",
                    "--out",
                    out,
                ],
                capture_output=True,
                text=True,
            )
            self.assertEqual(proc.returncode, 0, proc.stderr)
            with open(out, encoding="utf-8") as f:
                doc = json.load(f)
            # Structural conformance to decision_list_schema.json's
            # required fields (full JSON Schema validation happens on the
            # Rust side, which embeds the same schema file).
            for key in (
                "version",
                "pair_id",
                "house",
                "raw_ref",
                "published_ref",
                "window_s",
                "alignment",
                "segments",
            ):
                self.assertIn(key, doc)
            self.assertEqual(doc["version"], 1)
            for seg in doc["segments"]:
                self.assertIn(seg["action"], ("keep", "cut"))
                self.assertEqual(len(seg["raw_span"]), 2)


class VttParsing(unittest.TestCase):
    def test_vtt_cues_strip_tags_and_ordinals(self):
        with tempfile.TemporaryDirectory() as d:
            p = os.path.join(d, "x.vtt")
            with open(p, "w", encoding="utf-8") as f:
                f.write(
                    "WEBVTT\n\n1\n00:00:01.000 --> 00:00:03.000\n"
                    "<v Speaker>Hello <b>world</b>\n\n"
                    "2\n00:00:04.500 --> 00:00:06.000\nsecond cue text\n\n"
                )
            cues = vtt_cues(p)
            self.assertEqual(len(cues), 2)
            self.assertEqual(cues[0], (1.0, "hello world"))
            self.assertEqual(cues[1][0], 4.5)


if __name__ == "__main__":
    unittest.main()
