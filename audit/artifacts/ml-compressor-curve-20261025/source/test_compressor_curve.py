"""Independent gain-law, model, fitting and actual native plateau checks."""

import hashlib
import json
import math
import os
import subprocess
import tempfile
import unittest
from pathlib import Path

import numpy as np
import torch
from compressor_curve import (
    GRID,
    LOWER,
    UPPER,
    Model,
    dataset,
    fit_curve,
    response,
    validate_target,
)
from fit_compressor_curve import make_preset, validate
from train_compressor_curve import differentiable_response

ROOT = Path(__file__).resolve().parents[2]
MODEL = ROOT / "crates/sotf-plugins/models/compressor-static-fit/model.json"


def independent(p, levels):
    threshold, log_ratio, knee, makeup = p
    ratio, values = np.exp(log_ratio), []
    for level in levels:
        if knee < 0.1:
            output = (
                level if level <= threshold else threshold + (level - threshold) / ratio
            )
        elif level < threshold - knee / 2:
            output = level
        elif level > threshold + knee / 2:
            output = threshold + (level - threshold) / ratio
        else:
            output = level + (1 / ratio - 1) * (level - threshold + knee / 2) ** 2 / (
                2 * knee
            )
        values.append(output + makeup)
    return np.asarray(values)


class CompressorCurveTests(unittest.TestCase):
    def test_gain_law_matches_piecewise_reference_at_knee_boundaries(self):
        for knee in [0, 0.099999, 0.1, 0.100001, 6, 20]:
            p = [-18, np.log(4), knee, 2]
            levels = np.unique(np.r_[GRID, -18 + np.array([-knee / 2, 0, knee / 2])])
            np.testing.assert_allclose(
                response(p, levels), independent(p, levels), atol=2e-14, rtol=0
            )

    def test_torch_response_and_gradients_match_numpy(self):
        p, targets, _ = dataset(16, 20421026)
        parameters = torch.tensor(p, requires_grad=True)
        actual = differentiable_response(parameters)
        np.testing.assert_allclose(actual.detach().numpy(), targets, atol=1e-12, rtol=0)
        actual.square().mean().backward()
        self.assertTrue(torch.isfinite(parameters.grad).all())
        self.assertGreater(float(parameters.grad.abs().sum()), 0)

    def test_fresh_parameter_identities_do_not_overlap(self):
        self.assertFalse(set(dataset(64, 20421027)[2]) & set(dataset(64, 20421028)[2]))

    def test_unity_ratio_returns_response_equivalent_canonical_controls(self):
        result = fit_curve(GRID, GRID + 3, None)
        self.assertEqual(result["method"], "canonical-constant-gain")
        np.testing.assert_array_equal(response(result["parameters"]), GRID + 3)
        self.assertEqual(result["parameters"], [0, 0, 0, 3])

    def test_exact_refinement_and_incompatible_curve_rejection(self):
        p = [-24, np.log(5), 8, -2]
        result = fit_curve(GRID, response(p), None)
        self.assertTrue(result["accepted"])
        self.assertLess(result["max_db"], 1e-7)
        rejected = fit_curve(
            GRID, GRID + np.where(np.arange(len(GRID)) % 2, 2, -2), None
        )
        self.assertFalse(rejected["accepted"])
        self.assertIsNone(rejected["parameters"])

    def test_loader_and_numpy_prediction_agree_with_torch(self):
        digest = hashlib.sha256(MODEL.read_bytes()).hexdigest()
        model = Model(MODEL, digest)
        _, curves, _ = dataset(8, 20421029)
        for curve in curves:
            x = torch.tensor((curve - GRID) / 84)
            for index, (w, b) in enumerate(model.layers):
                x = torch.tensor(w) @ x + torch.tensor(b)
                x = torch.nn.functional.silu(x) if index < 2 else x.tanh()
            expected = LOWER + (x.numpy() + 1) * (UPPER - LOWER) / 2
            np.testing.assert_allclose(
                model.predict(curve), expected, atol=1e-12, rtol=0
            )
        with self.assertRaises(ValueError):
            Model(MODEL, "0" * 64)

    def test_invalid_targets_and_implicit_timing_are_rejected(self):
        with self.assertRaises(ValueError):
            validate_target({"input_dbfs": [0] * 64, "output_dbfs": [0] * 64})
        target = {
            "format": "sotf-compressor-static-target-v1",
            "level_convention": "constant_amplitude_dbfs",
            "sample_rate": 48000,
            "crossovers_hz": [],
            "bands": [{"input_dbfs": GRID.tolist(), "output_dbfs": GRID.tolist()}],
        }
        with self.assertRaises(ValueError):
            validate(target)


@unittest.skipUnless(
    os.environ.get("SOTF_COMPRESSOR_CURVE_RENDERER"),
    "native plateau renderer not configured",
)
class NativeCompressorCurveTests(unittest.TestCase):
    def test_native_hard_knee_switch_at_point_one_db_is_observable(self):
        renderer = os.environ["SOTF_COMPRESSOR_CURVE_RENDERER"]
        with tempfile.TemporaryDirectory() as temporary:
            results = []
            for knee in [0.099999, 0.1, 0.100001]:
                p = [-18, np.log(4), knee, 0]
                source = {"attack_ms": 5, "release_ms": 50}
                preset = make_preset(
                    {"bands": [source], "crossovers_hz": []}, [{"parameters": p}]
                )
                request = {
                    "sample_rate": 48000,
                    "callback_frames": 193,
                    "settle_multiple": 32,
                    "params": preset,
                    "levels_dbfs": [[-18.0]],
                }
                path = Path(temporary) / "request.json"
                path.write_text(json.dumps(request))
                completed = subprocess.run(
                    [renderer, str(path)], check=True, capture_output=True, text=True
                )
                results.append(
                    json.loads(completed.stdout)["curves"][0]["output_dbfs"][0]
                )
            # Native fast_exp2 uses a truncated positive Taylor polynomial:
            # each gain underestimates by at most its documented ~0.08%.
            # Together with the shared log2 error target (~0.0009), this
            # bounds the paired knee jump; an ideal-pow-only bound is wrong.
            ideal_jump = 0.75 * 0.1 / 8
            log_error = 20 * math.log10(2) * 0.0009
            gain_error = -20 * math.log10(1 - 0.0008)
            budget = 0.375 * log_error + 0.75 * log_error**2 / 0.2 + gain_error + 0.0001
            self.assertGreater(ideal_jump - budget, 0)
            self.assertGreater(results[0] - results[1], ideal_jump - budget)
            self.assertLess(results[0] - results[1], ideal_jump + budget)
            self.assertLess(abs(results[1] - results[2]), 1e-5)

    def test_one_to_five_band_presets_and_plateau_partition_settling(self):
        renderer = os.environ["SOTF_COMPRESSOR_CURVE_RENDERER"]
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            levels = np.linspace(-84, 0, 64)
            for rate in [44100, 48000, 96000, 192000]:
                for count in [1, 3, 5]:
                    parameters = [
                        [-18 - index * 3, np.log(2 + index), 2 + index, index - 2]
                        for index in range(count)
                    ]
                    bands = [
                        {
                            "input_dbfs": levels.tolist(),
                            "output_dbfs": independent(p, levels).tolist(),
                            "attack_ms": 5,
                            "release_ms": 50 + index * 10,
                        }
                        for index, p in enumerate(parameters)
                    ]
                    target = {
                        "sample_rate": rate,
                        "crossovers_hz": [200, 2000, 8000, 12000][: count - 1],
                        "bands": bands,
                    }
                    preset = make_preset(
                        target, [{"parameters": p} for p in parameters]
                    )
                    results = []
                    for block, settle in [(31, 32), (257, 64)]:
                        request = {
                            "sample_rate": rate,
                            "callback_frames": block,
                            "settle_multiple": settle,
                            "params": preset,
                            "levels_dbfs": [levels.tolist()] * count,
                        }
                        path = root / "request.json"
                        path.write_text(json.dumps(request))
                        completed = subprocess.run(
                            [renderer, str(path)],
                            check=True,
                            capture_output=True,
                            text=True,
                        )
                        result = json.loads(completed.stdout)
                        self.assertEqual(result["full_preset_latency_samples"], 0)
                        for p, curve in zip(parameters, result["curves"]):
                            np.testing.assert_allclose(
                                curve["output_dbfs"],
                                independent(p, levels),
                                atol=0.05,
                                rtol=0,
                            )
                        results.append(result)
                    for a, b in zip(results[0]["curves"], results[1]["curves"]):
                        np.testing.assert_allclose(
                            a["output_dbfs"], b["output_dbfs"], atol=1e-6, rtol=0
                        )


if __name__ == "__main__":
    unittest.main()
