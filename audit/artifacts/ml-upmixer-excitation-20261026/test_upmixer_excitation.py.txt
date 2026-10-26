"""The development gate must expose lost energy and threshold disagreement."""

import json
import unittest

import numpy as np
from develop_upmixer_excitation import assess


class ExcitationTests(unittest.TestCase):
    def test_raw_power_distinguishes_bands_below_the_clipped_feature_floor(self):
        reference = np.zeros((1, 2, 6))
        native = reference.copy()
        native[0, 1, 0] = 0.5
        valid = np.ones((1, 2), dtype=bool)
        power = np.array([[1e-10, 1e-15]])
        clipped = assess(reference, valid, native, valid, power, 40)
        raw = assess(reference, valid, native, valid, power, 40, power)
        self.assertFalse(clipped["pass"])
        self.assertTrue(raw["pass"])
        self.assertEqual(raw["selected_band_frames"], 1)
        self.assertIn('"pass": true', json.dumps(raw, allow_nan=False))

    def test_dark_covariance_error_can_be_excluded_without_discarding_energy(self):
        reference = np.zeros((1, 2, 6))
        reference[0, :, 5] = [0.9, 0.1]
        native = reference.copy()
        native[0, 1, 0] = 0.9
        valid = np.ones((1, 2), dtype=bool)
        result = assess(reference, valid, native, valid, np.array([[1, 1e-8]]), 60)
        self.assertTrue(result["pass"])
        self.assertEqual(result["selected_band_frames"], 1)
        self.assertGreater(result["retained_reference_band_energy_fraction"], 0.999)

    def test_empty_and_excessively_selective_policies_do_not_pass(self):
        reference = np.zeros((1, 2, 6))
        reference[0, :, 5] = [0.9, 0.1]
        valid = np.ones((1, 2), dtype=bool)
        for features, power in [
            (reference, np.array([[1.0, 0.1]])),
            (reference * 0, np.ones((1, 2))),
        ]:
            self.assertFalse(
                assess(features, valid, features, valid, power, 60)["pass"]
            )
        silent = np.zeros_like(reference)
        self.assertTrue(
            assess(silent, valid & False, silent, valid & False, np.zeros((1, 2)), 60)[
                "pass"
            ]
        )

    def test_cutoff_disagreement_and_retained_error_remain_failures(self):
        reference = np.zeros((1, 2, 6))
        reference[0, :, 5] = [0.9, 0.300001]
        native = reference.copy()
        native[0, 1, 5] = 0.299999
        valid = np.ones((1, 2), dtype=bool)
        power = np.array([[1.0, 1e-6]])
        result = assess(reference, valid, native, valid, power, 60)
        self.assertFalse(result["pass"])
        self.assertEqual(result["validity_disagreements"], 1)
        native = reference.copy()
        native[0, 0, 0] = 0.001
        self.assertFalse(assess(reference, valid, native, valid, power, 60)["pass"])


if __name__ == "__main__":
    unittest.main()
