"""Causal key policy and actual native sidechain/delay regression checks."""

import os
import tempfile
import unittest
from pathlib import Path

import numpy as np
from deesser_conditioning import PARAMS, key_policy, render


class KeyPolicyTests(unittest.TestCase):
    def test_unavailable_model_is_exact_classical_key(self):
        x = np.random.default_rng(1).uniform(-0.5, 0.5, 10001).astype(np.float32)
        key, decisions = key_policy(x, "speech-relative")
        np.testing.assert_array_equal(key, x)
        self.assertTrue(all(d["reason"] == "classical_fallback" for d in decisions))

    def test_future_and_stale_scores_do_not_backdate_decisions(self):
        x = np.full(14401, 0.2, np.float32)
        trace = {
            "rows": [
                {
                    "available_at_frame": 4800,
                    "new_observation": True,
                    "score": 0.1,
                    "warmup_complete": True,
                    "source_age_ms_at_snapshot": 100.0,
                }
            ]
        }
        key, rows = key_policy(x, "speech-relative", trace)
        np.testing.assert_array_equal(key[:4800], x[:4800])
        self.assertEqual(rows[10]["reason"], "low_speech_score")
        self.assertEqual(rows[15]["reason"], "classical_fallback")
        self.assertEqual(rows[16]["reason"], "classical_fallback")
        self.assertGreater(float(key[-1]), 0.1999)

    def test_key_prefix_invariance_and_no_gain_boost(self):
        x = np.random.default_rng(2).uniform(-0.5, 0.5, 19001).astype(np.float32)
        trace = {
            "rows": [
                {
                    "available_at_frame": 4800,
                    "new_observation": True,
                    "score": 0.05,
                    "warmup_complete": True,
                    "source_age_ms_at_snapshot": 100.0,
                }
            ]
        }
        for method, observation in [("relative", None), ("speech-relative", trace)]:
            full, _ = key_policy(x, method, observation)
            self.assertTrue(np.all(np.abs(full) <= np.abs(x) + 1e-8))
            for length in [1, 479, 480, 481, 5001, 7201, 10000, 12033]:
                prefix, _ = key_policy(x[:length], method, observation)
                np.testing.assert_array_equal(prefix, full[:length])

    def test_invalid_observation_clock_rejected(self):
        with self.assertRaises(ValueError):
            key_policy(
                np.ones(100), "speech-relative", {"rows": [{"available_at_frame": -1}]}
            )


class NativeDeesserTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.renderer = os.environ.get("SOTF_DEESSER_RENDERER")
        if not cls.renderer:
            raise unittest.SkipTest("Set SOTF_DEESSER_RENDERER to qa-de-esser-render")

    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.work = Path(self.temporary.name)

    def run_native(self, x, key=None, params=None, rate=48000, callback=193):
        return render(
            x,
            self.renderer,
            self.work / str(len(list(self.work.iterdir()))),
            key,
            params,
            rate,
            callback,
        )

    def test_matching_key_is_exact_internal_detector_and_callbacks_match(self):
        x = np.random.default_rng(3).uniform(-0.5, 0.5, (2001, 2)).astype(np.float32)
        for mode, topology in [
            ("Wideband", "Minimum-Phase"),
            ("Split-Band", "Minimum-Phase"),
            ("Split-Band", "Linear-Phase"),
        ]:
            p = PARAMS | {"mode": mode, "split_topology": topology}
            a, _ = self.run_native(x, params=p)
            b, _ = self.run_native(x, key=x, params=p, callback=17)
            np.testing.assert_array_equal(a, b)

    def test_zero_key_and_delay_marker_oracle(self):
        for rate in [44100, 48000, 96000, 192000]:
            x = np.zeros((499, 2), np.float32)
            x[0] = [0.8, -0.2]
            x[-1] = [-0.4, 0.1]
            output, meta = self.run_native(
                x, key=np.zeros_like(x), rate=rate, callback=1
            )
            np.testing.assert_array_equal(output, x)
            self.assertEqual(meta["latency_samples"], max(1, round(rate * 0.002)))

    def test_native_finalized_prefix_does_not_depend_on_future_input(self):
        x = np.random.default_rng(4).uniform(-0.5, 0.5, (4001, 2)).astype(np.float32)
        for mode, topology in [
            ("Wideband", "Minimum-Phase"),
            ("Split-Band", "Linear-Phase"),
        ]:
            p = PARAMS | {"mode": mode, "split_topology": topology}
            full, meta = self.run_native(x, params=p)
            prefix, _ = self.run_native(x[:2001], params=p, callback=127)
            finalized = 2001 - meta["latency_samples"]
            np.testing.assert_array_equal(prefix[:finalized], full[:finalized])


if __name__ == "__main__":
    unittest.main()
