"""Independent derivatives, feature invariance and frozen mask contract checks."""

import hashlib
import json
import os
import tempfile
import unittest
from pathlib import Path

import numpy as np
from beamformer_mask_native import parameters, render
from beamformer_noise_mask import export, features, load, loss_gradient, predict
from train_beamformer_noise_mask import scene

RENDERER = os.environ.get("SOTF_BEAMFORMER_MASK_RENDERER")
MODEL = (
    Path(__file__).resolve().parents[2]
    / "crates/sotf-plugins/models/beamformer-noise-mask/model.synthetic.json"
)


class MaskTests(unittest.TestCase):
    def test_gradient_matches_independent_finite_differences(self):
        rng = np.random.default_rng(20261121)
        x, y = rng.random((13, 6)), rng.random(13)
        theta = rng.normal(0, 0.2, 65)
        _, actual = loss_gradient(theta, x, y)
        expected = np.empty_like(theta)
        for k in range(len(theta)):
            delta = np.zeros_like(theta)
            delta[k] = 1e-6
            expected[k] = (
                loss_gradient(theta + delta, x, y)[0]
                - loss_gradient(theta - delta, x, y)[0]
            ) / (2e-6)
        np.testing.assert_allclose(actual, expected, atol=1e-9, rtol=1e-6)

    def test_features_preserve_global_gain_and_phase_but_reject_bad_evidence(self):
        rng = np.random.default_rng(20261122)
        x = rng.normal(size=(33, 4)) + 1j * rng.normal(size=(33, 4))
        d = np.exp(1j * rng.normal(size=x.shape))
        f = rng.random(33)
        expected = features(x, d, f)
        for gain in [1e-6, 1, 1e4]:
            np.testing.assert_allclose(
                features(x * gain * np.exp(0.7j), d, f), expected, atol=1e-14
            )
        for bad in [np.zeros_like(x), np.full_like(x, np.nan), x * 1e10]:
            with self.assertRaises(ValueError):
                features(bad, d, f)
        with self.assertRaises(ValueError):
            features(x, d * 2, f)

    def test_disjoint_scene_identity_and_correlated_shift_are_explicit(self):
        x, y, row = scene(20261120)
        other, _, other_row = scene(20262120)
        self.assertEqual(x.shape, (256, 6))
        self.assertTrue(np.all((y >= 0) & (y <= 1)))
        self.assertNotEqual(row["features_sha256"], other_row["features_sha256"])
        self.assertFalse(np.array_equal(x, other))

    def test_hash_bounds_dimensions_and_deployment_refusals(self):
        with tempfile.TemporaryDirectory() as folder:
            p = Path(folder) / "model.json"
            value = export(np.zeros(65))

            def read(v):
                p.write_text(json.dumps(v))
                return load(p, hashlib.sha256(p.read_bytes()).hexdigest())

            theta = read(value)
            np.testing.assert_array_equal(predict(theta, np.zeros((3, 6))), 0.5)
            with self.assertRaises(ValueError):
                load(p, "0" * 64)
            for key, replacement in [
                ("deployment_admitted", True),
                ("weights", [[0] * 8] * 5),
                ("output_bias", 17),
                ("output_bias", True),
                ("provenance", {}),
                ("unknown", True),
            ]:
                bad = dict(value, **{key: replacement})
                with self.assertRaises(ValueError):
                    read(bad)


@unittest.skipUnless(RENDERER, "Set SOTF_BEAMFORMER_MASK_RENDERER")
class NativeTests(unittest.TestCase):
    def test_broadside_target_identity_and_json_callback_reopen_at_three_rates(self):
        rng = np.random.default_rng(20261124)
        source = rng.normal(0, 0.01, 7101).astype(np.float32)
        identity = hashlib.sha256(MODEL.read_bytes()).hexdigest()
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            for rate in [16000, 48000, 96000]:
                x = np.tile(source[:, None], (1, 4))
                y, m = render(
                    x,
                    RENDERER,
                    root / str(rate),
                    parameters(),
                    model=MODEL,
                    sha=identity,
                    enabled=True,
                    rate=rate,
                )
                np.testing.assert_array_equal(y[:512], 0)
                np.testing.assert_allclose(
                    y[512 : 512 + len(source)], source, atol=3e-8, rtol=0
                )
                reopened, _ = render(
                    x,
                    RENDERER,
                    root / (str(rate) + "-reopen"),
                    m["effective_parameters"],
                    model=MODEL,
                    sha=identity,
                    enabled=True,
                    rate=rate,
                    callback=17,
                )
                np.testing.assert_array_equal(y, reopened)

    def test_rejected_hash_and_outside_context_keep_native_fallback(self):
        rng = np.random.default_rng(20261125)
        x = rng.normal(0, 0.02, (5101, 4)).astype(np.float32)
        identity = hashlib.sha256(MODEL.read_bytes()).hexdigest()
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            expected, _ = render(
                x, RENDERER, root / "missing", parameters(), enabled=True
            )
            y, m = render(
                x,
                RENDERER,
                root / "rejected",
                parameters(),
                model=MODEL,
                sha="0" * 64,
                enabled=True,
            )
            self.assertFalse(m["candidate_loaded"])
            self.assertIsNotNone(m["load_error"])
            np.testing.assert_array_equal(y, expected)
            y, m = render(
                x,
                RENDERER,
                root / "unsupported",
                parameters(),
                model=MODEL,
                sha=identity,
                enabled=True,
                rate=44100,
            )
            expected, _ = render(
                x,
                RENDERER,
                root / "unsupported-baseline",
                parameters(),
                enabled=True,
                rate=44100,
            )
            np.testing.assert_array_equal(y, expected)
            self.assertGreater(m["counts"]["unavailable_frames"], 0)


if __name__ == "__main__":
    unittest.main()
