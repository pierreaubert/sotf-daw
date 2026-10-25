"""Independent fixed-filter bounds and native lifecycle/geometry checks."""

import os
import tempfile
import unittest
from pathlib import Path

import numpy as np
from dynamic_eq_native import preset, render
from scipy.signal import freqz, lfilter

RENDERER = os.environ.get("SOTF_DYNAMIC_EQ_RENDERER")


def section(shape, frequency, q, gain, slope, rate):
    """Independent RBJ/bilinear target coefficients, never native designers."""
    w = 2 * np.pi * frequency / rate
    c = np.cos(w)
    sn = np.sin(w)
    a = 10 ** (gain / 40)
    if shape == "peak":
        alpha = sn / (2 * q)
        b = [1 + alpha * a, -2 * c, 1 - alpha * a]
        den = [1 + alpha / a, -2 * c, 1 - alpha / a]
    elif shape == "tilt":
        g = a * a
        k = np.tan(w / 2)
        b = [1 + g * k, g * k - 1, 0]
        den = [g + k, k - g, 0]
    else:
        alpha = sn / 2 * np.sqrt((a + 1 / a) * (1 / slope - 1) + 2)
        beta = 2 * np.sqrt(a) * alpha
        if shape == "low_shelf":
            b = [
                a * ((a + 1) - (a - 1) * c + beta),
                2 * a * ((a - 1) - (a + 1) * c),
                a * ((a + 1) - (a - 1) * c - beta),
            ]
            den = [
                (a + 1) + (a - 1) * c + beta,
                -2 * ((a - 1) + (a + 1) * c),
                (a + 1) + (a - 1) * c - beta,
            ]
        else:
            b = [
                a * ((a + 1) + (a - 1) * c + beta),
                -2 * a * ((a - 1) + (a + 1) * c),
                a * ((a + 1) + (a - 1) * c - beta),
            ]
            den = [
                (a + 1) - (a - 1) * c + beta,
                2 * ((a - 1) - (a + 1) * c),
                (a + 1) - (a - 1) * c - beta,
            ]
    return np.asarray(b) / den[0], np.asarray(den) / den[0]


@unittest.skipUnless(RENDERER, "Set SOTF_DYNAMIC_EQ_RENDERER")
class DynamicEqNativeTests(unittest.TestCase):
    def test_single_band_output_is_bounded_by_dry_and_full_target(self):
        rng = np.random.default_rng(20541026)
        x = (rng.normal(size=(7001, 2)) * 0.08).astype(np.float32)
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for rate in [44100, 48000, 96000, 192000]:
                for shape in ["peak", "low_shelf", "high_shelf", "tilt"]:
                    for gain in [-12, 12]:
                        p = preset(
                            [{"shape": shape, "gain": gain, "shelf_slope": 0.7}],
                            threshold=-45,
                            ratio=12,
                            attack_ms=0.1,
                            release_ms=100,
                        )
                        y, m = render(
                            x,
                            RENDERER,
                            root / f"{rate}-{shape}-{gain}",
                            p,
                            rate=rate,
                            response_hz=[0, 100, 1000, 10000, rate / 2],
                        )
                        band = m["requested_params"]["bands"][0]
                        b, a = section(
                            shape,
                            band["frequency"],
                            band["q"],
                            gain,
                            band["shelf_slope"],
                            rate,
                        )
                        full = lfilter(b, a, x.astype(float), axis=0)
                        violation = max(
                            float(np.max(np.minimum(x, full) - y)),
                            float(np.max(y - np.maximum(x, full))),
                            0,
                        )
                        self.assertLess(violation, 2e-6)
                        _, h = freqz(
                            b, a, worN=np.array(m["response_hz"]) * 2 * np.pi / rate
                        )
                        error = float(
                            np.max(
                                abs(
                                    20 * np.log10(abs(h))
                                    - m["full_target_band_response_db"][0]
                                )
                            )
                        )
                        self.assertLess(error, 2e-6)
                        print(
                            f"bounds rate={rate} shape={shape} gain={gain} violation={violation:.10g} response_error_db={error:.10g}",
                            flush=True,
                        )

    def test_full_preset_callback_prefix_and_explicit_zero_extension(self):
        x = (np.random.default_rng(20551026).normal(size=(6001, 6)) * 0.05).astype(
            np.float32
        )
        bands = [
            {
                "shape": ["peak", "low_shelf", "high_shelf", "tilt"][i % 4],
                "placement": ["stereo", "left", "right", "mid", "side"][i % 5],
                "frequency": 400 + 300 * i,
                "gain": (-1 if i % 2 else 1) * 6,
            }
            for i in range(8)
        ]
        p = preset(bands, pairs=[[0, 1], [2, 3], [4, 5]], linked=False, mix=0.7)
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            y, _ = render(x, RENDERER, root / "main", p, zeros=513)
            z, _ = render(x, RENDERER, root / "other", p, zeros=513, callback=17)
            np.testing.assert_array_equal(y, z)
            short, _ = render(x[:3331], RENDERER, root / "prefix", p, callback=4096)
            np.testing.assert_array_equal(y[:3331], short)
            long, _ = render(
                np.vstack([x, np.zeros((513, 6), dtype=np.float32)]),
                RENDERER,
                root / "explicit",
                p,
                callback=1,
            )
            np.testing.assert_array_equal(y, long)
            self.assertEqual(y.shape, (6514, 6))

    def test_unity_controls_and_orthogonal_mid_side_are_dry(self):
        x = (np.random.default_rng(20561026).normal(size=(1001, 2)) * 0.05).astype(
            np.float32
        )
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for index, p in enumerate(
                [
                    preset([{}], ratio=1),
                    preset([{}], mix=0),
                    preset([{"active": False}]),
                ]
            ):
                y, _ = render(x, RENDERER, root / f"dry-{index}", p)
                np.testing.assert_array_equal(y, x)
            for placement, signal in [
                ("side", np.column_stack([x[:, 0], x[:, 0]])),
                ("mid", np.column_stack([x[:, 0], -x[:, 0]])),
            ]:
                y, _ = render(
                    signal,
                    RENDERER,
                    root / placement,
                    preset([{"placement": placement}], threshold=-60),
                )
                np.testing.assert_array_equal(y, signal)

    def test_returned_native_parameters_reopen_complete_schema(self):
        x = (np.random.default_rng(20571026).normal(size=(2001, 2)) * 0.04).astype(
            np.float32
        )
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            y, metadata = render(x, RENDERER, root / "original", preset([{}]))
            self.assertIn("stereo_pairs", metadata["requested_params"])
            z, _ = render(
                x,
                RENDERER,
                root / "reopened",
                metadata["requested_params"],
                callback=17,
            )
            np.testing.assert_array_equal(y, z)

    def test_strict_schema_rate_and_routing_errors_create_no_output(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            cases = []
            p = preset([{}])
            p["threshhold"] = -20
            cases.append(p)
            p = preset([{}])
            p["bands"][0]["freqency"] = 1000
            cases.append(p)
            p = preset([{}])
            p["num_bands"] = 2
            cases.append(p)
            p = preset([{"frequency": 20000, "q": 0.1}])
            cases.append(p)
            p = preset([{"placement": "mid"}], pairs=[[0, 1], [1, 0]])
            cases.append(p)
            for i, p in enumerate(cases):
                with self.assertRaises(ValueError):
                    render(np.zeros((10, 2)), RENDERER, root / str(i), p)
                self.assertFalse((root / str(i) / "output.f32le").exists())
            with self.assertRaises(ValueError):
                render(np.full((10, 2), np.nan), RENDERER, root / "nan", preset([{}]))


if __name__ == "__main__":
    unittest.main()
