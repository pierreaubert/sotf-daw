"""Check IR timing, response rejection, linked masking and CLI fallback contracts."""

import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

import numpy as np
from ir_denoise import apply, load_model, response_checks, sha, validate_input
from scipy.io import wavfile


class IrDenoiseTests(unittest.TestCase):
    def test_linked_mask_and_protected_samples_are_exact(self):
        rate, arrival = 16000, 1280
        rng = np.random.default_rng(14)
        mono = rng.normal(0, 0.01, 8000).astype("<f4")
        samples = np.column_stack([mono, mono * -0.5])
        starts = np.arange(arrival, len(samples), 320)
        result, mask = apply(samples, rate, arrival, starts, np.full(len(starts), 0.2))
        np.testing.assert_array_equal(result[: arrival + 800], samples[: arrival + 800])
        np.testing.assert_array_equal(result[:, 1], result[:, 0] * -0.5)
        self.assertGreaterEqual(mask.min(), 0.05)
        self.assertLessEqual(mask.max(), 1)
        with self.assertRaisesRegex(ValueError, "invalid tail mask"):
            apply(samples, rate, arrival, starts, np.full(len(starts), -1.0))

    def test_late_structural_change_can_trigger_response_rejection(self):
        original = np.zeros((8000, 1), dtype="<f4")
        original[1280] = 1
        original[6000] = 0.5
        changed = original.copy()
        changed[6000] = 0
        checks = response_checks(original.astype(float), changed, 16000, 1280)
        self.assertTrue(checks["early_f32_exact"])
        self.assertFalse(checks["accepted"])
        self.assertGreater(checks["relative_ir_change"], 0.05)

    def test_invalid_geometry_and_nonfinite_input(self):
        samples = np.zeros((8000, 1))
        for rate, arrival, prefix in [
            (0, 1280, 960),
            (16000, 800, 960),
            (16000, 1280, 0),
        ]:
            with self.assertRaises(ValueError):
                validate_input(samples, rate, arrival, prefix)
        samples[4000] = np.nan
        with self.assertRaises(ValueError):
            validate_input(samples, 16000, 1280, 960)

    def test_missing_model_cli_exports_exact_original(self):
        rng = np.random.default_rng(2)
        samples = rng.normal(0, 0.01, (8000, 2)).astype("<f4")
        samples[1280] = 0.8
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source, output, report = (
                root / "input.wav",
                root / "output.wav",
                root / "report.json",
            )
            wavfile.write(source, 16000, samples)
            command = [
                sys.executable,
                str(Path(__file__).with_name("ir_denoise.py")),
                "--input",
                str(source),
                "--output",
                str(output),
                "--report",
                str(report),
                "--model",
                str(root / "missing.json"),
                "--model-sha256",
                "0" * 64,
                "--arrival-ms",
                "80",
                "--noise-prefix-ms",
                "60",
            ]
            completed = subprocess.run(
                command, check=False, capture_output=True, text=True
            )
            self.assertEqual(completed.returncode, 0, completed.stderr)
            rate, actual = wavfile.read(output)
            self.assertEqual(rate, 16000)
            np.testing.assert_array_equal(actual, samples)
            record = json.loads(report.read_text())
            self.assertFalse(record["used_learned_output"])
            self.assertFalse(record["adoption_recommended"])
            self.assertIsNotNone(record["fallback_error"])
            # Re-running may not overwrite a previous reviewed export.
            before = sha(output)
            again = subprocess.run(command, check=False, capture_output=True, text=True)
            self.assertNotEqual(again.returncode, 0)
            self.assertEqual(sha(output), before)

    def test_malformed_model_rejected_before_inference(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "model.json"
            path.write_text('{"format":"wrong"}')
            with self.assertRaisesRegex(ValueError, "unsupported model format"):
                load_model(path, sha(path))
            with self.assertRaisesRegex(ValueError, "identity"):
                load_model(path, "0" * 64)


if __name__ == "__main__":
    unittest.main()
