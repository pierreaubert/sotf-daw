"""Independent numerical and contract checks for the offline source mask."""

import hashlib
import json
import tempfile
import unittest
from pathlib import Path

import numpy as np
from upmixer_mask_cases import fixture
from upmixer_source_mask import (
    analyze,
    apply_mask,
    evidence,
    load,
    objective,
    predict,
    save,
    select_masks,
    synthesize,
    targets,
)


class SourceMaskTests(unittest.TestCase):
    def test_overlap_add_identity_and_source_additivity(self):
        x, d, a, _ = fixture(20295100)
        s = analyze(x)
        np.testing.assert_allclose(synthesize(s, len(x)), x, atol=2e-16)
        np.testing.assert_allclose(s, analyze(d) + analyze(a), atol=3e-15)
        mask = np.ones((len(s), 32))
        np.testing.assert_array_equal(apply_mask(s, mask), s)

    def test_exact_covariance_extremes_and_silent_fallback(self):
        rng = np.random.default_rng(811)
        mono = rng.normal(0, 0.1, 1501)
        for stereo, expected in [
            (np.column_stack([mono, mono]), 1),
            (np.column_stack([mono, -mono]), 0),
            (np.column_stack([mono, mono * 0]), 1),
        ]:
            x, baseline, valid = evidence(analyze(stereo))
            self.assertTrue(np.all((x >= 0) & (x <= 1)))
            np.testing.assert_allclose(baseline[valid], expected, atol=1e-14)
        s = analyze(np.zeros((1501, 2)))
        selected, baseline, valid = select_masks(np.zeros(65), s)
        self.assertFalse(valid.any())
        np.testing.assert_array_equal(selected, baseline)

    def test_missing_invalid_model_keeps_analytic_source_masks(self):
        x, _, _, _ = fixture(20295101)
        s = analyze(x)
        for theta in [None, np.ones(65) * np.nan, np.zeros(64)]:
            selected, baseline, _ = select_masks(theta, s)
            np.testing.assert_array_equal(selected, baseline)

    def test_gradient_matches_independent_central_differences(self):
        rng = np.random.default_rng(812)
        theta, x, y = (
            rng.normal(0, 0.1, 65),
            rng.uniform(size=(19, 6)),
            rng.uniform(size=19),
        )
        _, grad = objective(theta, x, y)
        numerical = []
        for i in range(65):
            delta = np.zeros(65)
            delta[i] = 1e-5
            numerical.append(
                (objective(theta + delta, x, y)[0] - objective(theta - delta, x, y)[0])
                / 2e-5
            )
        np.testing.assert_allclose(grad, numerical, atol=2e-10, rtol=2e-7)

    def test_model_loading_hash_policy_bool_and_bounds(self):
        with tempfile.TemporaryDirectory() as tmp:
            p = Path(tmp) / "model.json"
            sha = save(np.zeros(65), p)
            self.assertEqual(predict(load(p, sha), np.zeros((1, 6)))[0], 0.5)
            with self.assertRaises(ValueError):
                load(p, "0" * 64)
            for mutate in [
                lambda d: d["policy"].update(fft_size=1024),
                lambda d: d.update(deployment_admitted=True),
                lambda d: d["coefficients"].__setitem__(0, True),
                lambda d: d["coefficients"].__setitem__(0, 17),
            ]:
                save(np.zeros(65), p)
                data = json.loads(p.read_text())
                mutate(data)
                p.write_text(json.dumps(data))
                with self.assertRaises(ValueError):
                    load(p, hashlib.sha256(p.read_bytes()).hexdigest())

    def test_identical_observations_can_have_opposite_source_labels(self):
        x, _, _, _ = fixture(20295102, "direct_only")
        s, zero = analyze(x), analyze(np.zeros_like(x))
        # Identical centered audio could be intended direct or a correlated bed.
        direct_labels, bed_labels = targets(s, zero), targets(zero, s)
        _, _, valid = evidence(s)
        np.testing.assert_array_equal(direct_labels[valid], 1)
        np.testing.assert_array_equal(bed_labels[valid], 0)


if __name__ == "__main__":
    unittest.main()
