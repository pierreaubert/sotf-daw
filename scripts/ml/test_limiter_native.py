"""Independent streaming/ceiling checks for release-assistant native renders."""

import os
import tempfile
import unittest
from pathlib import Path

import numpy as np
from limiter_native import PARAMS, render


class LimiterNativeTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.renderer = os.environ.get("SOTF_LIMITER_RENDERER")
        if not cls.renderer:
            raise unittest.SkipTest("Set SOTF_LIMITER_RENDERER to qa-limiter-render")

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.work = Path(self.temp.name)

    def run_native(self, signal, rate=48000, callback=193, **settings):
        return render(
            signal,
            rate,
            self.renderer,
            self.work / str(len(list(self.work.iterdir()))),
            callback,
            PARAMS | settings,
        )

    def test_identity_markers_and_exact_delay(self):
        for rate in [44100, 48000, 96000, 192000]:
            for channels in [1, 2, 6]:
                signal = np.zeros((379, channels), dtype=np.float32)
                signal[0] = np.arange(1, channels + 1) * 0.001
                signal[-1] = -signal[0]
                for callback in [1, 127, 4096]:
                    got, meta = self.run_native(signal, rate, callback)
                    np.testing.assert_array_equal(got, signal)
                    self.assertEqual(meta["latency_samples"], int(rate * 5 / 1000))

    def test_ceiling_streaming_and_zero_continuation(self):
        rng = np.random.default_rng(20411026)
        for rate in [44100, 48000, 96000, 192000]:
            signal = rng.uniform(-3, 3, (1237, 2)).astype(np.float32)
            for oversampling in [0, 1, 2]:
                settings = {"oversampling": oversampling, "release_ms": 379.0}
                a, _ = self.run_native(signal, rate, 193, **settings)
                b, _ = self.run_native(signal, rate, 17, **settings)
                np.testing.assert_array_equal(a, b)
                continued = np.concatenate([signal, np.zeros((4096, 2), np.float32)])
                c, _ = self.run_native(continued, rate, 257, **settings)
                np.testing.assert_array_equal(a, c[: len(signal)])
                self.assertLessEqual(float(np.abs(a).max()), 10 ** (-6 / 20) + 2e-6)

    def test_release_changes_recovery_without_changing_bound(self):
        signal = np.full(48000, 0.1, np.float32)
        signal[0:2400] = 2.0
        fast, _ = self.run_native(signal, release_ms=10.0)
        slow, _ = self.run_native(signal, release_ms=1000.0)
        self.assertGreater(
            float(fast[6000:12000].mean()), float(slow[6000:12000].mean())
        )
        for out in [fast, slow]:
            self.assertLessEqual(float(np.abs(out).max()), 10 ** (-6 / 20) + 2e-6)

    def test_short_stream_isp_and_linked_dual_release(self):
        for rate in [44100, 96000, 192000]:
            for channels in [1, 2, 6]:
                for link in [0.0, 0.4, 1.0]:
                    signal = np.full((11, channels), 2.0, np.float32)
                    signal[:, 1::2] *= -0.7
                    settings = {
                        "true_peak": True,
                        "isp_mode": True,
                        "dual_release": True,
                        "link_amount": link,
                    }
                    a, _ = self.run_native(signal, rate, 1, **settings)
                    b, _ = self.run_native(signal, rate, 193, **settings)
                    np.testing.assert_array_equal(a, b)
                    self.assertEqual(a.shape, signal.shape)

    def test_zero_lookahead_retains_first_and_last_frames(self):
        signal = np.full((379, 2), 0.001, np.float32)
        signal[0] = [0.002, -0.003]
        signal[-1] = [-0.004, 0.005]
        got, meta = self.run_native(signal, lookahead_ms=0.0)
        np.testing.assert_array_equal(got, signal)
        self.assertEqual(meta["latency_samples"], 0)
        self.assertEqual(meta["drained_frames"], 0)

    def test_invalid_requests_do_not_export(self):
        for settings in [
            {"release_ms": 9.0},
            {"threshold_db": -21.0},
            {"soft": True},
            {"mix": 0.5},
            {"typo": 1},
            {"oversampling": 3},
        ]:
            before = set(self.work.iterdir())
            with self.assertRaises(ValueError):
                self.run_native(np.ones(97, np.float32), **settings)
            added = set(self.work.iterdir()) - before
            self.assertEqual(len(added), 1)
            self.assertFalse((added.pop() / "output.f32le").exists())


if __name__ == "__main__":
    unittest.main()
