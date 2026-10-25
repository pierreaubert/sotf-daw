"""Independent scalar inference and observation-contract checks."""

import hashlib
import json
import math
import tempfile
import unittest
from pathlib import Path

import numpy as np
from aec_residual_model import (
    BANDS,
    FEATURES,
    FLOOR,
    FORMAT,
    HIDDEN,
    ResidualModel,
    apply_masks,
)


def payload():
    w1 = np.zeros((HIDDEN, FEATURES))
    w1[0, 0] = 2
    return {
        "format": FORMAT,
        "feature_version": 1,
        "bands": BANDS,
        "features": FEATURES,
        "hidden": HIDDEN,
        "gain_floor": FLOOR,
        "w1": w1.tolist(),
        "b1": [0.1] + [0.0] * (HIDDEN - 1),
        "w2": [0.7] + [0.0] * (HIDDEN - 1),
        "b2": -0.3,
    }


class ModelTests(unittest.TestCase):
    def load(self, value):
        root = tempfile.TemporaryDirectory()
        self.addCleanup(root.cleanup)
        path = Path(root.name) / "model.json"
        raw = json.dumps(value).encode()
        path.write_bytes(raw)
        return ResidualModel(path, hashlib.sha256(raw).hexdigest())

    def test_predictions_match_scalar_tanh_and_sigmoid(self):
        model = self.load(payload())
        x = np.full((3, BANDS, FEATURES), 0.25, np.float32)
        expected = FLOOR + (1 - FLOOR) / (1 + math.exp(-(0.7 * math.tanh(0.6) - 0.3)))
        np.testing.assert_allclose(model.predict(x), expected, atol=1e-7)

    def test_invalid_contract_arrays_and_features_are_rejected(self):
        for key in ["hidden", "bands", "features", "feature_version", "gain_floor"]:
            value = payload()
            value[key] = 99
            with self.assertRaises(ValueError):
                self.load(value)
        for value in [101.0, float("nan"), float("inf")]:
            body = payload()
            body["w1"][0][0] = value
            with self.assertRaises(ValueError):
                self.load(body)
        model = self.load(payload())
        for value in [-0.01, 1.01, float("nan")]:
            features = np.full((BANDS, FEATURES), value)
            with self.assertRaises(ValueError):
                model.predict(features)

    def test_no_reference_keeps_exact_time_domain_adaptive_signal(self):
        model = self.load(payload())
        source = np.random.default_rng(20274025).normal(size=(4, 256))
        spectrum = np.fft.rfft(np.column_stack([np.zeros_like(source), source]), axis=1)
        features = np.full((4, BANDS, FEATURES), 0.5)
        features[:, :, 9] = 0
        output, gains = apply_masks(spectrum, features, model, 48000)
        np.testing.assert_array_equal(gains, 1)
        np.testing.assert_allclose(output, source.reshape(-1), atol=1e-14)


if __name__ == "__main__":
    unittest.main()
