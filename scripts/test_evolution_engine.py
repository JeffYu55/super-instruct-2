#!/usr/bin/env python3
import json
import pathlib
import sys
import tempfile
import unittest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import evolution_engine as engine


class EvolutionEngineTests(unittest.TestCase):
    def config(self):
        return {
            "categories": ["pentest"],
            "eligible_skills": {"pentest": ["vuln-scanner"]},
            "min_samples": 2,
            "min_lift": 0.04,
            "rollback_min_samples": 2,
            "max_regression": 0.10,
            "max_skills_per_category": 1,
            "rolling_events": 100,
        }

    @staticmethod
    def event(score, skills, category="pentest"):
        return {
            "category": category,
            "skills": skills,
            "outcome": "model_success" if score else "protocol_error",
            "quality_score": 100 if score else 0,
            "evidence_coverage": score,
            "action_coverage": score,
        }

    def test_promotes_positive_eligible_skill(self):
        events = [
            self.event(1, ["vuln-scanner"]),
            self.event(1, ["vuln-scanner"]),
            self.event(0, []),
            self.event(0, []),
        ]
        candidate = engine.build_candidate(
            events, 4, self.config(), "config", "engine", {"generation": 2}
        )
        self.assertEqual(candidate["generation"], 3)
        self.assertEqual(candidate["routes"][0]["skills"], ["vuln-scanner"])
        self.assertEqual(candidate["source_offset"], 4)

    def test_general_events_never_create_route(self):
        events = [self.event(1, ["vuln-scanner"], "general") for _ in range(8)]
        candidate = engine.build_candidate(
            events, 8, self.config(), "config", "engine", {"generation": 0}
        )
        self.assertEqual(candidate["routes"], [])

    def test_atomic_promotion_and_rollback(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            policy = root / "policy.json"
            history = root / "history"
            engine.atomic_write(policy, {"generation": 1, "source_offset": 1})
            candidate = {"generation": 2, "source_offset": 2}
            generation = engine.promote(candidate, policy, history)
            self.assertTrue(generation.is_file())
            self.assertEqual(json.loads(policy.read_text())["generation"], 2)
            engine.rollback(policy)
            self.assertEqual(json.loads(policy.read_text())["generation"], 1)

    def test_detects_post_promotion_regression(self):
        previous = {
            "routes": [{
                "category": "pentest",
                "skills": ["vuln-scanner"],
                "baseline_score": 0.8,
            }]
        }
        regressions = engine.detect_regression(
            [self.event(0, ["vuln-scanner"]), self.event(0, ["vuln-scanner"])],
            previous,
            self.config(),
        )
        self.assertEqual(regressions[0]["category"], "pentest")
        self.assertEqual(regressions[0]["skills"], ["vuln-scanner"])


if __name__ == "__main__":
    unittest.main()
