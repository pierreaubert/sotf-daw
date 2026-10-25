"""Independent bilinear response, mono identity and native reconstruction checks."""

import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path

import numpy as np
from scipy.signal import lfilter
from stereo_imager_native import preset, render

RENDERER = os.environ.get("SOTF_STEREO_IMAGER_RENDERER")


def lowpass(frequency, rate):
    k = np.tan(np.pi * frequency / rate)
    return [k / (1 + k)] * 2, [1, (k - 1) / (k + 1)]


def reference(signal, parameters, rate):
    """Independent f64 bilinear sections and complementary M/S transfer."""
    p = parameters
    x = np.asarray(signal, dtype=float)
    mid = (x[:, 0] + x[:, 1]) / 2
    side = (x[:, 0] - x[:, 1]) / 2
    lo = lfilter(*lowpass(p["low_mid_freq"], rate), side)
    upper = lfilter(*lowpass(p["mid_high_freq"], rate), side)
    low_weight = 0 if p["mono_bass"] else p["low_width"]
    shaped = p["width"] * (
        side * p["high_width"]
        + lo * (low_weight - p["mid_width"])
        + upper * (p["mid_width"] - p["high_width"])
    )
    out_side = side + p["mix"] * (shaped - side)
    return np.column_stack([mid + out_side, mid - out_side])


@unittest.skipUnless(RENDERER, "Set SOTF_STEREO_IMAGER_RENDERER")
class StereoImagerNativeTests(unittest.TestCase):
    def test_native_response_matches_independent_bilinear_oracle_and_keeps_mid(self):
        rng = np.random.default_rng(20831026)
        x = (rng.normal(size=(7001, 2)) * 0.07).astype(np.float32)
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for rate in [44100, 48000, 96000, 192000]:
                for bass in [False, True]:
                    for cutoff in [80, 1000]:
                        p = preset(
                            width=1.3,
                            low_width=0.4,
                            mid_width=1.1,
                            high_width=1.7,
                            mono_bass=bass,
                            mix=0.65,
                            low_mid_freq=cutoff,
                            mid_high_freq=10000,
                        )
                        y, m = render(
                            x,
                            RENDERER,
                            root / f"{rate}-{bass}-{cutoff}",
                            p,
                            rate=rate,
                            zeros=1024,
                        )
                        padded = np.pad(x, ((0, 1024), (0, 0)))
                        expected = reference(padded, m["effective_params"], rate)
                        np.testing.assert_allclose(y, expected, atol=3e-7, rtol=0)
                        np.testing.assert_allclose(
                            y.astype(float).mean(axis=1),
                            padded.astype(float).mean(axis=1),
                            atol=5e-8,
                            rtol=0,
                        )

    def test_exact_neutral_dry_and_mono_routes(self):
        rng = np.random.default_rng(20841026)
        x = (rng.normal(size=(4001, 2)) * 0.1).astype(np.float32)
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for key, p in {"neutral": preset(), "dry": preset(width=2, mix=0)}.items():
                y, m = render(x, RENDERER, root / key, p)
                self.assertEqual(m["input_sha256"], m["output_sha256"])
                np.testing.assert_array_equal(y, x)
            y, _ = render(x, RENDERER, root / "mono", preset(width=0))
            mid = (x[:, 0] + x[:, 1]) * np.float32(0.5)
            np.testing.assert_array_equal(y[:, 0], mid)
            np.testing.assert_array_equal(y[:, 1], mid)

    def test_callback_and_json_preset_reconstruction_agree(self):
        rng = np.random.default_rng(20851026)
        x = (rng.normal(size=(15001, 2)) * 0.08).astype(np.float32)
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for rate in [44100, 192000]:
                p = preset(
                    width=1.6,
                    low_width=0.25,
                    mid_width=0.9,
                    high_width=1.8,
                    mono_bass=True,
                    mix=0.7,
                )
                a, m = render(
                    x,
                    RENDERER,
                    root / f"{rate}-17",
                    p,
                    rate=rate,
                    callback=17,
                    zeros=4096,
                )
                reloaded = json.loads(json.dumps(m["effective_params"]))
                for block in [193, 4096]:
                    b, _ = render(
                        x,
                        RENDERER,
                        root / f"{rate}-{block}",
                        reloaded,
                        rate=rate,
                        callback=block,
                        zeros=4096,
                    )
                    np.testing.assert_array_equal(a, b)

    def test_complex_impulse_response_and_explicit_zero_extension(self):
        x = np.zeros((512, 2), dtype=np.float32)
        x[0] = [1, -1]
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for rate in [44100, 192000]:
                p = preset(
                    width=1.4,
                    low_width=0.3,
                    mid_width=0.8,
                    high_width=1.5,
                    mono_bass=True,
                    low_mid_freq=80,
                    mid_high_freq=4000,
                    mix=0.8,
                )
                y, m = render(x, RENDERER, root / str(rate), p, rate=rate, zeros=32768)
                f = np.array([0, 20, 250, 4000, 16000, rate / 2])
                z = np.exp(-2j * np.pi * f / rate)

                def lp(fc, rate=rate, z=z):
                    b, a = lowpass(fc, rate)
                    return (b[0] + b[1] * z) / (a[0] + a[1] * z)

                expected = (1 - p["mix"]) + p["mix"] * p["width"] * (
                    p["high_width"]
                    - p["mid_width"] * lp(p["low_mid_freq"])
                    + (p["mid_width"] - p["high_width"]) * lp(p["mid_high_freq"])
                )
                actual = np.array(
                    [
                        np.dot(
                            y[:, 0].astype(float),
                            np.exp(-2j * np.pi * frequency * np.arange(len(y)) / rate),
                        )
                        for frequency in f
                    ]
                )
                np.testing.assert_allclose(actual, expected, atol=2e-6, rtol=0)
                self.assertEqual(m["output_frames"], len(x) + 32768)
                self.assertEqual(m["latency_samples"], 0)

    def test_invalid_schema_geometry_and_native_ranges_refuse(self):
        x = np.zeros((101, 2), dtype=np.float32)
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for index, p in enumerate(
                [
                    {},
                    preset(width=2.1),
                    preset(low_mid_freq=20),
                    preset(low_mid_freq=1000, mid_high_freq=1000),
                    dict(preset(), unrecognized=True),
                ]
            ):
                with self.assertRaises(ValueError):
                    render(x, RENDERER, root / str(index), p)
            render(x, RENDERER, root / "valid", preset())
            request = json.loads((root / "valid/request.json").read_text())
            request["channels"] = 1
            path = root / "invalid.json"
            path.write_text(json.dumps(request))
            result = subprocess.run(
                [RENDERER, str(path)], capture_output=True, timeout=10, check=False
            )
            self.assertNotEqual(result.returncode, 0)


if __name__ == "__main__":
    unittest.main()
