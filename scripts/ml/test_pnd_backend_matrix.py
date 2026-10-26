"""Keep backend equivalence separate from rejected or mismatched observations."""

import unittest

from analyze_pnd_backend_matrix import parity


def observation(hz=220, confidence=0.9, end=3072):
    return [
        {
            "source_end_frame": end,
            "window_frames": 3072,
            "frequency_hz": hz,
            "confidence": confidence,
        }
    ]


class BackendParityTests(unittest.TestCase):
    def test_wrong_pitch_cannot_pass_matching_confidence(self):
        self.assertFalse(parity(observation(), observation(hz=440))["passed"])

    def test_threshold_crossing_is_not_hidden_by_small_numeric_difference(self):
        result = parity(
            observation(confidence=0.800001), observation(confidence=0.799999)
        )
        self.assertEqual(result["acceptance_disagreements"], 1)
        self.assertFalse(result["passed"])

    def test_rejected_windows_do_not_claim_zero_pitch_difference(self):
        result = parity(
            observation(confidence=0.1), observation(hz=440, confidence=0.1)
        )
        self.assertEqual(result["jointly_accepted_windows"], 0)
        self.assertIsNone(result["max_accepted_pitch_difference_cents"])

    def test_mismatched_source_windows_are_rejected(self):
        with self.assertRaises(ValueError):
            parity(observation(), observation(end=3552))


if __name__ == "__main__":
    unittest.main()
