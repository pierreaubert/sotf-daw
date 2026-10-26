"""Runtime gates must catch precision and decision changes despite fast timings."""

import sys
import tempfile
import unittest
from pathlib import Path

import numpy as np
from analyze_crepe_runtime import decode, parity, timing
from crepe_runtime_corpus import corpus
from run_crepe_runtime_matrix import run_case


class CrepeRuntimeTests(unittest.TestCase):
    def test_timed_out_benchmark_is_unscored_and_stops_its_owned_process_group(self):
        with tempfile.TemporaryDirectory() as work:
            result = run_case(
                Path(work),
                [sys.executable, "-c", "import time;time.sleep(10)"],
                timeout_s=0.05,
            )
            self.assertEqual(result["exit_code"], 124)
            self.assertIn("no runtime score", (Path(work) / "stderr.txt").read_text())

    def test_corpus_is_bounded_and_uses_sample_standard_deviation(self):
        frames, labels = corpus()
        self.assertEqual(frames.shape, (32, 1024))
        self.assertEqual(len(labels), 32)
        self.assertTrue(np.isfinite(frames).all())
        self.assertLess(float(abs(frames).max()), 64)
        np.testing.assert_allclose(frames.mean(axis=1, dtype=np.float64), 0, atol=2e-8)
        nonzero = np.max(abs(frames), axis=1) > 0
        np.testing.assert_allclose(frames[nonzero].std(axis=1, ddof=1), 1, atol=2e-7)

    def test_activation_precision_failure_cannot_be_hidden_by_unchanged_decode(self):
        baseline = np.zeros((1, 360), dtype=np.float32)
        baseline[0, 120] = 0.9
        hz, _, _ = decode(baseline[0])
        candidate = baseline.copy()
        candidate[0, 0] = 0.001
        result = parity(baseline, candidate, [{"reference_hz": hz}])
        self.assertEqual(result["policy_disagreements"], 0)
        self.assertEqual(result["maximum_accepted_pitch_error_cents"], 0)
        self.assertFalse(result["gate_pass"])

    def test_sub_tolerance_activation_change_can_still_fail_policy_stability(self):
        baseline = np.zeros((1, 360), dtype=np.float32)
        baseline[0, 120] = np.nextafter(np.float32(0.8), np.float32(1))
        candidate = baseline.copy()
        candidate[0, 120] = np.nextafter(np.float32(0.8), np.float32(0))
        hz, _, _ = decode(baseline[0])
        result = parity(baseline, candidate, [{"reference_hz": hz}])
        self.assertLess(result["maximum_activation_error"], 2e-5)
        self.assertEqual(result["policy_disagreements"], 1)
        self.assertFalse(result["gate_pass"])

    def test_missing_rows_float_units_and_impossible_time_arithmetic_reject(self):
        request = {"frames": 1, "warmup_cycles": 1, "measured_cycles": 2}
        base = {
            "frames": 1,
            "warmup_cycles": 1,
            "measured_cycles": 2,
            "timings_ns": [20, 30],
            "measurement_wall_ns": 51,
            "ready_from_entry_ns": 20,
            "model_load_ns": 10,
            "first_inference_ns": 15,
        }
        self.assertEqual(timing(base, request)["measured_calls"], 2)
        for changed in [
            base | {"timings_ns": [20]},
            base | {"timings_ns": [0.02, 0.03]},
            base | {"measurement_wall_ns": 49},
            base | {"ready_from_entry_ns": 9},
        ]:
            with self.assertRaises(ValueError):
                timing(changed, request)


if __name__ == "__main__":
    unittest.main()
