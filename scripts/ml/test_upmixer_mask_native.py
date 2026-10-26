"""Native exporter timing, callback identity and independent-feature checks."""

import hashlib
import os
import tempfile
import unittest
from pathlib import Path

import numpy as np
from evaluate_upmixer_mask_native import parity
from upmixer_mask_native import render
from upmixer_source_mask import load

MODEL = (
    Path(__file__).resolve().parents[2]
    / "crates/sotf-plugins/models/upmixer-source-mask/model.synthetic.json"
)


@unittest.skipUnless(
    os.environ.get("SOTF_UPMIXER_MASK_RENDERER"), "native renderer required"
)
class NativeMaskTests(unittest.TestCase):
    def test_native_features_network_timing_and_callback_independence(self):
        renderer = os.environ["SOTF_UPMIXER_MASK_RENDERER"]
        sha = hashlib.sha256(MODEL.read_bytes()).hexdigest()
        theta = load(MODEL, sha)
        rng = np.random.default_rng(904)
        x = rng.normal(0, 0.03, (4097, 2)).astype("<f4")
        with tempfile.TemporaryDirectory() as tmp:
            for layout in ["5.1", "7.1"]:
                a, am = render(
                    x,
                    renderer,
                    Path(tmp) / (layout + "one"),
                    model=MODEL,
                    sha=sha,
                    layout=layout,
                    callback=1,
                )
                b, bm = render(
                    x,
                    renderer,
                    Path(tmp) / (layout + "odd"),
                    model=MODEL,
                    sha=sha,
                    layout=layout,
                    callback=113,
                )
                np.testing.assert_array_equal(a, b)
                self.assertEqual(
                    [r["state"] for r in am["rows"]], [r["state"] for r in bm["rows"]]
                )
                p = parity(x, am, theta)
                self.assertTrue(p["network_gate_pass"])
                self.assertTrue(p["feature_gate_pass"], p)

    def test_bad_hash_keeps_complete_actual_native_output(self):
        renderer = os.environ["SOTF_UPMIXER_MASK_RENDERER"]
        rng = np.random.default_rng(905)
        x = rng.normal(0, 0.03, (4101, 2))
        with tempfile.TemporaryDirectory() as tmp:
            a, _ = render(x, renderer, Path(tmp) / "native")
            b, metadata = render(
                x, renderer, Path(tmp) / "bad", model=MODEL, sha="0" * 64
            )
            np.testing.assert_array_equal(a, b)
            self.assertFalse(metadata["model_loaded"])
            self.assertIsNotNone(metadata["model_error"])
            self.assertEqual(metadata["final_state"]["selected_bands"], 0)


class GeometryTests(unittest.TestCase):
    def test_invalid_native_geometry_rejects_before_output(self):
        for source in [
            np.zeros(20),
            np.ones((20, 3)),
            np.full((20, 2), np.nan),
            np.full((20, 2), 65),
        ]:
            with self.assertRaises(ValueError):
                render(source, "missing", "unused")


if __name__ == "__main__":
    unittest.main()
