"""Independent SN3D, regularized SVD, angular energy and native audio checks."""

import json
import math
import os
import subprocess
import tempfile
import unittest
from pathlib import Path

import numpy as np
from ambisonics_native import (
    diffuse_weights,
    energy_report,
    normalize_gains,
    preset,
    render,
)
from scipy.signal import butter, sosfilt
from scipy.special import lpmv

RENDERER = os.environ.get("SOTF_AMBISONICS_RENDERER")


def harmonics(order, azimuth, elevation):
    """Independent associated-Legendre ACN/SN3D evaluation without CS phase."""
    azimuth, elevation = np.broadcast_arrays(azimuth, elevation)
    result = []
    for l in range(order + 1):
        for m in range(-l, l + 1):
            a = abs(m)
            scale = math.sqrt(
                (2 if a else 1) * math.factorial(l - a) / math.factorial(l + a)
            )
            phase = np.sin(a * azimuth) if m < 0 else np.cos(a * azimuth)
            result.append(scale * (-1) ** a * lpmv(a, l, np.sin(elevation)) * phase)
    return np.stack(result, axis=-1)


def mode_matching(order, speakers, weighted):
    active = [i for i, s in enumerate(speakers) if not s["is_lfe"]]
    positions = [speakers[i] for i in active]
    y = harmonics(
        order,
        np.deg2rad([s["azimuth_deg"] for s in positions]),
        np.deg2rad([s["elevation_deg"] for s in positions]),
    )
    u, s, vt = np.linalg.svd(y, full_matrices=False)
    inverse = np.where(s > s[0] * 1e-7, s / (s * s + (s[0] * 1e-6) ** 2), 0)
    d = (u * inverse) @ vt
    if weighted:
        root = np.polynomial.legendre.leggauss(order + 1)[0][-1]
        degrees = np.floor(np.sqrt(np.arange((order + 1) ** 2))).astype(int)
        weights = np.array(
            [np.polynomial.legendre.legval(root, [0] * l + [1]) for l in degrees]
        )
        d *= weights
    result = np.zeros((len(speakers), (order + 1) ** 2))
    result[active] = d
    return result


def reference(x, m):
    x = np.asarray(x, dtype=float)
    d = np.array(m["decoder_matrix"]["matrix"]).reshape(m["output_channels"], -1)
    if m["effective_params"]["dual_band"]:
        low = butter(2, 700, fs=m["sample_rate"], output="sos")
        high = butter(2, 700, fs=m["sample_rate"], btype="highpass", output="sos")
        lf = sosfilt(np.concatenate([low, low]), x, axis=0)
        hf = sosfilt(np.concatenate([high, high]), x, axis=0)
        basic = np.array(m["basic_matrix"]["matrix"]).reshape(d.shape)
        decoded = lf @ basic.T + hf @ d.T
    else:
        decoded = x @ d.T
    return decoded * np.array(m["speaker_gains"])


def custom_layout(count=12):
    k = np.arange(count)
    z = 1 - 2 * (k + 0.5) / count
    az = k * np.pi * (3 - np.sqrt(5))
    speakers = [
        {
            "label": f"S{i}",
            "azimuth_deg": float((a * 180 / np.pi + 180) % 360 - 180),
            "elevation_deg": float(np.arcsin(e) * 180 / np.pi),
            "is_lfe": False,
        }
        for i, (a, e) in enumerate(zip(az, z, strict=True))
    ]
    return {"name": f"Synthetic Fibonacci {count}", "speakers": speakers}


class EnergyTests(unittest.TestCase):
    def test_sn3d_addition_theorem_and_uniform_sphere_covariance(self):
        z, w = np.polynomial.legendre.leggauss(10)
        az = np.arange(32) * 2 * np.pi / 32
        h = harmonics(7, az[None, :], np.arcsin(z[:, None]))
        for l in range(8):
            np.testing.assert_allclose(
                np.sum(h[..., l * l : (l + 1) ** 2] ** 2, axis=-1), 1, atol=2e-14
            )
        cov = np.einsum("ijk,ijl,i->kl", h, h, w) / (2 * len(az))
        degree = np.floor(np.sqrt(np.arange(64))).astype(int)
        np.testing.assert_allclose(cov, np.diag(1 / (2 * degree + 1)), atol=2e-14)
        rng = np.random.default_rng(20261025)
        d = rng.normal(size=(5, 64))
        angular = np.einsum("ijs,i->s", (h @ d.T) ** 2, w) / (2 * len(az))
        np.testing.assert_allclose(angular, diffuse_weights(d), atol=1e-13)

    def test_two_band_normalization_and_arbitrary_energy_bound(self):
        rng = np.random.default_rng(20261026)
        matrices = [rng.normal(size=(8, 16)) for _ in range(2)]
        for d in matrices:
            d[3] = 0
        g = normalize_gains(matrices, np.linspace(0.95, 1.05, 8), [3])
        self.assertEqual(g[3], 1)
        r = energy_report(matrices, g)
        for band in r["bands"]:
            self.assertAlmostEqual(band["ratio"], 1, places=6)
        for cross in r["cross_terms"]:
            self.assertLess(abs(cross["normalized_error"]), 2e-6)
        # Arbitrary complex crossover responses include the LF/HF overlap term.
        degree = np.floor(np.sqrt(np.arange(16))).astype(int)
        for low, high in [(1, 0), (0, 1), (0.3 + 0.2j, 0.7 - 0.1j)]:
            composed = low * matrices[0] + high * matrices[1]
            weights = np.sum(abs(composed) ** 2 / (2 * degree + 1), axis=1)
            self.assertLess(
                abs(weights @ g.astype(float) ** 2 / weights.sum() - 1), 2e-6
            )
        x = rng.normal(size=(200, 8))
        ratio = np.sum((x * g) ** 2, axis=1) / np.sum(x * x, axis=1)
        lo, hi = r["arbitrary_output_energy_ratio_bounds"]
        self.assertTrue(np.all((ratio >= lo) & (ratio <= hi)))
        self.assertGreater(np.ptp(ratio), 1e-3)

    def test_invalid_constraints_refuse(self):
        for matrices, gains, lfe in [
            ([], [1] * 6, []),
            ([np.zeros((6, 4))], [1] * 6, []),
            ([np.ones((6, 4))], [1] * 6, [3]),
            ([np.ones((6, 4))], [1] * 5, []),
            ([np.ones((6, 4))], [float("nan")] * 6, []),
        ]:
            with self.assertRaises(ValueError):
                normalize_gains(matrices, gains, lfe)


@unittest.skipUnless(RENDERER, "Set SOTF_AMBISONICS_RENDERER")
class NativeTests(unittest.TestCase):
    def test_native_single_dual_allrad_and_reloaded_callbacks(self):
        rng = np.random.default_rng(20261027)
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for rate in [44100, 48000, 96000, 192000]:
                for algorithm in ["mode_matching", "allrad"]:
                    for dual in [False, True]:
                        p = preset(
                            order=3,
                            target_layout="7.1.4",
                            algorithm=algorithm,
                            dual_band=dual,
                        )
                        x = rng.normal(0, 0.03, (3101, 16)).astype(np.float32)
                        gains = np.linspace(0.95, 1.05, 12)
                        gains[3] = 1
                        name = f"{rate}-{algorithm}-{dual}"
                        y, m = render(
                            x, RENDERER, root / name, p, gains, rate=rate, zeros=1024
                        )
                        oracle = reference(np.pad(x, ((0, 1024), (0, 0))), m)
                        np.testing.assert_allclose(y, oracle, atol=1e-5, rtol=0)
                        np.testing.assert_array_equal(y[:, 3], 0)
                        if algorithm == "mode_matching":
                            expected = mode_matching(
                                3, m["speaker_layout"]["speakers"], True
                            )
                            actual = np.array(m["decoder_matrix"]["matrix"]).reshape(
                                12, 16
                            )
                            np.testing.assert_allclose(
                                actual, expected, atol=2e-7, rtol=0
                            )
                        p2 = json.loads(json.dumps(m["effective_params"]))
                        b, _ = render(
                            x,
                            RENDERER,
                            root / (name + "-reload"),
                            p2,
                            m["speaker_gains"],
                            rate=rate,
                            zeros=1024,
                            callback=4096,
                        )
                        np.testing.assert_array_equal(y, b)

    def test_custom_high_order_and_normalized_correction(self):
        rng = np.random.default_rng(20261028)
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for order in [1, 4, 7]:
                p = preset(
                    order=order,
                    target_layout="custom",
                    custom_layout=custom_layout(32),
                    dual_band=True,
                )
                x = rng.normal(0, 0.02, (2003, (order + 1) ** 2)).astype(np.float32)
                baseline, m = render(
                    x, RENDERER, root / f"base-{order}", p, [1] * 32, zeros=512
                )
                matrices = [
                    np.array(m[n]["matrix"]).reshape(32, -1)
                    for n in ["decoder_matrix", "basic_matrix"]
                ]
                g = normalize_gains(matrices, np.linspace(0.97, 1.03, 32))
                corrected, cm = render(
                    x,
                    RENDERER,
                    root / f"corrected-{order}",
                    p,
                    g,
                    callback=17,
                    zeros=512,
                )
                np.testing.assert_allclose(corrected, baseline * g, atol=3e-8, rtol=0)
                np.testing.assert_allclose(
                    corrected,
                    reference(np.pad(x, ((0, 512), (0, 0))), cm),
                    atol=1e-5,
                    rtol=0,
                )
                expected = mode_matching(order, m["speaker_layout"]["speakers"], True)
                np.testing.assert_allclose(matrices[0], expected, atol=2e-6, rtol=0)
                for band in cm["energy"]["bands"]:
                    self.assertLess(abs(band["ratio"] - 1), 2e-6)
                for cross in cm["energy"]["cross_terms"]:
                    self.assertLess(abs(cross["normalized_error"]), 2e-6)

    def test_native_invalid_schema_normalization_lfe_and_pcm_refuse(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            _, m = render(
                np.zeros((17, 4)), RENDERER, root / "valid", preset(), [1] * 6
            )
            request = json.loads((root / "valid" / "request.json").read_text())
            for i, change in enumerate(
                [
                    {"normalization": "N3D"},
                    {"speaker_gains": [1, 1, 1, 1.1, 1, 1]},
                    {"params": preset(target_layout="5.0")},
                    {"params": dict(preset(), typo=True)},
                    {"speaker_gains": [1] * 5},
                    {"frames": 2**63},
                ]
            ):
                r = dict(request, **change, output=str(root / f"bad-{i}.f32le"))
                path = root / f"bad-{i}.json"
                path.write_text(json.dumps(r))
                process = subprocess.run(
                    [RENDERER, str(path)],
                    check=False,
                    capture_output=True,
                    text=True,
                    timeout=30,
                )
                self.assertNotEqual(process.returncode, 0)
                self.assertFalse(Path(r["output"]).exists())
            for i, value in enumerate([float("nan"), float("inf"), 33.0]):
                pcm = np.zeros((17, 4), dtype="<f4")
                pcm[0, 0] = value
                path = root / f"pcm-{i}.f32le"
                path.write_bytes(pcm.tobytes())
                r = dict(request, input=str(path), output=str(root / f"pcm-out-{i}"))
                source = root / f"pcm-{i}.json"
                source.write_text(json.dumps(r))
                process = subprocess.run(
                    [RENDERER, str(source)],
                    check=False,
                    capture_output=True,
                    text=True,
                    timeout=30,
                )
                self.assertNotEqual(process.returncode, 0)
                self.assertFalse(Path(r["output"]).exists())
            self.assertEqual(m["tail"], "finite_zero")


if __name__ == "__main__":
    unittest.main()
