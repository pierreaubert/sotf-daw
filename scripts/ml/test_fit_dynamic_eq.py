"""GP algebra, bounded proposals, native export/refusal and validation isolation."""

import copy
import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

import numpy as np
from dynamic_eq_fit import controls, encode, make_preset, optimize, posterior
from dynamic_eq_fit_cases import case
from dynamic_eq_native import preset
from fit_dynamic_eq import digest, prepare, run
from scipy.spatial.distance import cdist

RENDERER = os.environ.get("SOTF_DYNAMIC_EQ_RENDERER")


class SurrogateTests(unittest.TestCase):
    def test_posterior_matches_independent_dense_linear_algebra(self):
        rng = np.random.default_rng(20601026)
        x = rng.uniform(size=(11, 4))
        query = rng.uniform(size=(7, 4))
        y = np.sum((x - 0.3) ** 2, axis=1)
        actual_mu, actual_sigma = posterior(x, y, query)

        def k(a, b):
            z = np.sqrt(5) * cdist(a, b) / 0.25
            return (1 + z + z * z / 3) * np.exp(-z)

        labels = np.log10(y)
        scale = max(float(labels.std()), 0.1)
        system = k(x, x) + np.eye(len(x)) * 1e-6
        cross = k(x, query)
        expected_mu = labels.mean() + cross.T @ np.linalg.solve(
            system, labels - labels.mean()
        )
        variance = 1 - np.einsum("ij,ij->j", cross, np.linalg.solve(system, cross))
        np.testing.assert_allclose(actual_mu, expected_mu, rtol=0, atol=1e-12)
        np.testing.assert_allclose(
            actual_sigma, scale * np.sqrt(variance), rtol=0, atol=1e-12
        )

    def test_explicit_control_mapping_keeps_structure_and_shared_dynamics(self):
        template = preset(
            [{"shape": "tilt", "placement": "side", "gain": 3}], pairs=[[0, 1]], knee=5
        )
        original = copy.deepcopy(template)
        for point in [np.zeros(4), np.ones(4), encode(template)]:
            p = make_preset(template, point)
            self.assertEqual(p["bands"][0]["shape"], "tilt")
            self.assertEqual(p["bands"][0]["placement"], "side")
            self.assertEqual(p["stereo_pairs"], [[0, 1]])
            self.assertEqual(p["knee"], 5)
            self.assertEqual(p["bands"][0]["band_threshold"], p["threshold"])
            self.assertEqual(p["bands"][0]["band_ratio"], p["ratio"])
        self.assertEqual(template, original)
        with self.assertRaises(ValueError):
            controls([0, 0, 0, 2])

    def test_both_methods_improve_bounded_objective_within_allowance(self):
        for method in ["surrogate", "classical"]:
            calls = []
            target = np.array([0.45, 0.4, 0.55, 0.6])

            def objective(x, calls=calls, target=target):
                self.assertTrue(np.all((x >= 0) & (x <= 1)))
                calls.append(x.copy())
                return float(np.sum((x - target) ** 2))

            result = optimize(objective, np.full(4, 0.5), budget=80, method=method)
            self.assertLess(result["best"]["loss"], 0.002)
            self.assertLessEqual(len(calls), 80)
            self.assertEqual(result["evaluations"], len(calls))


@unittest.skipUnless(RENDERER, "Set SOTF_DYNAMIC_EQ_RENDERER")
class NativeFitTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.temp = tempfile.TemporaryDirectory()
        cls.root = Path(cls.temp.name)
        cls.data = case(cls.root / "fixture", RENDERER)
        cls.known = copy.deepcopy(cls.data)
        cls.known["template"] = json.loads(
            (cls.root / "fixture/truth.json").read_text()
        )["hidden_parameters"]

    @classmethod
    def tearDownClass(cls):
        cls.temp.cleanup()

    def test_known_control_export_and_reopening(self):
        # Wiring/admission control: initial dynamics deliberately equal truth.
        # This does not claim discovery of unknown parameters.
        report = run(self.known, RENDERER, self.root / "known", budget=40)
        self.assertTrue(report["accepted"], report["reasons"])
        self.assertEqual(report["search"]["best"]["loss"], 0)
        self.assertTrue((self.root / "known/preset.json").exists())
        self.assertTrue(
            all(c["reopen_max_error"] == 0 for c in report["qualification"])
        )

    def test_validation_reference_never_changes_search(self):
        data = copy.deepcopy(self.known)
        c = data["clips"][-1]
        x = np.fromfile(c["reference_file"], dtype="<f4")
        path = self.root / "shifted-validation.f32le"
        path.write_bytes(np.roll(x, 48).astype("<f4").tobytes())
        c["reference_file"] = str(path)
        c["reference_sha256"] = digest(path.read_bytes())
        bad = run(data, RENDERER, self.root / "validation-shift", budget=40)
        good = (
            json.loads((self.root / "known/report.json").read_text())
            if (self.root / "known/report.json").exists()
            else run(self.known, RENDERER, self.root / "isolation-good", budget=40)
        )
        self.assertEqual(bad["search"], good["search"])
        self.assertFalse(bad["accepted"])
        self.assertIn("validation_waveform_mismatch", bad["reasons"])
        self.assertFalse((self.root / "validation-shift/preset.json").exists())

    def test_dry_reference_abstains_without_preset(self):
        data = copy.deepcopy(self.known)
        for c in data["clips"]:
            c["reference_file"] = c["input_file"]
            c["reference_sha256"] = c["input_sha256"]
        result = run(data, RENDERER, self.root / "dry", budget=40)
        self.assertFalse(result["accepted"])
        self.assertIn("no_material_advantage_over_dry", result["reasons"])
        self.assertFalse((self.root / "dry/preset.json").exists())

    def test_identity_grouping_and_cli_output_preservation(self):
        data = copy.deepcopy(self.data)
        data["clips"][-1]["input_sha256"] = data["clips"][0]["input_sha256"]
        with self.assertRaises(ValueError):
            prepare(data)
        data = copy.deepcopy(self.data)
        data["clips"][0]["reference_sha256"] = "0" * 64
        with self.assertRaises(ValueError):
            prepare(data)
        data = copy.deepcopy(self.data)
        data["template"]["bands"][0]["band_ratio"] = 7
        with self.assertRaises(ValueError):
            prepare(data)
        output = self.root / "existing"
        output.mkdir()
        marker = output / "preset.json"
        marker.write_bytes(b"owned preset")
        source = self.root / "cli.json"
        source.write_text(json.dumps(self.known))
        command = [
            sys.executable,
            "scripts/ml/fit_dynamic_eq.py",
            "--target",
            str(source),
            "--renderer",
            RENDERER,
            "--output",
            str(output),
            "--budget",
            "40",
        ]
        completed = subprocess.run(command, capture_output=True, text=True, check=False)
        self.assertNotEqual(completed.returncode, 0)
        self.assertEqual(marker.read_bytes(), b"owned preset")


if __name__ == "__main__":
    unittest.main()
