"""Source-estimator derivatives, symmetry and full native conditional-trace checks."""

import hashlib
import json
import os
import tempfile
import unittest
from pathlib import Path

import numpy as np
from source_remix_model import export, features, load, objective, predict
from source_remix_native import reference, remix_parameters, render

RENDERER = os.environ.get("SOTF_SOURCE_REMIX_RENDERER")
MODEL = (
    Path(__file__).resolve().parents[2]
    / "crates/sotf-plugins/models/source-aware-remix/model.synthetic.json"
)


class ModelTests(unittest.TestCase):
    def test_gradient_matches_independent_central_differences(self):
        rng = np.random.default_rng(20271122)
        theta = rng.normal(0, 0.2, 65)
        x = rng.random((17, 6))
        y = rng.random(17)
        _, actual = objective(theta, x, y)
        expected = np.empty_like(theta)
        for k in range(len(theta)):
            delta = np.zeros_like(theta)
            delta[k] = 1e-6
            expected[k] = (
                objective(theta + delta, x, y)[0] - objective(theta - delta, x, y)[0]
            ) / 2e-6
        np.testing.assert_allclose(actual, expected, atol=1e-9, rtol=1e-6)

    def test_features_are_stereo_swap_and_global_phase_invariant(self):
        rng = np.random.default_rng(20271123)
        l = rng.normal(size=31) + 1j * rng.normal(size=31)
        r = rng.normal(size=31) + 1j * rng.normal(size=31)
        f = rng.random(31)
        expected = features(l, r, f)
        np.testing.assert_allclose(features(r, l, f), expected, atol=1e-14, rtol=0)
        np.testing.assert_allclose(
            features(l * np.exp(0.7j), r * np.exp(0.7j), f), expected, atol=1e-14
        )
        for bad in [np.zeros(31), np.full(31, np.nan), l * 1e10]:
            with self.assertRaises(ValueError):
                features(bad, bad, f)

    def test_hash_policy_provenance_shape_and_coefficients_reject(self):
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / "model.json"
            data = export(np.zeros(65))

            def read(d):
                path.write_text(json.dumps(d))
                return load(path, hashlib.sha256(path.read_bytes()).hexdigest())

            theta = read(data)
            np.testing.assert_array_equal(predict(theta, np.zeros((3, 6))), 0.5)
            with self.assertRaises(ValueError):
                load(path, "0" * 64)
            for key, value in [
                ("deployment_admitted", True),
                ("provenance", {}),
                ("policy", {}),
                ("weights", [[0] * 8] * 5),
                ("output_bias", 17),
                ("output_bias", True),
            ]:
                with self.assertRaises(ValueError):
                    read(dict(data, **{key: value}))


@unittest.skipUnless(RENDERER, "Set SOTF_SOURCE_REMIX_RENDERER")
class NativeTests(unittest.TestCase):
    def test_conditional_trace_and_callback_drain_reopening_match_actual_output(self):
        rng = np.random.default_rng(20271124)
        x = rng.normal(0, 0.02, (5101, 6)).astype(np.float32)
        identity = hashlib.sha256(MODEL.read_bytes()).hexdigest()
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            a, m = render(
                x, RENDERER, root / "a", remix_parameters(), model=MODEL, sha=identity
            )
            b, n = render(
                x,
                RENDERER,
                root / "b",
                m["remix_parameters"],
                model=MODEL,
                sha=identity,
                callback=17,
                drain=1,
            )
            np.testing.assert_array_equal(a["output"], b["output"])
            np.testing.assert_array_equal(a["gains"], b["gains"])
            expected = reference(
                a["baseline"], m["analysis_window"], a["gains"], len(a["output"])
            )
            np.testing.assert_allclose(a["output"], expected, atol=5e-8, rtol=0)
            self.assertEqual(m["counts"], n["counts"])

    def test_rejected_model_and_unsupported_clock_keep_neutral_delayed_downmix(self):
        rng = np.random.default_rng(20271125)
        x = rng.normal(0, 0.02, (5101, 6)).astype(np.float32)
        identity = hashlib.sha256(MODEL.read_bytes()).hexdigest()
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            a, m = render(
                x, RENDERER, root / "bad", remix_parameters(), model=MODEL, sha="0" * 64
            )
            b, _ = render(x, RENDERER, root / "neutral", remix_parameters("bypass"))
            np.testing.assert_array_equal(a["output"], b["output"])
            self.assertFalse(m["candidate_loaded"])
            a, m = render(
                x,
                RENDERER,
                root / "unsupported",
                remix_parameters(),
                model=MODEL,
                sha=identity,
                rate=192000,
            )
            b, _ = render(
                x,
                RENDERER,
                root / "unsupported-neutral",
                remix_parameters("bypass"),
                rate=192000,
            )
            np.testing.assert_array_equal(a["output"], b["output"])
            self.assertGreater(m["counts"]["unavailable_bins"], 0)


if __name__ == "__main__":
    unittest.main()
