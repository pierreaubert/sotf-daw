"""Independent bilinear and split/gradient checks for six-band proposals."""

import contextlib
import io
import json
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace

import eq_multiband as eq
import numpy as np
import torch
from scipy.signal import bilinear, sosfreqz


class MultibandPhysicsTests(unittest.TestCase):
    def test_native_conventions_match_independent_analog_bilinear_sections(self):
        parameters, rates, _, _ = eq.dataset(12, 20261117)
        for p, rate in zip(parameters, rates):
            sections = []
            for slot in range(6):
                if slot < 4:
                    f, q, gain = (
                        np.exp(p[3 * slot]),
                        np.exp(p[3 * slot + 1]),
                        p[3 * slot + 2],
                    )
                    w, a = 2 * rate * np.tan(np.pi * f / rate), 10 ** (gain / 40)
                    b, d = [1, w * a / q, w * w], [1, w / (a * q), w * w]
                else:
                    f, gain = np.exp(p[12 + 2 * (slot - 4)]), p[13 + 2 * (slot - 4)]
                    w, a = 2 * rate * np.tan(np.pi * f / rate), 10 ** (gain / 40)
                    if slot == 4:
                        b, d = (
                            [a, a * np.sqrt(2 * a) * w, a * a * w * w],
                            [a, np.sqrt(2 * a) * w, w * w],
                        )
                    else:
                        b, d = (
                            [a * a, a * np.sqrt(2 * a) * w, a * w * w],
                            [1, np.sqrt(2 * a) * w, a * w * w],
                        )
                b, d = bilinear(b, d, fs=rate)
                sections.append(np.concatenate([b, d]))
            _, reference = sosfreqz(sections, worN=eq.GRID, fs=rate)
            expected = 20 * np.log10(np.abs(reference))
            actual = eq.response(p, rate=rate)
            np.testing.assert_allclose(actual, expected, atol=1e-6, rtol=0)
            for section in eq.coefficients(p, np.asarray(rate)):
                self.assertTrue(np.all(np.abs(np.roots(section[3:])) < 1))

    def test_differentiable_response_and_gradients_agree_with_numpy(self):
        p, rates, target, _ = eq.dataset(16, 20261118)
        candidate = torch.tensor(p, requires_grad=True)
        actual = eq.differentiable_response(candidate, torch.tensor(rates))
        np.testing.assert_allclose(actual.detach().numpy(), target, atol=1e-6, rtol=0)
        actual.square().mean().backward()
        self.assertTrue(torch.isfinite(candidate.grad).all())
        self.assertGreater(candidate.grad.abs().sum().item(), 0)

    def test_effective_parameter_identities_do_not_overlap(self):
        a, b = eq.dataset(64, 20261119), eq.dataset(64, 20261120)
        self.assertEqual(len(set(a[3])), 64)
        self.assertFalse(set(a[3]) & set(b[3]))
        self.assertTrue(np.all(np.abs(a[2]) <= 24))

    def test_training_refuses_existing_artifacts(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            marker = root / "preserve.bin"
            marker.write_bytes(b"existing checkpoint")
            with self.assertRaises(ValueError):
                eq.train(SimpleNamespace(output=root, threads=1))
            self.assertEqual(marker.read_bytes(), b"existing checkpoint")
            self.assertFalse((root / "recipe.py").exists())

    def test_invalid_targets_fail_before_model_loading(self):
        data = {
            "sample_rate": 48000,
            "frequencies_hz": [100] * 64,
            "target_db": [1] * 64,
        }
        with self.assertRaises(ValueError):
            eq.validate_target(data)
        data["frequencies_hz"] = eq.GRID.tolist()
        data["target_db"] = np.zeros(len(eq.GRID)).tolist()
        data["target_db"][1] = float("nan")
        with self.assertRaises(ValueError):
            eq.validate_target(data)

    def test_flat_target_is_canonical_noop_without_weights(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            target = root / "target.json"
            target.write_text(
                json.dumps(
                    {
                        "sample_rate": 48000,
                        "frequencies_hz": eq.GRID.tolist(),
                        "target_db": np.zeros(len(eq.GRID)).tolist(),
                    }
                )
            )
            args = SimpleNamespace(
                target=target,
                output=root / "result.json",
                weights=root / "absent.json",
                max_evaluations=10,
            )
            with contextlib.redirect_stdout(io.StringIO()):
                eq.fit(args)
            result = json.loads(args.output.read_text())
            self.assertTrue(result["accepted"])
            self.assertEqual(result["parameters"]["filters"], [])
            self.assertFalse(result["parameters"]["auto_gain"]["enabled"])
            self.assertNotIn("qualified_proposals", result)
            args.include_qualified_proposals = True
            args.output = root / "extended.json"
            with contextlib.redirect_stdout(io.StringIO()):
                eq.fit(args)
            extended = json.loads(args.output.read_text())
            self.assertEqual(len(extended["qualified_proposals"]), 1)
            self.assertEqual(
                extended["qualified_proposals"][0]["parameters"], result["parameters"]
            )
            self.assertEqual(
                extended["qualified_proposals"][0]["target_sha256"],
                result["target_sha256"],
            )

    def test_zero_gain_is_identity_and_shelf_pair_can_represent_constant_gain(self):
        p = np.tile([np.log(1000), 0, 0], 4).tolist() + [np.log(1000), 0] * 2
        for rate in eq.TRAIN_RATES:
            np.testing.assert_allclose(eq.response(p, rate=rate), 0, atol=1e-10, rtol=0)
            tilted = np.asarray(p)
            tilted[[13, 15]] = 5.5
            np.testing.assert_allclose(
                eq.response(tilted, rate=rate), 5.5, atol=1e-7, rtol=0
            )


if __name__ == "__main__":
    unittest.main()
