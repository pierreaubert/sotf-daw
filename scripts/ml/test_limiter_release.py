"""Release model loading, input contracts and identity/feature invariants."""

import hashlib
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import numpy as np
from fit_limiter_release import target
from limiter_native import PARAMS
from limiter_release import FEATURES, FORMAT, Model, descriptors, validate_pair
from limiter_release_cases import signal


class ReleaseTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.fixed = {k: v for k, v in PARAMS.items() if k != "release_ms"}

    def model_data(self):
        return {
            "format": FORMAT,
            "features": FEATURES,
            "layers": [
                {"weight": np.zeros((32, FEATURES)).tolist(), "bias": [0.0] * 32},
                {"weight": [[0.0] * 32], "bias": [0.0]},
            ],
        }

    def load(self, data):
        p = self.root / "model.json"
        p.write_text(json.dumps(data))
        return Model(p, hashlib.sha256(p.read_bytes()).hexdigest())

    def test_bounded_prediction_and_layers(self):
        model = self.load(self.model_data())
        self.assertEqual(model.predict_features(np.zeros(FEATURES)), 100.0)
        with self.assertRaises(ValueError):
            model.predict_features([0] * (FEATURES - 1))
        data = self.model_data()
        data["layers"][0]["weight"][0].pop()
        with self.assertRaises(ValueError):
            self.load(data)

    def test_hash_size_and_nonfinite_rejected(self):
        p = self.root / "bad.json"
        p.write_text("x" * 262145)
        with self.assertRaises(ValueError):
            Model(p, hashlib.sha256(p.read_bytes()).hexdigest())
        self.load(self.model_data())
        with self.assertRaises(ValueError):
            Model(self.root / "model.json", "0" * 64)
        data = self.model_data()
        data["layers"][0]["bias"][0] = float("nan")
        with self.assertRaises(ValueError):
            self.load(data)

    def test_hash_and_parse_use_same_bytes(self):
        p = self.root / "model.json"
        p.write_text(json.dumps(self.model_data()))
        digest = hashlib.sha256(p.read_bytes()).hexdigest()
        parse = json.loads

        def replacing_parse(raw):
            p.write_text("invalid replacement")
            return parse(raw)

        with patch("limiter_release.json.loads", side_effect=replacing_parse):
            loaded = Model(p, digest)
        self.assertEqual(loaded.digest, digest)
        self.assertEqual(loaded.predict_features(np.zeros(FEATURES)), 100.0)

    def test_pair_geometry_and_profile(self):
        x = np.ones((96000, 1), np.float32)
        validate_pair(x, x, 48000, self.fixed)
        for fixed in [
            self.fixed | {"oversampling": 1},
            self.fixed | {"dual_release": True},
            self.fixed | {"threshold_db": True},
            self.fixed | {"mix": 0.5},
            self.fixed | {"release_ms": 50},
        ]:
            with self.assertRaises(ValueError):
                validate_pair(x, x, 48000, fixed)
        with self.assertRaises(ValueError):
            validate_pair(x[:100], x[:100], 48000, self.fixed)
        with self.assertRaises(ValueError):
            validate_pair(x, x[:, 0:0], 48000, self.fixed)

    def test_finite_silent_features(self):
        x = np.zeros((96000, 1), np.float32)
        features, analytic = descriptors(x, x, 48000, self.fixed)
        self.assertEqual(features.shape, (FEATURES,))
        self.assertTrue(np.isfinite(features).all())
        self.assertEqual(analytic, 100.0)

    def test_signal_identity_determinism_and_split(self):
        a = signal(1, 0)
        b = signal(1, 0)
        c = signal(2, 0)
        self.assertEqual(a[2], b[2])
        self.assertNotEqual(a[2], c[2])
        np.testing.assert_array_equal(a[0], b[0])
        self.assertEqual(a[3], b[3])

    def test_target_reads_exact_raw_bytes_and_hashes(self):
        x = np.full((96000, 1), 0.01, np.float32)
        x.astype("<f4").tofile(self.root / "dry.f32le")
        x.astype("<f4").tofile(self.root / "reference.f32le")
        data = {
            "format": "sotf-limiter-release-target-v1",
            "alignment": "latency_removed",
            "sample_rate": 48000,
            "frames": len(x),
            "channels": 1,
            "dry": "dry.f32le",
            "reference": "reference.f32le",
            "fixed_params": self.fixed,
        }
        path = self.root / "target.json"
        path.write_text(json.dumps(data))
        _, dry, reference, hashes = target(path)
        np.testing.assert_array_equal(dry, reference)
        self.assertEqual(
            hashes["dry"],
            hashlib.sha256((self.root / "dry.f32le").read_bytes()).hexdigest(),
        )
        with (self.root / "dry.f32le").open("ab") as stream:
            stream.write(b"x")
        with self.assertRaises(ValueError):
            target(path)


if __name__ == "__main__":
    unittest.main()
