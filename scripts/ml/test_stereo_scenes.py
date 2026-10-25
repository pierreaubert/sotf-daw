"""Grouped scene model checks and real native proposal/rejection paths."""

import copy
import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

import numpy as np
from stereo_scene_cases import CONTEXT, session, signal
from stereo_scenes import (
    digest,
    features,
    fit,
    load_model,
    prepare,
    propose,
    score,
    solve,
    validate_model,
)

RENDERER = os.environ.get("SOTF_STEREO_IMAGER_RENDERER")


@unittest.skipUnless(RENDERER, "Set SOTF_STEREO_IMAGER_RENDERER")
class StereoSceneTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.temp = tempfile.TemporaryDirectory()
        cls.root = Path(cls.temp.name)
        cls.data = session(cls.root / "training", RENDERER)
        cls.result = fit(cls.data)
        if not cls.result["accepted"]:
            raise AssertionError(cls.result["reasons"])
        cls.model = cls.result["model"]

    @classmethod
    def tearDownClass(cls):
        cls.temp.cleanup()

    def test_grouped_fit_and_weighted_normal_equations(self):
        self.assertEqual(self.result["confident_errors"], 0)
        self.assertGreaterEqual(min(self.result["class_source_coverage"]), 0.25)
        data = copy.deepcopy(self.data)
        data["records"].append(dict(data["records"][0], id="same-source-extra-record"))
        x, labels, groups, names = prepare(data)
        _, inverse, counts = np.unique(groups, return_inverse=True, return_counts=True)
        w = 1 / counts[inverse]
        # Unequal source record counts: reconstruct the covariance via
        # a weighted augmented design rather than the model's einsum scorer.
        m = solve(x, labels, w, len(names))
        center = np.average(x, axis=0, weights=w)
        scale = np.maximum(
            np.sqrt(np.average((x - center) ** 2, axis=0, weights=w)), 0.05
        )
        z = (x - center) / scale
        means = np.array(
            [
                np.average(z[labels == c], axis=0, weights=w[labels == c])
                for c in range(len(names))
            ]
        )
        residual = (z - means[labels]) * np.sqrt(w / w.sum())[:, None]
        covariance = residual.T @ residual * 0.8 + np.eye(16) * 0.2
        np.testing.assert_allclose(
            np.asarray(m["precision"]) @ covariance, np.eye(16), atol=2e-14, rtol=0
        )
        np.testing.assert_allclose(m["means"], means, atol=1e-14, rtol=0)

    def test_features_gain_invariance_full_tail_and_excitation(self):
        x = signal("diffuse", 20891026, seconds=1.003)
        np.testing.assert_allclose(
            features(x, CONTEXT), features(x * 0.125, CONTEXT), atol=1e-12, rtol=0
        )
        changed = x.copy()
        changed[-1] = [0.9, -0.9]
        self.assertGreater(
            np.max(abs(features(changed, CONTEXT) - features(x, CONTEXT))), 1e-8
        )
        with self.assertRaises(ValueError):
            features(np.zeros((48000, 2)), CONTEXT)
        with self.assertRaises(ValueError):
            features(x[:100], CONTEXT)

    def test_identity_conflicts_and_group_shortage_refuse(self):
        data = copy.deepcopy(self.data)
        for record in data["records"]:
            record["source_group"] = "one-source"
        self.assertFalse(fit(data)["accepted"])
        data = copy.deepcopy(self.data)
        a, b = data["records"][:2]
        b.update(audio_file=a["audio_file"], audio_sha256=a["audio_sha256"])
        with self.assertRaises(ValueError):
            fit(data)

    def test_native_export_headroom_and_context_fallback(self):
        x = signal("centered", 20901026)
        result = propose(x, CONTEXT, self.model, RENDERER, self.root / "native-ok")
        self.assertTrue(result["accepted"], result["reasons"])
        self.assertEqual(result["scene"], "centered")
        self.assertEqual(result["native_reopen_error"], 0)
        self.assertFalse(result["automatic_apply"])
        loud = propose(x * 10, CONTEXT, self.model, RENDERER, self.root / "native-loud")
        self.assertFalse(loud["accepted"])
        self.assertIn("insufficient_sample_headroom", loud["reasons"])
        for model, ctx, renderer in [
            (None, CONTEXT, RENDERER),
            (self.model, dict(CONTEXT, scene_definition_id="another-task"), RENDERER),
            (self.model, CONTEXT, self.root / "no-renderer"),
        ]:
            value = propose(x, ctx, model, renderer, self.root / "unused")
            self.assertFalse(value["accepted"])
            self.assertIsNone(value["suggested_params"])

    def test_model_loader_and_successful_cli_only_export_files(self):
        raw = (json.dumps(self.model, indent=2, allow_nan=False) + "\n").encode()
        path = self.root / "model.json"
        path.write_bytes(raw)
        self.assertEqual(
            load_model(path, digest(raw))["class_names"], self.model["class_names"]
        )
        with self.assertRaises(ValueError):
            load_model(path, "0" * 64)
        x = signal("side_dominant", 20911026)
        audio = self.root / "proposal.f32le"
        audio.write_bytes(x.astype("<f4").tobytes())
        context = self.root / "context.json"
        context.write_text(json.dumps(CONTEXT))
        output = self.root / "cli"
        argv = [
            sys.executable,
            "scripts/ml/propose_stereo_scene.py",
            "--audio",
            str(audio),
            "--frames",
            str(len(x)),
            "--context",
            str(context),
            "--model",
            str(path),
            "--model-sha256",
            digest(raw),
            "--renderer",
            RENDERER,
            "--output",
            str(output),
        ]
        result = subprocess.run(
            argv, capture_output=True, text=True, timeout=120, check=False
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        report = json.loads((output / "report.json").read_text())
        self.assertTrue(report["accepted"])
        self.assertEqual(
            json.loads((output / "preset.json").read_text())["parameters"],
            report["suggested_params"],
        )
        # Existing directories and caller-owned files are never replaced.
        marker = output / "user.txt"
        marker.write_text("keep")
        again = subprocess.run(argv, capture_output=True, timeout=120, check=False)
        self.assertNotEqual(again.returncode, 0)
        self.assertEqual(marker.read_text(), "keep")

    def test_classification_scores_do_not_imply_preferred_width(self):
        x = signal("side_dominant", 20921026)
        f = features(x, CONTEXT)
        first = score(self.model["parameters"], f)
        second = score(self.model["parameters"], f.copy())
        self.assertEqual(first, second)
        # Identical PCM used as desired width versus unwanted side spill cannot
        # reveal which native policy a listener would prefer.
        self.assertTrue(first["confident"])

    def test_model_schema_precision_policy_and_provenance_refuse(self):
        for kind in ["precision", "policy", "sources", "controls", "automatic"]:
            m = copy.deepcopy(self.model)
            if kind == "precision":
                m["parameters"]["main"]["precision"][0][1] += 1
            elif kind == "policy":
                m["policy"]["minimum_lower_score"] = 0.1
            elif kind == "sources":
                m["training_identities"][0]["source_group"] = ""
            elif kind == "controls":
                m["settings_by_scene"]["centered"]["low_mid_freq"] = 20
            else:
                m["automatic_apply"] = True
            with self.assertRaises(ValueError, msg=kind):
                validate_model(m)


if __name__ == "__main__":
    unittest.main()
