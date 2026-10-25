"""Analytic, gain, sampling and coverage oracles for AAE fitting descriptors."""

import math
import unittest

import numpy as np
from aae_room_features import (
    DIRECT_MASK_FRAMES,
    FLOOR_DB,
    RATE,
    TIME_FRAMES,
    describe,
    encode,
    parameters,
    resample_audio,
)


class Features(unittest.TestCase):
    def test_finite_exponential_edc_matches_closed_form(self):
        frames = RATE * 8
        rt60 = 0.9
        x = np.exp(-3.0 * math.log(10.0) * np.arange(frames) / (RATE * rt60))
        result = describe(x, RATE)
        r = math.exp(-6.0 * math.log(10.0) / (RATE * rt60))
        k = np.maximum(TIME_FRAMES, DIRECT_MASK_FRAMES)
        expected = np.maximum(
            10.0 * np.log10((r**k - r**frames) / (r**DIRECT_MASK_FRAMES - r**frames)),
            FLOOR_DB,
        )
        np.testing.assert_allclose(result["features"][:32], expected, atol=1e-10)
        self.assertAlmostEqual(result["t20_fits"][0]["rt60_seconds"], rt60, places=8)

    def test_gain_and_duplicate_channel_energy_invariance(self):
        rng = np.random.default_rng(20261025)
        x = rng.normal(size=RATE * 2) * np.exp(-np.arange(RATE * 2) / RATE * 7)
        a = describe(x, RATE)
        b = describe(np.column_stack([4 * x, 2 * x]), RATE)
        np.testing.assert_allclose(a["features"], b["features"], atol=1e-10)

    def test_short_records_are_masked_not_claimed_as_future_silence(self):
        x = np.exp(-np.arange(RATE) / RATE * 7)
        d = describe(x, RATE)
        self.assertEqual(d["observed_frames"], RATE)
        self.assertTrue(np.any(~d["mask"]))
        self.assertTrue(np.all(TIME_FRAMES[d["mask"][:32]] <= RATE * 0.75))

    def test_antialias_filter_rejects_12khz_before_48_to_16khz(self):
        time = np.arange(48000) / 48000
        high = resample_audio(np.sin(2 * np.pi * 12000 * time), 48000)[100:-100]
        low = resample_audio(np.sin(2 * np.pi * 1000 * time), 48000)[100:-100]
        self.assertLess(np.sqrt(np.mean(high**2)), 1e-4)
        self.assertGreater(np.sqrt(np.mean(low**2)), 0.70)

    def test_invalid_data_and_unbounded_controls_are_rejected(self):
        for x in [np.zeros(1000), np.full(1000, np.nan), np.ones(20)]:
            with self.assertRaises(ValueError):
                describe(x, RATE)
        with self.assertRaises(ValueError):
            encode(np.full(6, 1.1), "5.0", "small")
        p = parameters(np.zeros(6), "9.1.6", "cathedral")
        self.assertEqual(p["rt60"], float(np.float32(0.3)))
        self.assertFalse(p["content_aware"])
        self.assertEqual(p["dry_level"], 0.0)


if __name__ == "__main__":
    unittest.main()
