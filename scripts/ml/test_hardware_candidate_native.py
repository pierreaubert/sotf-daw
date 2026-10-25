"""Independent neural audio and exact continuously running classical fallback."""

import hashlib
import json
import os
import tempfile
import unittest
from pathlib import Path

import numpy as np
from analog_color_native import controls
from analog_color_native import render as classical_render
from analog_hardware_model import predict
from analyze_analog_hardware_model import reference
from hardware_candidate_native import render

RENDERER = os.environ.get("SOTF_HARDWARE_CANDIDATE_RENDERER")
CLASSICAL = os.environ.get("SOTF_ANALOG_COLOR_RENDERER")
CANDIDATE = (
    Path(__file__).resolve().parents[2]
    / "crates/sotf-plugins/models/analog-hardware/candidate.synthetic.json"
)


@unittest.skipUnless(
    RENDERER and CLASSICAL, "Set both native candidate and analog renderer paths"
)
class NativeTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.model = json.loads(CANDIDATE.read_text())
        cls.sha = hashlib.sha256(CANDIDATE.read_bytes()).hexdigest()
        cls.ctx = cls.model["context"]

    def test_native_neural_matches_independent_scalar_and_numpy_prediction(self):
        rng = np.random.default_rng(20261104)
        x = rng.normal(0, 0.08, 8193).astype(np.float32)
        with tempfile.TemporaryDirectory() as directory:
            y, m = render(
                x,
                RENDERER,
                Path(directory) / "candidate",
                CANDIDATE,
                self.sha,
                self.ctx,
                zeros=4096,
            )
            padded = np.pad(x, (0, 4096))
            scalar, _ = reference(self.model, padded)
            numpy = predict(self.model, padded, self.ctx)
            np.testing.assert_allclose(y, scalar, atol=6e-8, rtol=0)
            np.testing.assert_allclose(y, numpy, atol=6e-8, rtol=0)
            self.assertEqual(m["counts"]["neural"], len(y))
            self.assertTrue(m["candidate_loaded"])

    def test_missing_bad_hash_context_and_disabled_outputs_keep_classical(self):
        rng = np.random.default_rng(20261105)
        x = rng.normal(0, 0.08, 3001).astype(np.float32)
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for model in range(6):
                fallback = controls(
                    model_id=model,
                    drive_db=6,
                    color=0.73,
                    character=0.31,
                    output_trim_db=-3,
                )
                expected, _ = classical_render(
                    x[:, None],
                    CLASSICAL,
                    root / f"classical-{model}",
                    fallback,
                    zeros=512,
                )
                bad = json.loads(json.dumps(self.ctx))
                bad["calibration"]["input_full_scale_dbu"] += 1
                cases = [
                    (None, self.sha, self.ctx, True),
                    (CANDIDATE, "0" * 64, self.ctx, True),
                    (CANDIDATE, self.sha, bad, True),
                    (CANDIDATE, self.sha, self.ctx, False),
                ]
                for i, (path, sha, ctx, enabled) in enumerate(cases):
                    y, m = render(
                        x,
                        RENDERER,
                        root / f"{model}-{i}",
                        path,
                        sha,
                        ctx,
                        fallback=fallback,
                        zeros=512,
                        enabled=enabled,
                    )
                    np.testing.assert_array_equal(y, expected[:, 0])
                    self.assertEqual(m["counts"]["neural"], 0)
                    self.assertFalse(m["hardware_model_admitted"])

    def test_nonfinite_and_peak_domain_history_switches_match_scalar_reference(self):
        rng = np.random.default_rng(20261106)
        x = rng.normal(0, 0.06, 4001).astype(np.float32)
        x[[17, 103, 200, 500]] = [2, np.nan, np.inf, -16]
        clean = np.where(np.isfinite(x), np.clip(x, -16, 16), 0).astype(np.float32)
        valid = np.isfinite(x) & (abs(x) <= self.model["input_peak_limit"])
        invalid = (
            np.convolve(
                (~valid).astype(int),
                np.ones(self.model["architecture"]["taps"], dtype=int),
            )[: len(x)]
            > 0
        )
        neural, _ = reference(self.model, np.where(valid, x, 0))
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            fallback = controls(
                model_id=4, drive_db=6, color=0.73, character=0.31, output_trim_db=-3
            )
            expected, _ = classical_render(
                clean[:, None], CLASSICAL, root / "classical", fallback
            )
            y, m = render(
                x,
                RENDERER,
                root / "candidate",
                CANDIDATE,
                self.sha,
                self.ctx,
                fallback=fallback,
            )
            reference_audio = np.where(invalid, expected[:, 0], neural)
            np.testing.assert_allclose(y, reference_audio, atol=6e-8, rtol=0)
            np.testing.assert_array_equal(y[invalid], expected[invalid, 0])
            self.assertEqual(m["counts"]["out_of_domain"], int(invalid.sum()))

    def test_reset_and_enable_event_timeline_is_callback_invariant(self):
        rng = np.random.default_rng(20261107)
        x = rng.normal(0, 0.08, 7103).astype(np.float32)
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            args = {
                "zeros": 1024,
                "disabled_ranges": [[0, 101], [1200, 1313], [4000, 4200]],
                "reset_frames": [777, 3000],
            }
            y, m = render(
                x, RENDERER, root / "base", CANDIDATE, self.sha, self.ctx, **args
            )
            for block in [1, 17, 4096]:
                other, meta = render(
                    x,
                    RENDERER,
                    root / str(block),
                    CANDIDATE,
                    self.sha,
                    self.ctx,
                    callback=block,
                    **args,
                )
                np.testing.assert_array_equal(y, other)
                self.assertEqual(m["counts"], meta["counts"])
            self.assertEqual(len(m["epoch_counts"]), 3)


if __name__ == "__main__":
    unittest.main()
