"""Independent inference equations and exact-verification control-flow tests."""

import copy
import hashlib
import json
import math
import tempfile
import unittest
from itertools import pairwise
from pathlib import Path

import numpy as np
from aae_room_features import (
    CONTROL_SPEC,
    FEATURE_SPEC,
    RATE,
    describe,
    encode,
    parameters,
)
from aae_room_model import Surrogate
from fit_aae_room import fit


def payload():
    dimensions = [19, 128, 128, 224]
    layers = []
    for low, high in pairwise(dimensions):
        layers.append(
            {
                "weight": np.zeros((high, low), dtype=float).tolist(),
                "bias": np.zeros(high).tolist(),
            }
        )
    layers[0]["weight"][0][0] = 2.0
    layers[0]["bias"][0] = -0.1
    layers[1]["weight"][0][0] = 0.5
    for row in layers[2]["weight"]:
        row[0] = 0.25
    layers[2]["bias"] = [-2.0] * 224
    return {
        "format": "sotf_aae_monotone_forward_v1",
        "feature_spec": FEATURE_SPEC,
        "control_spec": CONTROL_SPEC,
        "architecture": {
            "input": 19,
            "hidden": [128, 128],
            "hidden_activation": "silu",
            "output": "negative_cumsum_softplus_clamp40",
            "curve_shape": [7, 32],
        },
        "layers": layers,
    }


class AnalyticRenderer:
    """A different analytic system exercises optimizer admission, not AAE quality."""

    def __init__(self):
        self.calls = 0
        self.observation_seconds = []

    def render(self, unit, layout, preset, *, rate, seconds=8):
        self.calls += 1
        self.observation_seconds.append(seconds)
        p = parameters(unit, layout, preset)
        frame = np.arange(round(rate * seconds))
        delay = round(p["pre_delay_ms"] * rate / 1000)
        time = np.maximum(frame - delay, 0) / rate
        audio = np.exp(-3 * math.log(10) * time / p["rt60"])
        audio[:delay] = 0
        return audio[:, None], {
            "params": p,
            "sha256": str(self.calls),
            "native_tail": "Infinite",
        }


class ModelTests(unittest.TestCase):
    def load(self, body):
        root = tempfile.TemporaryDirectory()
        self.addCleanup(root.cleanup)
        path = Path(root.name) / "model.json"
        raw = json.dumps(body).encode()
        path.write_bytes(raw)
        return Surrogate(path, hashlib.sha256(raw).hexdigest())

    def test_silu_and_monotone_decoder_match_scalar_equations(self):
        model = self.load(payload())
        encoded = encode(np.full(6, 0.5), "5.0", "medium")
        output = model.predict(encoded).reshape(7, 32)
        silu = lambda x: x / (1 + math.exp(-x))
        raw = 0.25 * silu(0.5 * silu(0.9)) - 2.0
        increment = math.log1p(math.exp(raw))
        expected = -np.arange(1, 33) * increment
        np.testing.assert_allclose(output, np.tile(expected, (7, 1)), atol=5e-6)
        self.assertTrue(np.all(np.diff(output, axis=1) <= 0))
        self.assertTrue(np.all((output >= -40) & (output <= 0)))

    def test_contract_and_numeric_weight_corruption_are_rejected(self):
        for change in [
            lambda p: p["feature_spec"].update(version=2),
            lambda p: p["control_spec"].update(log_coordinates=[]),
            lambda p: p["layers"][0]["weight"][0].__setitem__(0, float("nan")),
            lambda p: p["layers"][0]["weight"][0].__setitem__(0, 101.0),
        ]:
            body = copy.deepcopy(payload())
            change(body)
            with self.assertRaises(ValueError):
                self.load(body)

    def test_soft_categories_are_rejected(self):
        model = self.load(payload())
        encoded = encode(np.full(6, 0.5), "5.0", "medium")
        encoded[6:15] = 0
        encoded[6:8] = 0.5
        with self.assertRaises(ValueError):
            model.predict(encoded)

    def test_malformed_hashed_model_structure_fails_as_validation_error(self):
        for body in [[], None, "model", 42]:
            with self.assertRaises((ValueError, TypeError)):
                self.load(body)
        for layers in [None, {}, [None] * 3, [{"bias": []}] * 3]:
            body = payload()
            body["layers"] = layers
            with self.assertRaises((ValueError, TypeError)):
                self.load(body)

    def test_native_budget_is_bounded_and_only_observed_scores_select_output(self):
        source = np.exp(-3 * math.log(10) * np.arange(RATE * 8) / RATE / 0.9)
        target = describe(source, RATE)
        renderer = AnalyticRenderer()
        report = fit(target, renderer, layout="5.0", rate=RATE, budget=8)
        self.assertLessEqual(renderer.calls, 8)
        self.assertEqual(renderer.calls, len(report["trace"]))
        self.assertEqual(
            report["best"]["mse_db2"], min(row["mse_db2"] for row in report["trace"])
        )

    def test_flat_unobserved_decay_cannot_qualify_a_fit(self):
        target = describe(np.ones(RATE), RATE)
        renderer = AnalyticRenderer()
        with self.assertRaisesRegex(ValueError, "observed decay"):
            fit(target, renderer, layout="5.0", rate=RATE, budget=8)
        self.assertEqual(renderer.calls, 0)

    def test_native_energy_integration_uses_only_the_measured_window(self):
        source = np.exp(-3 * math.log(10) * np.arange(RATE) / RATE / 0.3)
        target = describe(source, RATE)
        renderer = AnalyticRenderer()
        fit(target, renderer, layout="5.0", rate=RATE, budget=8)
        self.assertGreater(renderer.calls, 0)
        self.assertEqual(renderer.observation_seconds, [1.0] * renderer.calls)

    def test_perfect_surrogate_claim_does_not_bypass_native_rejection(self):
        source = np.zeros(RATE * 8)
        source[200] = 1
        target = describe(source, RATE)

        class Liar:
            def predict(self, x):
                return (
                    target["features"].copy()
                    if x.ndim == 1
                    else np.tile(target["features"], (len(x), 1))
                )

        renderer = AnalyticRenderer()
        report = fit(target, renderer, layout="5.0", rate=RATE, budget=8, model=Liar())
        self.assertEqual(report["method"], "neural_plus_native")
        self.assertGreater(report["profile_rmse_db"], 3.0)
        self.assertFalse(report["qualified_profile_proxy"])


if __name__ == "__main__":
    unittest.main()
