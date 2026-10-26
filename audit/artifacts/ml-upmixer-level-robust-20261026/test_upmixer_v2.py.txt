"""Strict v2 loading and actual native selection with preserved v1 behavior."""

import hashlib
import json
import os
import tempfile
import unittest
from pathlib import Path

import numpy as np
from upmixer_mask_cases import fixture
from upmixer_mask_native import render
from upmixer_source_mask import load as load_v1
from upmixer_source_mask import predict
from upmixer_source_mask_v2 import load, observations, save

ROOT = Path(__file__).resolve().parents[2]
MODEL = ROOT / "crates/sotf-plugins/models/upmixer-source-mask-v2/model.synthetic.json"
V1 = ROOT / "crates/sotf-plugins/models/upmixer-source-mask/model.synthetic.json"
LEVEL_MODEL = (
    ROOT
    / "crates/sotf-plugins/models/upmixer-source-mask-level-robust/model.synthetic.json"
)


class V2ContractTests(unittest.TestCase):
    def test_level_robust_asset_is_exactly_independent_of_excitation_coordinate(self):
        sha = hashlib.sha256(LEVEL_MODEL.read_bytes()).hexdigest()
        theta = load(LEVEL_MODEL, sha)
        np.testing.assert_array_equal(theta[40:48], 0)
        x = np.random.default_rng(20305100).uniform(size=(29, 6))
        base = predict(theta, x)
        for value in [0.0, 0.2, 0.7, 1.0]:
            changed = x.copy()
            changed[:, 5] = value
            np.testing.assert_array_equal(predict(theta, changed), base)

    def test_version_binding_and_policy_rejection(self):
        sha = hashlib.sha256(MODEL.read_bytes()).hexdigest()
        self.assertEqual(load(MODEL, sha).shape, (65,))
        with self.assertRaises(ValueError):
            load_v1(MODEL, sha)
        with self.assertRaises(ValueError):
            load(V1, hashlib.sha256(V1.read_bytes()).hexdigest())
        with tempfile.TemporaryDirectory() as work:
            path = Path(work) / "model.json"
            for mutate in [
                lambda d: d["policy"]["excitation"].update(
                    relative_to_peak_band_min=1e-5
                ),
                lambda d: d["policy"].update(fallback="analytic_directness"),
                lambda d: d["provenance"].pop("training_features"),
                lambda d: d["coefficients"].__setitem__(0, True),
                lambda d: d.update(deployment_admitted=True),
            ]:
                save(np.zeros(65), path)
                doc = json.loads(path.read_text())
                mutate(doc)
                path.write_text(json.dumps(doc))
                with self.assertRaises(ValueError):
                    load(path, hashlib.sha256(path.read_bytes()).hexdigest())


@unittest.skipUnless(
    os.environ.get("SOTF_UPMIXER_MASK_RENDERER"), "native renderer required"
)
class V2NativeTests(unittest.TestCase):
    def test_level_robust_candidate_is_callback_independent(self):
        renderer = os.environ["SOTF_UPMIXER_MASK_RENDERER"]
        sha = hashlib.sha256(LEVEL_MODEL.read_bytes()).hexdigest()
        signal, _, _, _ = fixture(20305101, "hard_pan")
        with tempfile.TemporaryDirectory() as work:
            a, am = render(
                signal,
                renderer,
                Path(work) / "one",
                model=LEVEL_MODEL,
                sha=sha,
                callback=1,
            )
            b, bm = render(
                signal,
                renderer,
                Path(work) / "odd",
                model=LEVEL_MODEL,
                sha=sha,
                callback=113,
            )
            self.assertTrue(am["model_loaded"])
            np.testing.assert_array_equal(a, b)
            self.assertEqual(
                [r["state"] for r in am["rows"]], [r["state"] for r in bm["rows"]]
            )

    def test_actual_v2_selection_and_callback_identity(self):
        renderer = os.environ["SOTF_UPMIXER_MASK_RENDERER"]
        sha = hashlib.sha256(MODEL.read_bytes()).hexdigest()
        theta = load(MODEL, sha)
        signal, _, _, _ = fixture(20302100, "direct_only")
        with tempfile.TemporaryDirectory() as work:
            root = Path(work)
            for layout in ["5.1", "7.1"]:
                a, am = render(
                    signal,
                    renderer,
                    root / (layout + "one"),
                    model=MODEL,
                    sha=sha,
                    layout=layout,
                    callback=1,
                )
                b, bm = render(
                    signal,
                    renderer,
                    root / (layout + "odd"),
                    model=MODEL,
                    sha=sha,
                    layout=layout,
                    callback=113,
                )
                np.testing.assert_array_equal(a, b)
                self.assertEqual(
                    [r["state"] for r in am["rows"]], [r["state"] for r in bm["rows"]]
                )
                features, eligible, _ = observations(am)
                masks = np.array(
                    [
                        [v if v is not None else np.nan for v in row["state"]["masks"]]
                        for row in am["rows"]
                    ]
                )
                np.testing.assert_array_equal(np.isfinite(masks), eligible)
                np.testing.assert_allclose(
                    masks[eligible],
                    predict(theta, features[eligible]),
                    atol=2e-7,
                    rtol=0,
                )
                self.assertGreater(am["final_state"]["fallback_bands"], 0)
                self.assertGreater(am["final_state"]["selected_bands"], 0)

    @unittest.skipUnless(
        os.environ.get("SOTF_UPMIXER_V1_RENDERER"), "previous v1 renderer required"
    )
    def test_v1_waveform_and_snapshots_match_previous_immutable_renderer(self):
        old = os.environ["SOTF_UPMIXER_V1_RENDERER"]
        new = os.environ["SOTF_UPMIXER_MASK_RENDERER"]
        sha = hashlib.sha256(V1.read_bytes()).hexdigest()
        with tempfile.TemporaryDirectory() as work:
            for kind in ["matched", "direct_only", "silence"]:
                signal, _, _, _ = fixture(20302101, kind)
                a, am = render(
                    signal, old, Path(work) / (kind + "old"), model=V1, sha=sha
                )
                b, bm = render(
                    signal, new, Path(work) / (kind + "new"), model=V1, sha=sha
                )
                np.testing.assert_array_equal(a, b)
                self.assertEqual(
                    [r["state"] for r in am["rows"]], [r["state"] for r in bm["rows"]]
                )
                self.assertEqual(am["final_state"], bm["final_state"])


if __name__ == "__main__":
    unittest.main()
