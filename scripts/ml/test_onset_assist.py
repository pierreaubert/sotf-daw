"""Feature arithmetic, balanced gradients, model identity and causal timing."""

import hashlib
import json
import os
import tempfile
import unittest
from pathlib import Path

import numpy as np
from onset_assist_cases import fixture
from onset_assist_model import export, features, load, objective, predict
from onset_assist_native import render
from train_onset_assist import metrics

RENDERER = os.environ.get("SOTF_ONSET_RENDERER")
MODEL = (
    Path(__file__).resolve().parents[2]
    / "crates/sotf-plugins/models/transient-onset-assist/model.synthetic.json"
)


class OnsetModelTests(unittest.TestCase):
    def test_scalar_features_and_invalid_domains(self):
        np.testing.assert_allclose(
            features(0.01, 0.1, 0.01, 0, 0.1, 0.1, 0.1),
            [0.8, 0, 0.5, 0, 0, 0.5],
            atol=1e-14,
            rtol=0,
        )
        for values in [
            (0, 0, 0, 0, 0, 0, 0),
            (0.01, 0.1, 2, 0, 0.1, 0.1, 0.1),
            (0.1, 0.1, 0.01, 0, 0.1, 0.1, 0.1),
        ]:
            with self.assertRaises(ValueError):
                features(*values)

    def test_balanced_gradient_matches_independent_central_difference(self):
        rng = np.random.default_rng(20291118)
        theta = rng.normal(0, 0.2, 65)
        x = rng.random((31, 6))
        y = np.zeros(31)
        y[[3, 19]] = 1
        _, actual = objective(theta, x, y)
        expected = []
        for i in range(65):
            delta = np.zeros(65)
            delta[i] = 1e-6
            expected.append(
                (objective(theta + delta, x, y)[0] - objective(theta - delta, x, y)[0])
                / 2e-6
            )
        np.testing.assert_allclose(actual, expected, atol=1e-9, rtol=1e-6)

    def test_loader_identity_policy_and_coefficients_reject(self):
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / "model.json"

            def read(d):
                path.write_text(json.dumps(d))
                return load(path, hashlib.sha256(path.read_bytes()).hexdigest())

            data = export(np.zeros(65))
            theta = read(data)
            np.testing.assert_array_equal(predict(theta, np.zeros((3, 6))), 0.5)
            with self.assertRaises(ValueError):
                load(path, "0" * 64)
            for key, value in [
                ("policy", {}),
                ("provenance", "unknown"),
                ("deployment_admitted", True),
                ("output_bias", True),
                ("output_bias", 17),
                ("weights", [[0] * 8] * 5),
            ]:
                with self.assertRaises(ValueError):
                    read(dict(data, **{key: value}))

    def test_event_decision_time_is_window_last_sample_and_never_future(self):
        x = np.zeros((2, 6))
        theta = np.zeros(65)
        theta[-1] = 10
        record = {
            "valid": np.ones(2, bool),
            "x": x,
            "native_p": np.full(2, np.nan),
            "ends": np.array([96, 192]),
            "metadata": {
                "sample_rate": 48000,
                "frames": 192,
                "desired_event_frames": [95],
            },
        }
        result = metrics(theta, x, np.array([1.0, 0.0]), [record])
        self.assertEqual(result["events_detected_within_6ms"], 1)
        self.assertEqual(result["detection_delay_ms_median"], 0)
        record["metadata"]["desired_event_frames"] = [200]
        result = metrics(theta, x, np.array([0.0, 1.0]), [record])
        self.assertEqual(result["events_detected_within_6ms"], 0)

    def test_generated_event_identities_reproduce_and_nuisance_tags_are_separate(self):
        a, m = fixture(20291117)
        b, n = fixture(20291117)
        np.testing.assert_array_equal(a, b)
        self.assertEqual(m, n)
        self.assertEqual(len(m["desired_event_frames"]), 6)
        _, shift = fixture(20291117, kind="noise_bursts")
        self.assertEqual(len(shift["nuisance_event_frames"]), 3)
        self.assertNotEqual(m["input_sha256"], shift["input_sha256"])


@unittest.skipUnless(RENDERER, "Set SOTF_ONSET_RENDERER")
class NativeOnsetTests(unittest.TestCase):
    def test_actual_assisted_audio_and_hop_features_are_callback_independent(self):
        x = np.random.default_rng(20294119).normal(0, 0.02, (5101, 2)).astype("<f4")
        sha = hashlib.sha256(MODEL.read_bytes()).hexdigest()
        theta = load(MODEL, sha)
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            a, ma = render(
                x, RENDERER, root / "a", model=MODEL, sha=sha, attack=60.0, sustain=20.0
            )
            b, mb = render(
                x,
                RENDERER,
                root / "b",
                model=MODEL,
                sha=sha,
                attack=60.0,
                sustain=20.0,
                callback=1,
            )
            np.testing.assert_array_equal(a, b)
            self.assertEqual(
                ma["final_state"]["counters"], mb["final_state"]["counters"]
            )
            for ra, rb in zip(ma["rows"], mb["rows"], strict=True):
                self.assertEqual(ra["end_frame"], rb["end_frame"])
                self.assertEqual(ra["state"]["features"], rb["state"]["features"])
                self.assertEqual(ra["state"]["probability"], rb["state"]["probability"])
                xrow = np.asarray(ra["state"]["features"])[None, :]
                self.assertLess(
                    abs(float(predict(theta, xrow)[0]) - ra["state"]["probability"]),
                    2e-7,
                )

    def test_rejected_and_unsupported_models_keep_actual_classical_shaping(self):
        x = np.random.default_rng(20294118).normal(0, 0.02, 3101).astype("<f4")
        sha = hashlib.sha256(MODEL.read_bytes()).hexdigest()
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            for rate in [48000, 192000]:
                expected, _ = render(
                    x,
                    RENDERER,
                    root / f"base-{rate}",
                    rate=rate,
                    attack=60.0,
                    sustain=20.0,
                )
                for name, identity in [("rejected", "0" * 64), ("valid", sha)]:
                    if name == "valid" and rate != 192000:
                        continue
                    actual, meta = render(
                        x,
                        RENDERER,
                        root / f"{name}-{rate}",
                        rate=rate,
                        model=MODEL,
                        sha=identity,
                        attack=60.0,
                        sustain=20.0,
                    )
                    np.testing.assert_array_equal(actual, expected)
                    self.assertEqual(meta["model_loaded"], name == "valid")


if __name__ == "__main__":
    unittest.main()
