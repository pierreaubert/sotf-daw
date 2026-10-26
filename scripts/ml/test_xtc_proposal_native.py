"""Independent WOLA identity and actual native prepared-resource parity."""

import hashlib
import os
import tempfile
import unittest
from pathlib import Path

import numpy as np
from xtc_proposal_native import coefficients, render, wola_reference

ROOT = Path(__file__).resolve().parents[2]
PROPOSAL = ROOT / "audit/artifacts/ml-xtc-transfers-20261026/proposal.json"
MODEL_SHA = "157b1f395717fcebb669d57e61450214823695f520dcef83545d35ce7c161248"


class ReferenceTests(unittest.TestCase):
    def test_identity_and_nonsymmetric_constant_matrix_reconstruct_delayed_source(self):
        rng = np.random.default_rng(531)
        x = rng.normal(0, 0.003, (5101, 2)).astype("<f4")
        for matrix in [np.eye(2), np.array([[0.5, 0.25], [-0.1, 0.75]])]:
            raw = np.zeros((1025, 2, 2, 2))
            raw[..., 0] = matrix
            metadata = {
                "filters_real_imag": raw.tolist(),
                "output_frames": 9216,
                "analysis_window": (
                    0.5 - 0.5 * np.cos(2 * np.pi * np.arange(2048) / 2048)
                ).tolist(),
                "output_scale": 1 / (1.5 * 2048),
            }
            y = wola_reference(x, metadata)
            np.testing.assert_allclose(
                y[2048 : 2048 + len(x)], x @ matrix.T, atol=8e-18
            )
            self.assertTrue(np.all(y[:2048] == 0))
            np.testing.assert_allclose(y[2048 + len(x) :], 0, atol=5e-18)


@unittest.skipUnless(
    os.environ.get("SOTF_XTC_PROPOSAL_RENDERER"), "native renderer required"
)
class NativeTests(unittest.TestCase):
    def test_actual_proposal_waveforms_match_independent_wola_and_callbacks(self):
        rng = np.random.default_rng(532)
        x = rng.normal(0, 0.003, (5101, 2))
        sha = hashlib.sha256(PROPOSAL.read_bytes()).hexdigest()
        with tempfile.TemporaryDirectory() as tmp:
            a, am = render(
                x,
                os.environ["SOTF_XTC_PROPOSAL_RENDERER"],
                Path(tmp) / "one",
                proposal=PROPOSAL,
                sha=sha,
                model_sha=MODEL_SHA,
                callback=1,
            )
            b, bm = render(
                x,
                os.environ["SOTF_XTC_PROPOSAL_RENDERER"],
                Path(tmp) / "odd",
                proposal=PROPOSAL,
                sha=sha,
                model_sha=MODEL_SHA,
                callback=257,
            )
            np.testing.assert_array_equal(a, b)
            np.testing.assert_array_equal(coefficients(am), coefficients(bm))
            np.testing.assert_allclose(a, wola_reference(x, am), atol=2e-7, rtol=0)
            self.assertEqual(am["selected_proposal_sha256"], sha)

    def test_rejected_proposal_matches_actual_native_fallback(self):
        x = np.random.default_rng(533).normal(0, 0.003, (4101, 2))
        with tempfile.TemporaryDirectory() as tmp:
            a, _ = render(
                x, os.environ["SOTF_XTC_PROPOSAL_RENDERER"], Path(tmp) / "native"
            )
            b, metadata = render(
                x,
                os.environ["SOTF_XTC_PROPOSAL_RENDERER"],
                Path(tmp) / "bad",
                proposal=PROPOSAL,
                sha="0" * 64,
                model_sha=MODEL_SHA,
            )
            np.testing.assert_array_equal(a, b)
            self.assertIsNone(metadata["selected_proposal_sha256"])
            self.assertIsNotNone(metadata["proposal_error"])


if __name__ == "__main__":
    unittest.main()
