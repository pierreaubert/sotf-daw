"""Check per-channel removal budgets, sparse reflection protection and policy I/O."""

import json
import tempfile
import unittest
from pathlib import Path

import numpy as np
from ir_preservation import FORMAT, load_policy, preserve_tail, validate_policy


class PreservationTests(unittest.TestCase):
    def test_arbitrary_mask_obeys_every_channel_window_budget(self):
        rng = np.random.default_rng(51)
        samples = rng.normal(size=(8000, 4)).astype("<f4") * np.array(
            [0.001, 0.0001, 0.00001, 0.000001], dtype="<f4"
        )
        samples[4000:4100] += np.array([0.1, -0.2, 0.02, 0.001], dtype="<f4")
        for window in [1.0, 5.0, 20.0]:
            policy = {"format": FORMAT, "window_ms": window, "noise_fraction": 0.5}
            result, mask, diagnostics = preserve_tail(
                samples, 16000, 1280, 960, rng.uniform(0.05, 1, 8000), policy
            )
            self.assertTrue(diagnostics["within_budget"])
            np.testing.assert_array_equal(result[:2080], samples[:2080])
            width = round(window * 16)
            noise = np.sum(samples[:960].astype(float) ** 2, axis=0) / 960
            for first in range(2080, len(samples), width):
                last = min(len(samples), first + width)
                difference = result[first:last].astype(float) - samples[first:last]
                self.assertTrue(
                    np.all(
                        np.sum(difference**2, axis=0)
                        <= noise * (last - first) * 0.5 * 1.00001
                    )
                )
            self.assertTrue(np.all(mask >= 0.05))
            self.assertTrue(np.all(mask <= 1))

    def test_late_reflection_is_preserved_above_noise(self):
        rng = np.random.default_rng(52)
        samples = rng.normal(0, 0.001, (8000, 2)).astype("<f4")
        samples[6000] += 0.15
        policy = {"format": FORMAT, "window_ms": 5.0, "noise_fraction": 1.0}
        output, _, diagnostics = preserve_tail(
            samples, 16000, 1280, 960, np.full(8000, 0.05), policy
        )
        self.assertTrue(diagnostics["within_budget"])
        self.assertTrue(np.all(output[6000] > samples[6000] * 0.99))
        # A naive global/early-only bound can admit this weak late reflection's
        # deletion; the local budget sees its energy against local noise.
        self.assertLess(np.sum((samples[6000] * 0.95) ** 2), 0.05)

    def test_zero_noise_channel_forces_exact_linked_preservation(self):
        rng = np.random.default_rng(4)
        samples = rng.normal(0, 0.001, (8000, 2)).astype("<f4")
        samples[:960, 1] = 0
        policy = {"format": FORMAT, "window_ms": 5.0, "noise_fraction": 1.0}
        output, mask, diagnostics = preserve_tail(
            samples, 16000, 1280, 960, np.full(8000, 0.05), policy
        )
        self.assertTrue(diagnostics["within_budget"])
        np.testing.assert_array_equal(output, samples)
        np.testing.assert_array_equal(mask, 1)

    def test_invalid_or_mismatched_policy_rejected(self):
        valid = {"format": FORMAT, "window_ms": 5.0, "noise_fraction": 1.0}
        for key, value in [
            ("window_ms", 0.0),
            ("noise_fraction", float("nan")),
            ("noise_fraction", True),
            ("noise_fraction", 1.1),
        ]:
            with self.assertRaises(ValueError):
                validate_policy({**valid, key: value})
        with self.assertRaises(ValueError):
            validate_policy({**valid, "extra": 1})
        with tempfile.TemporaryDirectory() as directory:
            file = Path(directory) / "policy.json"
            file.write_text(json.dumps(valid))
            with self.assertRaisesRegex(ValueError, "identity"):
                load_policy(file, "0" * 64)


if __name__ == "__main__":
    unittest.main()
