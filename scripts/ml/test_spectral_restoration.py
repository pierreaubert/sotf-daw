"""Actual native spectral restoration, independent WOLA and fallback checks."""

import hashlib
import os
import tempfile
import unittest
from pathlib import Path

import numpy as np
from spectral_restoration_native import parameters, reference, render

RENDERER = os.environ.get("SOTF_SPECTRAL_RESTORATION_RENDERER")
MODEL = (
    Path(__file__).resolve().parents[2]
    / "crates/sotf-plugins/models/expander-denoising-mask/model.synthetic.json"
)


@unittest.skipUnless(RENDERER, "Set SOTF_SPECTRAL_RESTORATION_RENDERER")
class NativeSpectralRestorationTests(unittest.TestCase):
    def test_callback_and_drain_partitions_preserve_actual_output_and_trace(self):
        x = np.random.default_rng(20279119).normal(0, 0.02, (5101, 2)).astype("<f4")
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
            np.testing.assert_allclose(
                a, reference(x, ma["analysis_window"], ga, len(a)), atol=2e-7, rtol=0
            )

    def test_missing_rejected_and_unsupported_models_preserve_classical_stream(self):
        x = np.random.default_rng(20279118).normal(0, 0.02, (3101, 1)).astype("<f4")
        sha = hashlib.sha256(MODEL.read_bytes()).hexdigest()
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            for rate in [48000, 192000]:
                expected, _, _ = render(
                    x, RENDERER, root / f"baseline-{rate}", enabled=False, rate=rate
                )
                variants = [("missing", None, None), ("rejected", MODEL, "0" * 64)]
                if rate == 192000:
                    variants.append(("unsupported", MODEL, sha))
                for name, path, identity in variants:
                    actual, _, metadata = render(
                        x,
                        RENDERER,
                        root / f"{name}-{rate}",
                        model=path,
                        sha=identity,
                        rate=rate,
                    )
                    np.testing.assert_array_equal(actual, expected)
                    self.assertEqual(metadata["source_counters"]["selected_bins"], 0)

    def test_unity_profile_reconstructs_delayed_source_including_finite_tail(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            for rate in [44100, 48000, 96000]:
                x = (
                    np.random.default_rng(rate + 19)
                    .normal(0, 0.02, (4117, 2))
                    .astype("<f4")
                )
                actual, gains, metadata = render(
                    x,
                    RENDERER,
                    root / str(rate),
                    enabled=False,
                    rate=rate,
                    params=parameters(ratio=1.0),
                )
                expected = np.zeros_like(actual)
                expected[1024 : 1024 + len(x)] = x
                np.testing.assert_allclose(actual, expected, atol=1e-7, rtol=0)
                np.testing.assert_array_equal(gains, 1)
                self.assertEqual(metadata["latency_samples"], 1024)


if __name__ == "__main__":
    unittest.main()
