import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]


class EvalWorkflowContractTests(unittest.TestCase):
    def test_real_corpus_job_exposes_editorial_gate_variables(self) -> None:
        workflow = (ROOT / ".github" / "workflows" / "evals.yml").read_text()

        for name in [
            "AWIDAT_REAL_CORPUS",
            "AWIDAT_REAL_VISUAL_MIN_METADATA_SHOTS",
            "AWIDAT_REAL_VISUAL_MIN_METADATA_RATIO",
            "AWIDAT_REAL_VISUAL_MIN_MATCH_CANDIDATE_SHOTS",
            "AWIDAT_REAL_VISUAL_MIN_MODEL_COMPOSITION_SHOTS",
            "AWIDAT_REAL_VISUAL_MIN_COMPOSITION_MODEL_REGIONS",
            "AWIDAT_REAL_VISUAL_MAX_INVALID_COMPOSITION_MODEL_REGIONS",
            "AWIDAT_REAL_MIN_ASSESSOR_PROPOSAL_FIXTURES",
            "AWIDAT_REAL_MIN_TRANSITION_PLANNER_FIXTURES",
            "AWIDAT_REAL_MIN_ROUGH_ASSEMBLY_FIXTURES",
            "AWIDAT_COMPOSITION_MODEL_PROJECT",
            "AWIDAT_COMPOSITION_MODEL_MIN_REGIONS",
            "AWIDAT_COMPOSITION_MODEL_MAX_INVALID_REGIONS",
        ]:
            self.assertIn(name, workflow)

        self.assertIn('test -n "$AWIDAT_REAL_CORPUS"', workflow)
        self.assertIn('test -d "$AWIDAT_REAL_CORPUS"', workflow)
        self.assertIn('test -f "$AWIDAT_REAL_CORPUS/project.otio.json"', workflow)
        self.assertIn("python3 python/scripts/smoke_safe.py", workflow)
        self.assertIn(
            "env.AWIDAT_REAL_VISUAL_MIN_COMPOSITION_MODEL_REGIONS != '0'",
            workflow,
        )


if __name__ == "__main__":
    unittest.main()
