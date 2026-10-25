"""Grouped classifier checks; proposal confidence is never capture consent."""

import copy
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

import hiss_intervals as learner
import numpy as np
from hiss_interval_cases import CONTEXT, session, signal
from scipy.optimize import minimize
from scipy.special import expit


class HissIntervalTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.temp = tempfile.TemporaryDirectory()
        cls.root = Path(cls.temp.name)
        cls.data = session(cls.root / "labels")
        cls.result = learner.fit(cls.data)
        if not cls.result["accepted"]:
            raise AssertionError(cls.result["reasons"])
        cls.model = cls.result["model"]

    @classmethod
    def tearDownClass(cls):
        cls.temp.cleanup()

    def test_grouped_fit_is_explicit_and_optimizer_matches_independent_bfgs(self):
        self.assertEqual(self.result["held_source_programme_proposals"], 0)
        self.assertGreaterEqual(self.result["held_source_noise_coverage"], 0.25)
        self.assertIs(self.model["automatic_capture"], False)
        x, y, groups = learner.prepare(self.data)
        names, index, counts = np.unique(
            groups, return_inverse=True, return_counts=True
        )
        weights = 1 / counts[index]
        weights /= weights.sum()
        actual = self.model["parameters"]["main"]
        design = np.column_stack(
            [(x - actual["center"]) / actual["scale"], np.ones(len(x))]
        )
        penalty = np.r_[np.full(16, 0.01), 1e-6]

        def loss(theta):
            z = design @ theta
            return float(
                weights @ (np.logaddexp(0, z) - y * z)
                + np.dot(penalty * theta, theta) / 2
            )

        def grad(theta):
            return design.T @ (weights * (expit(design @ theta) - y)) + penalty * theta

        solved = minimize(
            loss, np.zeros(17), jac=grad, method="BFGS", options={"gtol": 1e-10}
        )
        np.testing.assert_allclose(actual["weights"], solved.x, atol=2e-6, rtol=0)
        self.assertEqual(len(names), 8)

    def test_features_are_gain_and_channel_order_invariant(self):
        x = signal("noise_only", 20731026)
        np.testing.assert_allclose(
            learner.features(x, CONTEXT),
            learner.features(x * 0.125, CONTEXT),
            atol=1e-10,
            rtol=0,
        )
        stereo = np.column_stack([x[:, 0], signal("programme", 20731027)[:, 0]])
        ctx = dict(CONTEXT, channels=2)
        np.testing.assert_array_equal(
            learner.features(stereo, ctx), learner.features(stereo[:, ::-1], ctx)
        )
        with self.assertRaises(ValueError):
            learner.features(np.zeros((48000, 1)), CONTEXT)

    def test_prefix_model_unavailable_and_context_mismatch_never_capture(self):
        x = np.vstack([signal("noise_only", 20741026 + i) for i in range(3)])
        a = learner.propose(x[:108001], CONTEXT, self.model)
        b = learner.propose(x, CONTEXT, self.model)
        self.assertEqual(a["intervals"], b["intervals"][:2])
        self.assertEqual(a["unused_final_frames"], 12001)
        self.assertTrue(
            all(r["available_at_frame"] == r["end_frame"] for r in b["intervals"])
        )
        for model, ctx in [
            (None, CONTEXT),
            (dict(self.model, admitted=False), CONTEXT),
            (self.model, dict(CONTEXT, capture_setup_id="another-chain")),
        ]:
            result = learner.propose(x, ctx, model)
            self.assertFalse(any(r["review_candidate"] for r in result["intervals"]))
            self.assertFalse(result["profile_changed"])
            self.assertFalse(result["automatic_capture"])

    def test_repeated_source_and_contradictory_feature_labels_do_not_admit(self):
        data = copy.deepcopy(self.data)
        for r in data["records"]:
            r["source_group"] = "same-source"
        result = learner.fit(data)
        self.assertFalse(result["accepted"])
        self.assertIn("insufficient_independent_labelled_sources", result["reasons"])
        data = copy.deepcopy(self.data)
        data["records"][1].update(
            audio_file=data["records"][0]["audio_file"],
            audio_sha256=data["records"][0]["audio_sha256"],
        )
        with self.assertRaises(ValueError):
            learner.prepare(data)
        data = copy.deepcopy(self.data)
        data["protocol"]["whole_source_grouping"] = 1
        with self.assertRaises(ValueError):
            learner.prepare(data)

    def test_semantic_counterfactual_has_identical_review_decision(self):
        # Unwanted hiss and a desired noise effect may contain identical PCM.
        x = signal("noise_only", 20751026)
        first = learner.propose(x, CONTEXT, self.model)
        second = learner.propose(x.copy(), CONTEXT, self.model)
        self.assertEqual(first, second)
        self.assertFalse(first["automatic_capture"])
        self.assertFalse(first["profile_changed"])

    def test_bounded_verified_loader_and_cli_fallback_preserve_outputs(self):
        path = self.root / "model.json"
        raw = (json.dumps(self.model) + "\n").encode()
        path.write_bytes(raw)
        loaded = learner.load_model(path, learner.digest(raw))
        self.assertEqual(loaded, self.model)
        with self.assertRaises(ValueError):
            learner.load_model(path, "0" * 64)
        bad = self.root / "bad-model.json"
        bad.write_text("[]")
        with self.assertRaises(ValueError):
            learner.load_model(bad, learner.digest(bad.read_bytes()))
        ctx = self.root / "context.json"
        ctx.write_text(json.dumps(CONTEXT))
        audio = self.root / "proposal.f32le"
        audio.write_bytes(signal("noise_only", 20761026).astype("<f4").tobytes())
        output = self.root / "missing-model"
        command = [
            sys.executable,
            "scripts/ml/propose_hiss_intervals.py",
            "--audio",
            str(audio),
            "--frames",
            "48000",
            "--context",
            str(ctx),
            "--output",
            str(output),
        ]
        result = subprocess.run(command, capture_output=True, text=True, check=False)
        self.assertEqual(result.returncode, 2, result.stderr)
        report = json.loads((output / "proposals.json").read_text())
        self.assertFalse(any(r["review_candidate"] for r in report["intervals"]))
        before = (output / "proposals.json").read_bytes()
        again = subprocess.run(command, capture_output=True, text=True, check=False)
        self.assertNotEqual(again.returncode, 0)
        self.assertEqual(before, (output / "proposals.json").read_bytes())
        self.assertFalse((output / "model.json").exists())
        self.assertFalse((output / "preset.json").exists())


if __name__ == "__main__":
    unittest.main()
