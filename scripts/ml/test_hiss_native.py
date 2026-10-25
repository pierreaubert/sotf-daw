"""Independent native capture, full-tail and profile-identity qualification."""

import copy
import os
import tempfile
import unittest
from pathlib import Path

import numpy as np
from hiss_native import PARAMS, render
from scipy.signal import lfilter

RENDERER = os.environ.get("SOTF_HISS_RENDERER")


@unittest.skipUnless(RENDERER, "Set SOTF_HISS_RENDERER")
class HissNativeTests(unittest.TestCase):
    def test_capture_matches_independent_floor_and_periodic_hann_spectrum(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for rate in [44100, 48000, 96000, 192000]:
                capture = (
                    np.random.default_rng(20641026 + rate).normal(size=(rate, 2))
                    * 0.003
                ).astype(np.float32)
                capture[:, 1] *= 0.25
                spectra = []
                for spectral in [False, True]:
                    params = dict(
                        PARAMS, spectral_mode=spectral, use_captured_profile=True
                    )
                    _, meta = render(
                        np.zeros((1001, 2)),
                        RENDERER,
                        root / f"{rate}-{spectral}",
                        params,
                        capture=capture,
                        rate=rate,
                    )
                    profile = meta["effective_params"]["captured_profile"]
                    self.assertEqual(profile["frames_analyzed"], rate)
                    self.assertEqual(profile["channels"], 2)
                    alpha = 1 - np.exp(-2 * np.pi * 4000 / rate)
                    high = capture.astype(float) - lfilter(
                        [alpha], [1, -(1 - alpha)], capture.astype(float), axis=0
                    )
                    expected = 10 * np.log10(np.mean(high**2, axis=0))
                    error = float(
                        np.max(abs(expected - profile["floor_db_per_channel"]))
                    )
                    self.assertLess(error, 0.001)
                    n = 1024
                    hop = 256
                    window = 0.5 - 0.5 * np.cos(2 * np.pi * np.arange(n) / n)
                    windows = np.lib.stride_tricks.sliding_window_view(
                        capture.astype(float), n, axis=0
                    )[::hop]
                    power = np.mean(
                        abs(np.fft.rfft(windows * window, axis=-1)) ** 2, axis=0
                    )
                    measured = np.asarray(
                        profile["spectral"]["power_per_channel_bin"]
                    ).reshape(2, -1)
                    spectral_error = float(
                        np.max(abs(power - measured)) / max(float(power.max()), 1e-20)
                    )
                    self.assertLess(spectral_error, 2e-5)
                    self.assertEqual(profile["spectral"]["hops_analyzed"], len(windows))
                    spectra.append(profile)
                    print(
                        f"capture rate={rate} spectral={spectral} max_floor_error_db={error:.10g} max_spectral_error_relative_to_peak={spectral_error:.10g}",
                        flush=True,
                    )
                self.assertEqual(spectra[0], spectra[1])

    def test_profile_reopening_callbacks_and_fresh_program_state(self):
        rng = np.random.default_rng(20651026)
        capture = (rng.normal(size=(48000, 2)) * 0.004).astype(np.float32)
        x = (rng.normal(size=(5001, 2)) * 0.02).astype(np.float32)
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for spectral in [False, True]:
                params = dict(
                    PARAMS,
                    spectral_mode=spectral,
                    use_captured_profile=True,
                    strength=1.0,
                )
                y, m = render(
                    x, RENDERER, root / f"capture-{spectral}", params, capture=capture
                )
                reopened, _ = render(
                    x,
                    RENDERER,
                    root / f"reopen-{spectral}",
                    m["effective_params"],
                    callback=17,
                )
                np.testing.assert_array_equal(y, reopened)
                alternate, a = render(
                    x,
                    RENDERER,
                    root / f"other-{spectral}",
                    params,
                    capture=capture,
                    callback=4096,
                )
                self.assertEqual(m["effective_params"], a["effective_params"])
                np.testing.assert_array_equal(y, alternate)
                self.assertEqual(
                    len(y), len(x) + m["drained_frames"] - m["latency_samples"]
                )
                if spectral:
                    self.assertGreater(len(y), len(x))

    def test_no_capture_is_not_silent_learning_and_disabled_delay_is_correct(self):
        rng = np.random.default_rng(20661026)
        x = (rng.normal(size=(5003, 1)) * 0.02).astype(np.float32)
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for spectral in [False, True]:
                y, m = render(
                    x,
                    RENDERER,
                    root / f"off-{spectral}",
                    dict(PARAMS, spectral_mode=spectral, enabled=False),
                )
                self.assertIsNone(m["effective_params"]["captured_profile"])
                self.assertEqual(m["capture_frames"], 0)
                np.testing.assert_allclose(y[: len(x)], x, atol=1e-7, rtol=0)
                self.assertLess(
                    float(np.max(abs(y[len(x) :]))) if len(y) > len(x) else 0.0, 1e-7
                )

    def test_profile_mismatch_and_invalid_capture_refused_without_output(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            capture = (
                np.random.default_rng(20671026)
                .normal(size=(48000, 1))
                .astype(np.float32)
                * 0.003
            )
            _, m = render(
                np.zeros((100, 1)),
                RENDERER,
                root / "valid",
                dict(PARAMS, use_captured_profile=True),
                capture=capture,
            )
            for index, change in enumerate(
                [
                    {"channels": 2},
                    {"sample_rate": 44100},
                    {"measurement_cutoff_hz": 5000},
                ]
            ):
                params = copy.deepcopy(m["effective_params"])
                params["captured_profile"].update(change)
                with self.assertRaises(ValueError):
                    render(np.zeros((100, 1)), RENDERER, root / str(index), params)
                self.assertFalse((root / str(index) / "output.f32le").exists())
            with self.assertRaises(ValueError):
                render(
                    np.zeros((100, 1)),
                    RENDERER,
                    root / "partial",
                    PARAMS,
                    capture=capture[:-1],
                )
            with self.assertRaises(ValueError):
                render(
                    np.zeros((100, 1)),
                    RENDERER,
                    root / "range",
                    dict(PARAMS, strength=2),
                )

    def test_eight_channel_measured_preset_reopens_within_request_bound(self):
        capture = (
            np.random.default_rng(20681026).normal(size=(48000, 8)) * 0.003
        ).astype(np.float32)
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            params = dict(PARAMS, use_captured_profile=True, spectral_mode=True)
            y, m = render(
                np.zeros((19, 8)), RENDERER, root / "capture", params, capture=capture
            )
            z, _ = render(
                np.zeros((19, 8)), RENDERER, root / "reopen", m["effective_params"]
            )
            np.testing.assert_array_equal(y, z)


if __name__ == "__main__":
    unittest.main()
