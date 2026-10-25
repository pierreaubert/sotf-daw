"""Causal feature, optimization and complete native music-mask regression tests."""

import hashlib
import json
import os
import tempfile
import unittest
from pathlib import Path

import numpy as np
from music_mask_native import parameters, render
from music_noise_mask import (
    analysis,
    export,
    feature_sequence,
    features,
    load,
    loss_gradient,
    predict,
)

RENDERER = os.environ.get("SOTF_MUSIC_MASK_RENDERER")
MODEL = (
    Path(__file__).resolve().parents[2]
    / "crates/sotf-plugins/models/music-noise-mask/model.synthetic.json"
)


class MaskTests(unittest.TestCase):
    def test_weighted_gain_gradient_matches_independent_central_differences(self):
        rng = np.random.default_rng(20265126)
        theta = rng.normal(0, 0.2, 65)
        x = rng.random((17, 6))
        y = rng.random(17)
        _, gradient = loss_gradient(theta, x, y)
        expected = np.empty_like(theta)
        for i in range(len(theta)):
            d = np.zeros_like(theta)
            d[i] = 1e-6
            expected[i] = (
                loss_gradient(theta + d, x, y)[0] - loss_gradient(theta - d, x, y)[0]
            ) / 2e-6
        np.testing.assert_allclose(gradient, expected, atol=1e-9, rtol=1e-6)

    def test_periodic_window_and_half_prefix_match_direct_dft(self):
        rng = np.random.default_rng(20265127)
        for fft in [512, 2048]:
            x = rng.normal(0, 0.01, fft * 2 + 17)
            actual = analysis(x, fft)[0]
            frame = np.r_[np.zeros(fft // 2), x[: fft // 2]]
            window = np.sqrt(0.5 - 0.5 * np.cos(2 * np.pi * np.arange(fft) / fft))
            for bin in [0, 17, fft // 2]:
                reference = np.dot(
                    frame * window, np.exp(-2j * np.pi * bin * np.arange(fft) / fft)
                )
                self.assertLess(abs(actual[bin] - reference), 1e-12)

    def test_feature_history_is_causal_and_rejects_bad_evidence(self):
        rng = np.random.default_rng(20265128)
        spectra = rng.normal(size=(11, 257)) + 1j * rng.normal(size=(11, 257))
        first = feature_sequence(spectra, 48000, 512)
        changed = spectra.copy()
        changed[7:] *= 100
        other = feature_sequence(changed, 48000, 512)
        np.testing.assert_array_equal(first[:7], other[:7])
        p = np.ones(257)
        for bad in [np.full(257, np.nan), -p, p * 1e13]:
            with self.assertRaises(ValueError):
                features(bad, p, 512)

    def test_hash_policy_provenance_dimensions_and_numeric_bounds_reject(self):
        with tempfile.TemporaryDirectory() as folder:
            p = Path(folder) / "model.json"
            data = export(np.zeros(65))

            def read(value):
                p.write_text(json.dumps(value))
                return load(p, hashlib.sha256(p.read_bytes()).hexdigest())

            theta = read(data)
            np.testing.assert_array_equal(predict(theta, np.zeros((3, 6))), 0.5)
            with self.assertRaises(ValueError):
                load(p, "0" * 64)
            for key, replacement in [
                ("deployment_admitted", True),
                ("provenance", {}),
                ("policy", {}),
                ("weights", [[0] * 8] * 5),
                ("output_bias", True),
                ("output_bias", 17),
            ]:
                with self.assertRaises(ValueError):
                    read(dict(data, **{key: replacement}))


@unittest.skipUnless(RENDERER, "Set SOTF_MUSIC_MASK_RENDERER")
class NativeTests(unittest.TestCase):
    def test_complete_native_render_reopens_across_callback_and_drain_capacities(self):
        rng = np.random.default_rng(20265129)
        x = rng.normal(0, 0.02, (7101, 2)).astype(np.float32)
        identity = hashlib.sha256(MODEL.read_bytes()).hexdigest()
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            for low in [False, True]:
                y, m = render(
                    x,
                    RENDERER,
                    root / str(low),
                    parameters(low),
                    model=MODEL,
                    sha=identity,
                    enabled=True,
                )
                other, _ = render(
                    x,
                    RENDERER,
                    root / (str(low) + "-reopen"),
                    m["effective_parameters"],
                    model=MODEL,
                    sha=identity,
                    enabled=True,
                    callback=17,
                    drain_capacity=1,
                )
                np.testing.assert_array_equal(y, other)
                self.assertGreater(m["counts"]["neural_bins"], 0)

    def test_rejected_model_and_unsupported_clock_keep_classical_audio(self):
        rng = np.random.default_rng(20265130)
        x = rng.normal(0, 0.02, (5101, 1)).astype(np.float32)
        identity = hashlib.sha256(MODEL.read_bytes()).hexdigest()
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            y, m = render(
                x,
                RENDERER,
                root / "bad-hash",
                parameters(),
                model=MODEL,
                sha="0" * 64,
                enabled=True,
            )
            expected, _ = render(
                x, RENDERER, root / "missing", parameters(), enabled=True
            )
            np.testing.assert_array_equal(y, expected)
            self.assertFalse(m["candidate_loaded"])
            y, m = render(
                x,
                RENDERER,
                root / "unsupported",
                parameters(),
                model=MODEL,
                sha=identity,
                enabled=True,
                rate=192000,
            )
            expected, _ = render(
                x,
                RENDERER,
                root / "unsupported-classical",
                parameters(),
                enabled=True,
                rate=192000,
            )
            np.testing.assert_array_equal(y, expected)
            self.assertGreater(m["counts"]["unavailable_bins"], 0)


if __name__ == "__main__":
    unittest.main()
