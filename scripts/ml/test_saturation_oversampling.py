"""Independent folded-spectrum indexing, reconstruction and convergence checks."""

import unittest

import numpy as np
from analyze_saturation_oversampling import ideal_fold, ratio


class SpectrumTests(unittest.TestCase):
    def test_negative_alias_bins_are_not_mistaken_for_legitimate_harmonics(self):
        power = np.zeros(4096)
        power[[999, 4096 - 999]] = 1
        power[[1998, 4096 - 1998]] = 0.5
        power[[1099, 4096 - 1099]] = 0.01
        self.assertAlmostEqual(ratio(power, 999), -20)
        self.assertAlmostEqual(ratio(power * 0.25, 999), -20)

    def test_continuous_harmonic_fold_reconstructs_direct_periodic_curve(self):
        for drive in [1, 20]:
            for fundamental in [401, 999]:
                phase = 2 * np.pi * fundamental * np.arange(4096) / 4096
                curve = np.tanh(0.7 * drive * np.sin(phase)) / np.tanh(drive)
                expected = abs(np.fft.fft(curve) / 4096) ** 2
                actual = ideal_fold(drive, fundamental)
                np.testing.assert_allclose(actual, expected, atol=1e-11, rtol=0)

    def test_ideal_oversampled_reference_converges_and_reduces_alias(self):
        for drive in [1, 4, 12, 20]:
            for fundamental in [401, 999]:
                dense = ideal_fold(drive, fundamental, factor=4)
                coarse = ideal_fold(drive, fundamental, factor=4, points=32768)
                np.testing.assert_allclose(dense, coarse, atol=1e-10, rtol=0)
                self.assertLess(
                    ratio(dense, fundamental),
                    ratio(ideal_fold(drive, fundamental), fundamental) - 5,
                )


if __name__ == "__main__":
    unittest.main()
