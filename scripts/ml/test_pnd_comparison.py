"""Independent phase-probe calibration and honest rejection/accuracy denominators."""

import unittest

import numpy as np
from analyze_pnd_comparison import raw_metrics, trajectory
from pnd_compare_cases import fixture


class PndComparisonTests(unittest.TestCase):
    def test_phase_probe_recovers_known_modulated_carriers(self):
        for kind in ["offset", "wow", "flutter", "vibrato", "weak_fundamental"]:
            signal, truth, _, identity = fixture(20308100, kind)
            frames, estimated = trajectory(
                signal,
                48000,
                identity["reference_hz"] * identity["measurement_partial"],
            )
            valid = (frames >= 24000) & (frames < len(signal) - 24000)
            expected = np.interp(frames, np.arange(len(truth)), truth)
            self.assertLess(
                float(np.sqrt(np.mean((estimated[valid] - expected[valid]) ** 2))),
                0.1,
                kind,
            )

    def test_noise_probe_cannot_claim_clean_trajectory_accuracy(self):
        signal, truth, _, identity = fixture(20308100, "noisy_wow")
        # Deliberately stronger contamination than the comparison fixture.
        signal = signal + np.random.default_rng(777).normal(0, 0.2, len(signal))
        frames, estimated = trajectory(signal, 48000, identity["reference_hz"])
        valid = (frames >= 24000) & (frames < len(signal) - 24000)
        expected = np.interp(frames, np.arange(len(truth)), truth)
        self.assertGreater(
            float(np.sqrt(np.mean((estimated[valid] - expected[valid]) ** 2))), 2
        )

    def test_absent_fundamental_octave_is_reported_without_invented_truth(self):
        rows = [
            {
                "source_end_frame": 48000,
                "window_frames": 3072,
                "frequency_hz": 440,
                "confidence": 0.9,
            }
        ]
        result = raw_metrics(rows, np.zeros(144017), 48000, 220, False, 0.8)
        self.assertEqual(result["confidence_accepted"], 1)
        self.assertEqual(result["confidence_accepted_reference_octaves"], 1)
        self.assertEqual(result["reference_policy_accepted"], 0)
        self.assertIsNone(result["accepted_error_rms_cents"])
        self.assertIsNone(result["accepted_octave_errors"])

    def test_rejected_estimates_do_not_become_zero_accuracy_error(self):
        rows = [
            {
                "source_end_frame": 48000,
                "window_frames": 3072,
                "frequency_hz": 220,
                "confidence": 0.1,
            }
        ]
        result = raw_metrics(rows, np.zeros(144017), 48000, 220, True, 0.8)
        self.assertEqual(result["confidence_accepted_fraction"], 0)
        self.assertIsNone(result["accepted_error_rms_cents"])


if __name__ == "__main__":
    unittest.main()
