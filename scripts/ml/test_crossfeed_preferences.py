"""Preference-learning, uncertainty and rejection checks using synthetic choices."""

import copy
import json
import math
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

import fit_crossfeed_preferences as fitter
import numpy as np
from scipy.optimize import minimize
from scipy.special import expit


def session(kind="consistent", programmes=8):
    candidates = []
    for index, (mode, feed) in enumerate([("Off", 0), ("Bauer", 4.5), ("Bauer", 9)]):
        candidates.append(
            {
                "id": "candidate-" + str(index),
                "parameters": {
                    "mode": mode,
                    "enabled": True,
                    "autogain_enabled": False,
                    "mix": 1.0,
                    "bauer_fcut_hz": 700,
                    "bauer_feed_db": feed,
                    "meier_level": 40,
                    "mb_low_freq_hz": 200,
                    "mb_mid_high_freq_hz": 4000,
                    "mb_low_feed_db": 6,
                    "mb_mid_feed_db": 3,
                    "mb_high_feed_db": 0,
                    "itd_delay_ms": 0,
                    "head_yaw_deg": 0,
                },
            }
        )
    trials = []
    for programme in range(programmes):
        for a, b in [(0, 1), (0, 2), (1, 2), (1, 0), (2, 0), (2, 1)]:
            for repeat in range(2):
                rank = (
                    [0, 2, 1]
                    if kind != "heterogeneous" or programme < programmes // 2
                    else [2, 0, 1]
                )
                choice = "a" if rank[a] > rank[b] else "b"
                if kind == "ties":
                    choice = "tie"
                elif kind == "side-bias":
                    choice = "a"
                trials.append(
                    {
                        "id": f"trial-{len(trials)}",
                        "programme_id": f"programme-{programme}",
                        "a": candidates[a]["id"],
                        "b": candidates[b]["id"],
                        "choice": choice,
                    }
                )
    return {
        "format": fitter.FORMAT,
        "data_origin": "synthetic_validation",
        "context": {
            "listener_id": "synthetic-listener",
            "headphones_id": "synthetic-headphones",
            "session_id": "functional-" + kind,
            "sample_rate": 48000,
        },
        "protocol": {
            "blinded": True,
            "randomized_order": True,
            "maximum_level_mismatch_db": 0.1,
        },
        "candidates": candidates,
        "baseline_id": "candidate-0",
        "trials": trials,
    }


class CrossfeedPreferenceTests(unittest.TestCase):
    def test_consistent_preferences_return_exact_auditioned_setting(self):
        data = session()
        result = fitter.fit(data)
        self.assertTrue(result["accepted"])
        self.assertEqual(result["selected_id"], "candidate-1")
        self.assertEqual(result["parameters"], data["candidates"][1]["parameters"])
        self.assertEqual(result["winner_bootstrap_fraction"], 1)
        self.assertEqual(result["lopo_winners"], ["candidate-1"] * 8)

    def test_ambiguous_heterogeneous_and_side_biased_choices_abstain(self):
        for kind in ("ties", "heterogeneous", "side-bias"):
            with self.subTest(kind=kind):
                result = fitter.fit(session(kind))
                self.assertFalse(result["accepted"])
                self.assertIsNone(result["parameters"])
                self.assertIsNone(result["selected_id"])
                self.assertEqual(result["fallback_id"], "candidate-0")

    def test_repetition_cannot_inflate_independent_programme_count(self):
        data = session(programmes=1)
        original = copy.deepcopy(data["trials"])
        for _ in range(10):
            for trial in original:
                trial = copy.deepcopy(trial)
                trial["id"] = f"repeat-{len(data['trials'])}"
                data["trials"].append(trial)
        result = fitter.fit(data)
        self.assertFalse(result["accepted"])
        self.assertIn(
            "insufficient_independent_programmes_or_trials", result["reasons"]
        )

    def test_disconnected_and_single_order_designs_abstain(self):
        data = session()
        data["trials"] = [
            t for t in data["trials"] if "candidate-2" not in (t["a"], t["b"])
        ]
        self.assertIn("disconnected_comparison_graph", fitter.fit(data)["reasons"])
        data = session()
        data["trials"] = [t for t in data["trials"] if t["a"] < t["b"]]
        self.assertIn(
            "pairs_not_auditioned_in_both_orders", fitter.fit(data)["reasons"]
        )

    def test_invalid_parameters_and_protocols_fail_before_learning(self):
        for key, value in [
            ("bauer_feed_db", 16),
            ("mix", float("nan")),
            ("head_yaw_deg", 1),
            ("autogain_enabled", True),
        ]:
            data = session()
            data["candidates"][1]["parameters"][key] = value
            with self.assertRaises(ValueError):
                fitter.validate(data)
        data = session()
        data["protocol"]["blinded"] = False
        with self.assertRaises(ValueError):
            fitter.validate(data)

    def test_duplicate_effective_settings_cannot_inflate_candidate_count(self):
        data = session()
        duplicate = copy.deepcopy(data["candidates"][1])
        duplicate["id"] = "duplicate"
        duplicate["parameters"]["meier_level"] = 90
        duplicate["parameters"]["bauer_fcut_hz"] = 700.0
        data["candidates"].append(duplicate)
        with self.assertRaisesRegex(ValueError, "Duplicate auditioned"):
            fitter.validate(data)

    def test_cli_refuses_existing_outputs_and_malformed_sessions(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "session.json"
            source.write_text(json.dumps(session()))
            output = root / "existing"
            output.mkdir()
            marker = output / "preset.json"
            marker.write_bytes(b"user preset")
            command = [
                sys.executable,
                str(Path(fitter.__file__)),
                "--session",
                str(source),
                "--output",
                str(output),
            ]
            completed = subprocess.run(
                command, check=False, capture_output=True, text=True
            )
            self.assertNotEqual(completed.returncode, 0)
            self.assertEqual(marker.read_bytes(), b"user preset")
            self.assertFalse((output / "report.json").exists())
            source.write_text("[]")
            command[-1] = str(root / "invalid")
            completed = subprocess.run(
                command, check=False, capture_output=True, text=True
            )
            self.assertNotEqual(completed.returncode, 0)
            self.assertFalse((root / "invalid").exists())

    def test_optimizer_agrees_with_independent_logistic_likelihood(self):
        _, design, labels, groups, _ = fitter.validate(session())
        weights = np.full(len(labels), 1 / len(labels))
        actual = fitter.learn(design, labels, weights)

        def reference(theta):
            p = expit(design @ theta)
            return (
                -np.mean(labels * np.log(p) + (1 - labels) * np.log(1 - p))
                + fitter.POLICY["ridge"] * np.dot(theta, theta) / 2
            )

        solved = minimize(
            reference, np.zeros(len(actual)), method="BFGS", options={"gtol": 1e-8}
        )
        np.testing.assert_allclose(actual, solved.x, atol=2e-6, rtol=0)
        loss, gradient, hessian = fitter.objective(actual, design, labels, weights)
        self.assertLess(loss, math.log(2))
        self.assertLess(np.linalg.norm(gradient), 1e-7)
        self.assertGreater(np.linalg.eigvalsh(hessian).min(), 0)
        self.assertAlmostEqual(actual[:-1].sum(), 0, places=8)
        self.assertEqual(len(set(groups)), 8)


if __name__ == "__main__":
    unittest.main()
