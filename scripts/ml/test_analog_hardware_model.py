"""Analytic gradient, causality, training split isolation and classical comparison."""

import copy
import hashlib
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import numpy as np
from analog_hardware_capture import read_corpus
from analog_hardware_cases import session
from analog_hardware_model import (
    evaluate,
    fit,
    load_model,
    objective,
    predict,
    training_design,
    validate_model,
    windows,
)
from train_analog_hardware import run


class ModelTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.temp = tempfile.TemporaryDirectory()
        cls.data = session(Path(cls.temp.name) / "fixtures")
        cls.loaded, _ = read_corpus(cls.data)
        cls.model, cls.diagnostics = fit(
            cls.data, cls.loaded, taps=3, hidden=6, iterations=300
        )

    @classmethod
    def tearDownClass(cls):
        cls.temp.cleanup()

    def test_analytic_gradient_matches_finite_differences(self):
        rng = np.random.default_rng(20261103)
        x = rng.normal(size=(57, 3))
        y = rng.normal(size=57)
        theta = rng.normal(0, 0.1, 3 + 3 * 4 + 4 + 4 + 1)
        weights = np.full(57, 1 / 57)
        _, gradient = objective(theta, x, y, weights, 3, 4, 1.0, 1e-6)
        finite = []
        for i in range(len(theta)):
            delta = np.zeros(len(theta))
            delta[i] = 1e-6
            finite.append(
                (
                    objective(theta + delta, x, y, weights, 3, 4, 1.0, 1e-6)[0]
                    - objective(theta - delta, x, y, weights, 3, 4, 1.0, 1e-6)[0]
                )
                / 2e-6
            )
        np.testing.assert_allclose(gradient, finite, atol=1e-9, rtol=0)

    def test_explicit_hash_json_reopening_and_corrupt_candidate_refusal(self):
        path = Path(self.temp.name) / "candidate.json"
        path.write_text(json.dumps(self.model, allow_nan=False))
        h = hashlib.sha256(path.read_bytes()).hexdigest()
        reopened = load_model(path, h, self.data["context"])
        x = self.loaded["record-10"][0]
        np.testing.assert_array_equal(
            predict(self.model, x, self.data["context"]),
            predict(reopened, x, self.data["context"]),
        )
        with self.assertRaises(ValueError):
            load_model(path, "0" * 64, self.data["context"])
        path.write_text(" " + path.read_text())
        with self.assertRaises(ValueError):
            load_model(path, h, self.data["context"])

    def test_context_domain_bounds_and_malformed_parameters_refuse(self):
        context = copy.deepcopy(self.data["context"])
        context["sample_rate"] = 96000
        with self.assertRaises(ValueError):
            predict(self.model, np.zeros(10), context)
        with self.assertRaises(ValueError):
            predict(self.model, np.ones(10), self.data["context"])
        for key, value in [
            ("linear", [float("nan")] * 3),
            ("global_output_bound", 0),
            ("deployment_admitted", True),
        ]:
            bad = dict(self.model, **{key: value})
            with self.assertRaises(ValueError):
                validate_model(bad)
        signals = dict(self.loaded)
        x, wet = signals["record-10"]
        bad_target = wet.copy()
        bad_target[0] = np.nan
        signals["record-10"] = (x, bad_target)
        with self.assertRaises(ValueError):
            evaluate(self.model, self.data, signals, "test")

    def test_current_then_past_causality_global_bound_and_zero_response(self):
        np.testing.assert_array_equal(
            windows([1, 2, 3], 3), [[1, 0, 0], [2, 1, 0], [3, 2, 1]]
        )
        x = np.zeros(101)
        y = predict(self.model, x, self.data["context"])
        x[50] = 0.2
        changed = predict(self.model, x, self.data["context"])
        np.testing.assert_array_equal(y[:50], changed[:50])
        np.testing.assert_array_equal(y[53:], changed[53:])
        self.assertLess(np.max(abs(changed)), self.model["global_output_bound"] + 1e-12)

    def test_cross_memory_held_source_quality_and_frozen_classical_baseline(self):
        for split in ["validation", "test"]:
            result = evaluate(self.model, self.data, self.loaded, split)
            self.assertEqual(result["source_groups"], 2)
            self.assertTrue(all(r["supported"] for r in result["captures"]))
            # Record quality rather than assuming a neural win on every physical target.
            self.assertTrue(
                all(np.isfinite(r["neural_nmse_db"]) for r in result["captures"])
            )
        self.assertFalse(self.model["deployment_admitted"])
        self.assertEqual(self.model["context"]["data_origin"], "synthetic_validation")

    def test_training_weights_do_not_read_held_targets(self):
        loaded = dict(self.loaded)
        for r in self.data["captures"]:
            if r["split"] != "train":
                loaded[r["id"]] = (None, None)
        other, _ = fit(self.data, loaded, taps=3, hidden=6, iterations=300)
        self.assertEqual(other, self.model)

    def test_source_equal_total_weight_with_repeated_hardware_capture(self):
        data = copy.deepcopy(self.data)
        extra = dict(data["captures"][0], id="repeat-capture")
        path = Path(self.temp.name) / "repeat-wet.f32le"
        wet = (self.loaded["record-0"][1].astype(float) + 1e-6).astype("<f4")
        path.write_bytes(wet.tobytes())
        extra.update(
            wet_file=str(path), wet_sha256=hashlib.sha256(path.read_bytes()).hexdigest()
        )
        data["captures"].append(extra)
        signals, _ = read_corpus(data)
        _, _, weights, _, _ = training_design(data, signals, 3, 6, 300)
        quota = len(weights) // 9
        self.assertAlmostEqual(
            weights[:quota].sum() + weights[-quota:].sum(), 1 / 8, places=12
        )
        for record in range(1, 8):
            self.assertAlmostEqual(
                weights[record * quota : (record + 1) * quota].sum(), 1 / 8, places=12
            )

    def test_candidate_is_frozen_before_either_held_pcm_read(self):
        output = Path(self.temp.name) / "freeze-proof"
        seen = []
        frozen = []

        def checked_read(data):
            split = data["captures"][0]["split"]
            seen.append(split)
            if split == "train":
                self.assertFalse((output / "candidate.json").exists())
            else:
                frozen.append((output / "candidate.json").read_bytes())
            return read_corpus(data)

        with (
            patch("train_analog_hardware.read_corpus", side_effect=checked_read),
            patch(
                "train_analog_hardware.fit",
                return_value=(copy.deepcopy(self.model), dict(self.diagnostics)),
            ),
        ):
            _, report = run(self.data, output, taps=3, hidden=6, iterations=300)
        self.assertEqual(seen, ["train", "validation", "test"])
        self.assertEqual(frozen, [frozen[0]] * 2)
        self.assertEqual(
            report["candidate_sha256"], hashlib.sha256(frozen[0]).hexdigest()
        )


if __name__ == "__main__":
    unittest.main()
