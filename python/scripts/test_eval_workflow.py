import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]


class EvalWorkflowContractTests(unittest.TestCase):
    def test_real_corpus_job_exposes_editorial_gate_variables(self) -> None:
        workflow = (ROOT / ".github" / "workflows" / "evals.yml").read_text()

        for name in [
            "MONTAGE_REAL_CORPUS",
            "MONTAGE_REAL_VISUAL_MIN_METADATA_SHOTS",
            "MONTAGE_REAL_VISUAL_MIN_METADATA_RATIO",
            "MONTAGE_REAL_VISUAL_MIN_MATCH_CANDIDATE_SHOTS",
            "MONTAGE_REAL_VISUAL_MIN_MODEL_COMPOSITION_SHOTS",
            "MONTAGE_REAL_VISUAL_MIN_COMPOSITION_MODEL_REGIONS",
            "MONTAGE_REAL_VISUAL_MAX_INVALID_COMPOSITION_MODEL_REGIONS",
            "MONTAGE_REAL_MIN_ASSESSOR_PROPOSAL_FIXTURES",
            "MONTAGE_REAL_MIN_TRANSITION_PLANNER_FIXTURES",
            "MONTAGE_REAL_MIN_ROUGH_ASSEMBLY_FIXTURES",
            "MONTAGE_COMPOSITION_MODEL_PROJECT",
            "MONTAGE_COMPOSITION_MODEL_MIN_REGIONS",
            "MONTAGE_COMPOSITION_MODEL_MAX_INVALID_REGIONS",
        ]:
            self.assertIn(name, workflow)

        self.assertIn('test -n "$MONTAGE_REAL_CORPUS"', workflow)
        self.assertIn('test -d "$MONTAGE_REAL_CORPUS"', workflow)
        self.assertIn('test -f "$MONTAGE_REAL_CORPUS/project.otio.json"', workflow)
        self.assertIn("python3 python/scripts/smoke_safe.py", workflow)
        self.assertIn(
            "env.MONTAGE_REAL_VISUAL_MIN_COMPOSITION_MODEL_REGIONS != '0'",
            workflow,
        )


if __name__ == "__main__":
    unittest.main()
