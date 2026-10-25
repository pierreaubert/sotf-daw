"""Independent analytic checks of the recorded-source measurement operator."""

import hashlib
import tempfile
import unittest
from pathlib import Path

import numpy as np
from evaluate_aec_recorded import decompose, fixture


class RecordedMeasurementTests(unittest.TestCase):
    def test_uniform_half_gain_has_six_db_projection_loss(self):
        rng = np.random.default_rng(40)
        near = rng.normal(0, 0.01, 512).astype("<f4")
        padded = np.pad(near.astype(float).reshape(-1, 256), ((0, 0), (256, 0)))
        spectrum = np.fft.rfft(padded, axis=1)
        pairs = np.stack([spectrum.real, spectrum.imag], axis=-1)
        with tempfile.TemporaryDirectory() as directory:
            metadata = {"directory": directory, "output_stage": "hybrid"}
            for name, data in {
                "effective_gains": np.full((2, 257), 0.5),
                "error_spectra": pairs,
                "hybrid": near * 0.5,
            }.items():
                raw = np.asarray(data, dtype="<f4").tobytes()
                (Path(directory) / (name + ".f32le")).write_bytes(raw)
                metadata[name] = {
                    "sha256": hashlib.sha256(raw).hexdigest(),
                    "floats": len(raw) // 4,
                }
            output, conditional, error = decompose(metadata, near)
            self.assertLess(error, 1e-8)
            np.testing.assert_allclose(conditional, near * 0.5, atol=1e-10)
            self.assertAlmostEqual(
                -20
                * np.log10(
                    np.dot(conditional, near) / np.dot(near.astype(float), near)
                ),
                6.020599913,
                places=6,
            )
            np.testing.assert_array_equal(output, near * 0.5)
            # Coefficients with the wrong convention must fail reconstruction,
            # even when their stored byte hashes are internally consistent.
            wrong = np.ones((2, 257), dtype="<f4").tobytes()
            (Path(directory) / "effective_gains.f32le").write_bytes(wrong)
            metadata["effective_gains"]["sha256"] = hashlib.sha256(wrong).hexdigest()
            with self.assertRaisesRegex(ValueError, "reconstruction failed"):
                decompose(metadata, near)

    def test_reference_silent_recorded_fixture_and_resampling(self):
        sources = [np.sin(np.arange(192000) * omega) for omega in [0.01, 0.02, 0.03]]
        for rate in [16000, 44100, 48000]:
            arrays, metadata = fixture(sources, [0, 1], rate, "near_only", 10)
            self.assertEqual(metadata["frames"], 12 * rate // 256 * 256)
            self.assertTrue(np.isfinite(arrays["input"]).all())
            np.testing.assert_array_equal(arrays["echo"], 0)
            np.testing.assert_array_equal(arrays["input"][:, 1], 0)
            np.testing.assert_array_equal(arrays["input"][:, 0], arrays["near"])


if __name__ == "__main__":
    unittest.main()
