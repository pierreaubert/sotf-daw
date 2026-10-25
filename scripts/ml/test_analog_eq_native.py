"""Independent four-section IIR reference, native color order and preset identity."""

import copy
import os
import tempfile
import unittest
from pathlib import Path

import numpy as np
from analog_color_native import render as color_render
from analog_eq_fit import decode, encode, kernel, posterior, space
from analog_eq_native import RANGES, preset, render
from scipy.signal import sosfilt

RENDERER = os.environ.get("SOTF_ANALOG_EQ_RENDERER")
COLOR = os.environ.get("SOTF_ANALOG_COLOR_RENDERER")


def sections(params, rate):
    result = []
    for kind, f, g, q in [
        ("low", params["low_freq"], params["low_gain"], 0.71),
        ("peak", params["mid1_freq"], params["mid1_gain"], params["mid1_q"]),
        ("peak", params["mid2_freq"], params["mid2_gain"], params["mid2_q"]),
        ("high", params["high_freq"], params["high_gain"], 0.71),
    ]:
        a = 10 ** (g / 40)
        w = 2 * np.pi * f / rate
        c, s = np.cos(w), np.sin(w)
        alpha = s / (2 * q)
        beta = np.sqrt(2 * a) * s
        if kind == "peak":
            b = [1 + alpha * a, -2 * c, 1 - alpha * a]
            den = [1 + alpha / a, -2 * c, 1 - alpha / a]
        elif kind == "low":
            b = [
                a * (a + 1 - (a - 1) * c + beta),
                2 * a * (a - 1 - (a + 1) * c),
                a * (a + 1 - (a - 1) * c - beta),
            ]
            den = [
                a + 1 + (a - 1) * c + beta,
                -2 * (a - 1 + (a + 1) * c),
                a + 1 + (a - 1) * c - beta,
            ]
        else:
            b = [
                a * (a + 1 + (a - 1) * c + beta),
                -2 * a * (a - 1 + (a + 1) * c),
                a * (a + 1 + (a - 1) * c - beta),
            ]
            den = [
                a + 1 - (a - 1) * c + beta,
                2 * (a - 1 - (a + 1) * c),
                a + 1 - (a - 1) * c - beta,
            ]
        result.append(np.r_[b, den] / den[0])
    return np.array(result)


class SearchTests(unittest.TestCase):
    def test_all_numeric_controls_and_categorical_kernel_are_supported(self):
        spec = {
            "template": preset(),
            "models": ["Harmonics", "Static"],
            "free": {
                k: {"minimum": v[0], "maximum": v[1], "scale": "linear"}
                for k, v in RANGES.items()
            },
        }
        self.assertEqual(len(space(spec)), 14)
        point = encode(spec)
        p = decode(spec, point, "Static")
        self.assertEqual(p["analog_model"], "Static")
        np.testing.assert_allclose(
            [p[k] for k in RANGES], [spec["template"][k] for k in RANGES], atol=1e-10
        )
        matrix = kernel(
            np.array([[0], [0]]), np.array([[0], [0]]), ["A", "B"], ["A", "B"]
        )
        np.testing.assert_array_equal(matrix, np.eye(2))

    def test_posterior_reconstructs_training_observations_and_native_rounding_bounds(
        self,
    ):
        rows = [
            {"x": [0.0], "model": "A", "loss": 0.1},
            {"x": [1.0], "model": "A", "loss": 0.001},
        ]
        mean, sigma = posterior(rows, np.array([[0.0], [1.0]]), ["A", "A"])
        np.testing.assert_allclose(mean, [-1, -3], atol=2e-6, rtol=0)
        self.assertTrue(np.all(sigma >= 0))
        spec = {
            "template": preset(analog_color=0.3),
            "models": ["Harmonics"],
            "free": {
                "analog_color": {"minimum": 0.1, "maximum": 0.7, "scale": "linear"}
            },
        }
        for x in [0, 1]:
            self.assertTrue(
                0.1 <= decode(spec, [x], "Harmonics")["analog_color"] <= 0.7
            )
        bad = copy.deepcopy(spec)
        bad["free"]["analog_color"]["minimum"] = -1
        with self.assertRaises(ValueError):
            space(bad)


@unittest.skipUnless(RENDERER, "Set SOTF_ANALOG_EQ_RENDERER")
class NativeTests(unittest.TestCase):
    def test_independent_iir_waveform_and_json_callback_reopening(self):
        rng = np.random.default_rng(20261110)
        x = rng.normal(0, 0.02, (7101, 2)).astype(np.float32)
        p = preset(
            low_gain=3,
            mid1_gain=-4,
            mid1_freq=937,
            mid1_q=1.3,
            mid2_gain=2,
            mid2_q=0.7,
            high_gain=-3,
        )
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for rate in [44100, 48000, 96000, 192000]:
                y, m = render(x, RENDERER, root / f"{rate}", p, rate=rate, zeros=4096)
                expected = sosfilt(
                    sections(p, rate),
                    np.pad(x, ((0, 4096), (0, 0))).astype(float),
                    axis=0,
                )
                np.testing.assert_allclose(y, expected, atol=3e-8, rtol=0)
                other, _ = render(
                    x,
                    RENDERER,
                    root / f"{rate}-reopen",
                    m["effective_params"],
                    rate=rate,
                    callback=4096,
                    zeros=4096,
                )
                np.testing.assert_array_equal(y, other)

    @unittest.skipUnless(COLOR, "Set SOTF_ANALOG_COLOR_RENDERER")
    def test_actual_native_signal_flow_is_eq_then_shared_color_for_all_models(self):
        from analog_color_native import NAMES

        rng = np.random.default_rng(20261111)
        x = rng.normal(0, 0.02, (4101, 1)).astype(np.float32)
        p = preset(low_gain=2, mid1_gain=-1.3, high_gain=-2)
        core = sosfilt(sections(p, 48000), x.astype(float), axis=0).astype(np.float32)
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for i, name in enumerate(NAMES):
                params = dict(
                    p,
                    analog_model=name,
                    analog_drive=6,
                    analog_color=0.73,
                    analog_character=0.31,
                    analog_trim=-3,
                )
                y, _ = render(x, RENDERER, root / f"eq-{i}", params)
                expected, _ = color_render(
                    core,
                    COLOR,
                    root / f"color-{i}",
                    {
                        "model_id": i,
                        "drive_db": 6.0,
                        "color": 0.73,
                        "character": 0.31,
                        "output_trim_db": -3.0,
                    },
                )
                np.testing.assert_allclose(y, expected, atol=6e-8, rtol=0)

    def test_native_identity_and_refusals(self):
        rng = np.random.default_rng(20261112)
        x = rng.normal(0, 0.1, (101, 8)).astype(np.float32)
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            y, _ = render(x, RENDERER, root / "flat", preset())
            np.testing.assert_array_equal(x, y)
            for i, p in enumerate(
                [
                    preset(mid1_q=0),
                    preset(low_freq=19),
                    preset(analog_model="unknown"),
                    dict(preset(), extra=True),
                ]
            ):
                with self.assertRaises(ValueError):
                    render(x, RENDERER, root / str(i), p)


if __name__ == "__main__":
    unittest.main()
