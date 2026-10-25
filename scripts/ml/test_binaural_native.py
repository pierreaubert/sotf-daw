"""Native fixed-pose convolution and geometry checks; no perceptual acceptance."""

import os
import tempfile
import unittest
from pathlib import Path

import numpy as np
from binaural_native import render

SOFA = (
    Path(__file__).resolve().parents[2]
    / "crates/sotf-plugins/crates/sotf-plugin-binaural/tests/fixtures/kemar/mit_kemar_normal_pinna.sofa"
)
RENDERER = os.environ.get("SOTF_BINAURAL_RENDERER")


@unittest.skipUnless(RENDERER, "Set SOTF_BINAURAL_RENDERER to the native QA binary")
class NativeBinauralTests(unittest.TestCase):
    def test_actual_native_impulses_predict_full_random_render(self):
        rng = np.random.default_rng(20501026)
        x = (rng.normal(size=(1301, 2)) * 0.03).astype(np.float32)
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for rate in [44100, 48000, 96000, 192000]:
                for eq in [False, True]:
                    n = 4096 if rate == 192000 else 2048
                    response = []
                    for ch in range(2):
                        impulse = np.zeros((1, 2), dtype=np.float32)
                        impulse[0, ch] = 32
                        h, _ = render(
                            impulse,
                            RENDERER,
                            SOFA,
                            root / f"h-{rate}-{eq}-{ch}",
                            rate=rate,
                            fft_size=n,
                            eq=eq,
                        )
                        response.append(h.astype(float) / 32)
                    y, meta = render(
                        x,
                        RENDERER,
                        SOFA,
                        root / f"x-{rate}-{eq}",
                        rate=rate,
                        fft_size=n,
                        eq=eq,
                    )
                    expected = np.zeros((len(x) + max(map(len, response)) - 1, 2))
                    for ch in range(2):
                        for ear in range(2):
                            c = np.convolve(
                                x[:, ch].astype(float),
                                response[ch][:, ear].astype(float),
                            )
                            expected[: len(c), ear] += c
                    size = max(len(y), len(expected))
                    actual = np.pad(y, ((0, size - len(y)), (0, 0)))
                    expected = np.pad(expected, ((0, size - len(expected)), (0, 0)))
                    # Native output suppresses magnitudes below 1e-5, including
                    # impulse taps. Its output is therefore not strictly LTI.
                    # Tap uncertainty <= floor/32 propagates by input L1;
                    # output suppression adds <= floor; retain 2e-6 FFT slack.
                    bound = (
                        1e-5 * (1 + float(np.abs(x.astype(float)).sum()) / 32) + 2e-6
                    )
                    error = float(np.max(np.abs(actual - expected)))
                    print(
                        f"convolution rate={rate} eq={eq} fft={n} max_error={error:.10g} bound={bound:.10g}",
                        flush=True,
                    )
                    self.assertLess(error, bound)
                    self.assertGreater(meta["drained_frames"], meta["latency_samples"])

    def test_callback_and_finalized_prefix_invariance(self):
        rng = np.random.default_rng(20511026)
        x = (rng.normal(size=(4201, 2)) * 0.02).astype(np.float32)
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            y, _ = render(x, RENDERER, SOFA, root / "full", callback=193)
            other, _ = render(x, RENDERER, SOFA, root / "other", callback=4096)
            self.assertLess(float(np.max(np.abs(y - other))), 1e-7)
            prefix = 3101
            short, meta = render(
                x[:prefix], RENDERER, SOFA, root / "short", callback=17
            )
            # Only compare samples finalized by accepted real input, before drain.
            finalized = prefix - meta["latency_samples"]
            np.testing.assert_array_equal(y[:finalized], short[:finalized])

    def test_invalid_sofa_fails_without_output(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            bad = root / "bad.sofa"
            bad.write_bytes(b"invalid SOFA data")
            with self.assertRaises(ValueError):
                render(np.zeros((10, 2)), RENDERER, bad, root / "render")
            self.assertFalse((root / "render/output.f32le").exists())

    def test_insufficient_design_capacity_fails_without_output(self):
        with tempfile.TemporaryDirectory() as directory:
            work = Path(directory) / "render"
            with self.assertRaisesRegex(ValueError, "design capacity"):
                render(
                    np.zeros((19, 2)), RENDERER, SOFA, work, rate=192000, fft_size=2048
                )
            self.assertFalse((work / "output.f32le").exists())

    def test_silence_supported_layouts_and_geometry_rejection(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for channels in [1, 2, 3, 5, 6, 8, 10, 12, 14, 16]:
                y, _ = render(
                    np.zeros((19, channels)), RENDERER, SOFA, root / str(channels)
                )
                self.assertEqual(y.shape[1], 2)
                self.assertEqual(float(np.max(np.abs(y))), 0)
            for x in [np.zeros((10, 4)), np.full((10, 2), np.nan), np.zeros((0, 2))]:
                with self.assertRaises(ValueError):
                    render(x, RENDERER, SOFA, root / "invalid")


if __name__ == "__main__":
    unittest.main()
