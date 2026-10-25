"""Independent feature arithmetic, derivative, identity and source checks."""

import hashlib
import json
import os
import tempfile
import unittest
from pathlib import Path

import numpy as np
from expander_mask_model import export, features, load, objective, predict
from expander_mask_native import reference, render
from train_expander_mask import scene

RENDERER = os.environ.get("SOTF_EXPANDER_MASK_RENDERER")
MODEL = (
    Path(__file__).resolve().parents[2]
    / "crates/sotf-plugins/models/expander-denoising-mask/model.synthetic.json"
)


class ExpanderMaskTests(unittest.TestCase):
    def test_features_match_scalar_replicate_edge_reference(self):
        rng = np.random.default_rng(20275118)
        power, previous = 10 ** rng.uniform(-12, -1, (2, 513))
        actual = features(power, previous)
        for k in [0, 1, 17, 256, 511, 512]:
            local = [max(power[min(512, max(0, k + j))], 1e-20) for j in range(-4, 5)]
            arithmetic = sum(local) / 9
            geometric = np.exp(sum(np.log(v) for v in local) / 9)
            expected = np.clip(
                [
                    (10 * np.log10(power[k]) + 100) / 100,
                    (10 * np.log10(power[k] / arithmetic) + 20) / 40,
                    geometric / arithmetic,
                    (10 * np.log10(power[k] / previous[k]) + 30) / 60,
                    k / 512,
                    (10 * np.log10(sum(power) / 513) + 100) / 100,
                ],
                0,
                1,
            )
            np.testing.assert_allclose(actual[k], expected, atol=1e-14, rtol=0)
        for bad in [np.zeros(513), np.full(513, np.nan), np.full(513, 4097)]:
            with self.assertRaises(ValueError):
                features(bad, previous)

    def test_gradient_matches_central_difference_and_domains_reject(self):
        rng = np.random.default_rng(20275117)
        theta, x, y = rng.normal(0, 0.2, 65), rng.random((23, 6)), rng.random(23)
        _, actual = objective(theta, x, y)
        expected = []
        for i in range(65):
            delta = np.zeros(65)
            delta[i] = 1e-6
            expected.append(
                (objective(theta + delta, x, y)[0] - objective(theta - delta, x, y)[0])
                / 2e-6
            )
        np.testing.assert_allclose(actual, expected, atol=1e-9, rtol=1e-6)
        np.testing.assert_array_equal(predict(np.zeros(65), x), 0.5)
        with self.assertRaises(ValueError):
            predict(theta, np.full((1, 6), np.nan))
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / "model.json"

            def read(d):
                path.write_text(json.dumps(d))
                return load(path, hashlib.sha256(path.read_bytes()).hexdigest())

            data = export(theta)
            np.testing.assert_array_equal(read(data), theta)
            with self.assertRaises(ValueError):
                load(path, "0" * 64)
            for key, value in [
                ("format", "unknown"),
                ("provenance", "unknown"),
                ("deployment_admitted", True),
                ("weights", [[0] * 8] * 5),
                ("output_bias", True),
                ("output_bias", 17),
            ]:
                with self.assertRaises(ValueError):
                    read(dict(data, **{key: value}))

    def test_sampled_source_rows_reproduce_and_shift_identities_change(self):
        a, y, metadata = scene(20275116, rows=17)
        b, z, repeated = scene(20275116, rows=17)
        np.testing.assert_array_equal(a, b)
        np.testing.assert_array_equal(y, z)
        self.assertEqual(metadata, repeated)
        _, zero, shifted = scene(20275116, kind="noise_only", rows=17)
        np.testing.assert_array_equal(zero, 0)
        self.assertNotEqual(metadata["clean_sha256"], shifted["clean_sha256"])
        self.assertNotEqual(metadata["mixture_sha256"], shifted["mixture_sha256"])


@unittest.skipUnless(RENDERER, "Set SOTF_EXPANDER_MASK_RENDERER")
class NativeExpanderTests(unittest.TestCase):
    def test_native_trace_replays_and_callback_drain_boundaries_are_identical(self):
        x = np.random.default_rng(20275114).normal(0, 0.02, (5101, 2)).astype("<f4")
        sha = hashlib.sha256(MODEL.read_bytes()).hexdigest()
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            a, ga, ma = render(x, RENDERER, root / "a", model=MODEL, sha=sha)
            b, gb, mb = render(
                x, RENDERER, root / "b", model=MODEL, sha=sha, callback=17, drain=1
            )
            np.testing.assert_array_equal(a, b)
            np.testing.assert_array_equal(ga, gb)
            self.assertEqual(ma["source_counters"], mb["source_counters"])
            expected = reference(x, ma["analysis_window"], ga, len(a))
            np.testing.assert_allclose(a, expected, atol=2e-7, rtol=0)

    def test_rejected_model_and_unsupported_clock_keep_exact_classical_output(self):
        x = np.random.default_rng(20275113).normal(0, 0.02, (3101, 1)).astype("<f4")
        sha = hashlib.sha256(MODEL.read_bytes()).hexdigest()
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            for rate in [48000, 192000]:
                expected, _, _ = render(
                    x, RENDERER, root / f"baseline-{rate}", enabled=False, rate=rate
                )
                for name, identity in [("rejected", "0" * 64), ("valid", sha)]:
                    if name == "valid" and rate != 192000:
                        continue
                    actual, _, metadata = render(
                        x,
                        RENDERER,
                        root / f"{name}-{rate}",
                        model=MODEL,
                        sha=identity,
                        rate=rate,
                    )
                    np.testing.assert_array_equal(actual, expected)
                    self.assertEqual(metadata["source_counters"]["selected_bins"], 0)
                    self.assertEqual(metadata["model_loaded"], name == "valid")


if __name__ == "__main__":
    unittest.main()
