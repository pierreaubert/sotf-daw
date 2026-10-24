"""Independent physics checks for the offline EQ proposal pipeline."""

import json
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace

import eq_assistant as eq
import numpy as np
import torch
from scipy.signal import freqz


class EqAssistantTests(unittest.TestCase):
    def test_differentiable_response_matches_independent_scipy_evaluation(self):
        rng = np.random.default_rng(11237)
        rows = rng.uniform(eq.LOWER, eq.UPPER, (32, 3))
        actual = eq.differentiable_response(torch.as_tensor(rows)).numpy()
        for p, response in zip(rows, actual):
            frequency, q, gain = np.exp(p[0]), np.exp(p[1]), p[2]
            a = 10 ** (gain / 40)
            omega = 2 * np.pi * frequency / eq.RATE
            alpha = np.sin(omega) / (2 * q)
            b = [1 + alpha * a, -2 * np.cos(omega), 1 - alpha * a]
            d = [1 + alpha / a, -2 * np.cos(omega), 1 - alpha / a]
            _, h = freqz(b, d, worN=eq.GRID, fs=eq.RATE)
            expected = 20 * np.log10(np.abs(h))
            np.testing.assert_allclose(response, expected, atol=1e-7, rtol=0)

    def test_split_identities_are_disjoint_and_response_gradients_finite(self):
        training = eq.dataset(32, 1)
        validation = eq.dataset(32, 2)
        self.assertFalse(set(training[2]) & set(validation[2]))
        p = torch.tensor(training[0], requires_grad=True)
        eq.differentiable_response(p).square().mean().backward()
        self.assertTrue(torch.isfinite(p.grad).all())
        self.assertGreater(float(p.grad.abs().sum()), 0)

    def test_zero_gain_is_unity_for_all_frequency_and_q_settings(self):
        rows, _, _ = eq.dataset(32, 3)
        rows[:, 2] = 0
        np.testing.assert_allclose(eq.response(rows), 0, atol=1e-10, rtol=0)

    def test_invalid_target_is_rejected_before_weights_are_loaded(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            target = root / "target.json"
            target.write_text(
                json.dumps(
                    {
                        "sample_rate": 48000,
                        "frequencies_hz": [100] * 16,
                        "target_db": [1] * 16,
                    }
                )
            )
            args = SimpleNamespace(
                target=target,
                weights=root / "missing.json",
                output=root / "output.json",
            )
            with self.assertRaises(ValueError):
                eq.fit(args)
            self.assertFalse(args.output.exists())


if __name__ == "__main__":
    unittest.main()
