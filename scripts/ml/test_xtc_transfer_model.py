"""Independent inverse, kernel, provenance and ambiguous-mute checks."""

import hashlib
import json
import tempfile
import unittest
from pathlib import Path

import numpy as np
from xtc_transfer_model import (
    bounded_inverse,
    cancellation_metrics,
    fit,
    kernel,
    load,
    plant,
    predict,
    save,
)


class XtcTransferTests(unittest.TestCase):
    def test_kernel_wrap_and_measured_anchor_fit(self):
        angles = np.arange(0, 360, 20)
        np.testing.assert_allclose(
            kernel([0, 360, -360], angles, 20)[0],
            kernel([360], angles, 20)[0],
            atol=1e-15,
        )
        ir = np.zeros((18, 2, 512))
        ir[:, 0, 0] = 1 + 0.1 * np.cos(np.deg2rad(angles))
        ir[:, 1, 1] = 1 - 0.1 * np.cos(np.deg2rad(angles))
        model = fit(angles, ir, 20, 1e-6)
        np.testing.assert_allclose(predict(model, angles), ir, atol=2e-6)
        np.testing.assert_allclose(
            predict(model, [0]), predict(model, [360]), atol=1e-15
        )
        for kwargs in [{"elevation_deg": 1}, {"radius_m": 1.0}]:
            with self.assertRaises(ValueError):
                predict(model, [0], **kwargs)

    def test_fft_matrix_ear_and_speaker_ownership_with_explicit_delays(self):
        left, right = np.zeros((2, 512)), np.zeros((2, 512))
        left[0, 1], left[1, 2], right[0, 3], right[1, 4] = 1, 2, 3, 4
        h = plant(left, right)
        b = 17
        np.testing.assert_allclose(
            h[b],
            np.array(
                [
                    [
                        np.exp(-2j * np.pi * b / 2048),
                        3 * np.exp(-6j * np.pi * b / 2048),
                    ],
                    [
                        2 * np.exp(-4j * np.pi * b / 2048),
                        4 * np.exp(-8j * np.pi * b / 2048),
                    ],
                ]
            ),
            atol=1e-15,
        )

    def test_bounded_inverse_matches_independent_linear_solve(self):
        h = np.tile(np.array([[1.0, 0.2j], [0.1, 0.8]], complex), (1025, 1, 1))
        w = bounded_inverse(h)
        # Mean squared singular value equals half the Frobenius squared norm.
        beta = 0.001 * np.sum(abs(h[0]) ** 2) / 2
        reference = np.linalg.solve(
            h[0].conj().T @ h[0] + beta * np.eye(2), h[0].conj().T
        )
        np.testing.assert_allclose(w, np.broadcast_to(reference, w.shape), atol=1e-14)

    def test_common_operator_cap_preserves_steering_and_handles_rank_deficiency(self):
        h = np.tile(np.array([[0.01, 0.002], [0.003, 0.02]], complex), (1025, 1, 1))
        w = bounded_inverse(h)
        beta = 0.001 * np.sum(abs(h[0]) ** 2) / 2
        unbounded = np.linalg.solve(
            h[0].conj().T @ h[0] + beta * np.eye(2), h[0].conj().T
        )
        ratios = w[0] / unbounded
        np.testing.assert_allclose(ratios, np.full((2, 2), ratios[0, 0]), atol=1e-15)
        np.testing.assert_allclose(
            np.linalg.svd(w, compute_uv=False)[:, 0], 4, atol=1e-14
        )
        singular = np.ones((1025, 2, 2), complex)
        self.assertTrue(np.isfinite(bounded_inverse(singular)).all())
        self.assertLessEqual(np.max(np.linalg.norm(w, axis=2)), 4 + 1e-14)

    def test_muting_and_signed_degradation_remain_visible(self):
        h = np.tile(np.array([[1, 0.1], [0.1, 1]], complex), (1025, 1, 1))
        mute = cancellation_metrics(h, np.zeros_like(h), [5])
        self.assertEqual(mute[0]["intended_gain_db"]["kind"], "negative_infinity")
        self.assertEqual(mute[0]["normalized_separation_db"]["kind"], "undefined")
        wrong = np.tile(np.array([[0, 1], [1, 0]], complex), (1025, 1, 1))
        degraded = cancellation_metrics(h, wrong, [5])
        self.assertLess(degraded[0]["normalized_separation_db"]["value"], 0)
        self.assertLess(degraded[0]["raw_crosstalk_attenuation_db"]["value"], 0)

    def test_loading_validates_hash_policy_coefficients_and_provenance(self):
        angles = np.arange(0, 360, 20)
        model = fit(angles, np.zeros((18, 2, 512)), 20, 0.001)
        provenance = {
            "dataset_sha256": "a" * 64,
            "authors": ["test fixture"],
            "terms": "project-generated untrained loader fixture",
        }
        with tempfile.TemporaryDirectory() as tmp:
            p = Path(tmp) / "model.json"
            sha = save(model, p, provenance)
            load(p, sha)
            with self.assertRaises(ValueError):
                load(p, "0" * 64)
            for mutate in [
                lambda d: d.update(deployment_admitted=True),
                lambda d: d["policy"].update(sample_rate=48000),
                lambda d: d["model"]["weights"][0][0].__setitem__(0, True),
                lambda d: d["provenance"].pop("terms"),
            ]:
                save(model, p, provenance)
                document = json.loads(p.read_text())
                mutate(document)
                p.write_text(json.dumps(document))
                with self.assertRaises(ValueError):
                    load(p, hashlib.sha256(p.read_bytes()).hexdigest())


if __name__ == "__main__":
    unittest.main()
