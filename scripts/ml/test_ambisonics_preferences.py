"""Learned categorical utilities, native auditions and fail-closed exports."""

import copy
import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

import fit_ambisonics_preferences as fitter
import numpy as np
from ambisonics_preference_cases import session

RENDERER = os.environ.get("SOTF_AMBISONICS_RENDERER")


@unittest.skipUnless(RENDERER, "Set SOTF_AMBISONICS_RENDERER")
class AmbisonicsPreferencesTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.temporary = tempfile.TemporaryDirectory()
        cls.root = Path(cls.temporary.name)
        cls.data = session(cls.root / "fixtures", RENDERER)

    @classmethod
    def tearDownClass(cls):
        cls.temporary.cleanup()

    def test_consistent_preferences_export_exact_native_decoder_and_correction(self):
        report = fitter.run(self.data, RENDERER, self.root / "consistent")
        self.assertTrue(report["accepted"])
        self.assertEqual(report["selected_id"], "candidate-1")
        self.assertEqual(len(report["native_validation"]["auditions"]), 24)
        self.assertEqual(report["reopen_max_errors"], [0] * 8)
        self.assertFalse(report["automatic_apply"])
        chain = json.loads((self.root / "consistent/preset.json").read_text())[
            "plugins"
        ]
        self.assertEqual(
            [p["plugin_type"] for p in chain], ["ambisonics_decoder", "matrix"]
        )
        self.assertEqual(
            chain[0]["parameters"], self.data["context"]["decoder_parameters"]
        )
        matrix = np.array(chain[1]["parameters"]["matrix"]).reshape(6, 6)
        np.testing.assert_array_equal(
            np.diag(matrix).astype(np.float32),
            self.data["candidates"][1]["parameters"]["speaker_gains"],
        )
        np.testing.assert_array_equal(matrix - np.diag(np.diag(matrix)), 0)
        model = json.loads((self.root / "consistent/model.json").read_text())
        self.assertEqual(model["data_origin"], "synthetic_validation")
        self.assertEqual(model["context"], self.data["context"])

    def test_ties_heterogeneity_and_side_bias_abstain(self):
        for kind in ["ties", "heterogeneous", "side-bias"]:
            data = copy.deepcopy(self.data)
            for t in data["trials"]:
                a, b = int(t["a"][-1]), int(t["b"][-1])
                source = int(t["programme_id"][-1])
                rank = [0, 2, 1] if source < 4 else [2, 0, 1]
                t["choice"] = (
                    "tie"
                    if kind == "ties"
                    else ("a" if kind == "side-bias" or rank[a] > rank[b] else "b")
                )
            report = fitter.fit(data)
            self.assertFalse(report["accepted"], kind)
            self.assertIsNone(report["selected_id"])
            self.assertEqual(report["fallback_id"], "candidate-0")

    def test_source_equal_weighting_and_insufficient_source_abstention(self):
        expected = fitter.fit(self.data)
        data = copy.deepcopy(self.data)
        source = data["programmes"][0]["id"]
        repeats = [t for t in data["trials"] if t["programme_id"] == source]
        for i in range(20):
            for j, t in enumerate(repeats):
                data["trials"].append(dict(t, id=f"repeat-{i}-{j}"))
        result = fitter.fit(data)
        np.testing.assert_allclose(
            result["model"]["utilities"],
            expected["model"]["utilities"],
            atol=1e-7,
            rtol=0,
        )
        data["programmes"] = data["programmes"][:3]
        sources = {p["id"] for p in data["programmes"]}
        data["trials"] = [t for t in data["trials"] if t["programme_id"] in sources]
        result = fitter.fit(data)
        self.assertFalse(result["accepted"])
        self.assertIn(
            "insufficient_independent_programmes_or_trials", result["reasons"]
        )

    def test_level_or_energy_mismatch_refuses_without_preset(self):
        for kind in ["level", "energy"]:
            data = copy.deepcopy(self.data)
            if kind == "level":
                for p in data["programmes"]:
                    p["post_gains_db"]["candidate-1"] += 3
            else:
                data["candidates"][1]["parameters"]["speaker_gains"] = [
                    1.05,
                    1.05,
                    1.05,
                    1,
                    1.05,
                    1.05,
                ]
            result = fitter.run(data, RENDERER, self.root / kind)
            self.assertFalse(result["accepted"])
            self.assertIn(
                "native_auditions_not_rms_level_matched"
                if kind == "level"
                else "candidate_diffuse_energy_not_normalized",
                result["reasons"],
            )
            self.assertFalse((self.root / kind / "preset.json").exists())

    def test_hash_context_groups_protocol_lfe_and_bound_refusals(self):
        changes = [
            lambda d: d["context"].update(normalization="N3D"),
            lambda d: d["protocol"].update(blinded=False),
            lambda d: d["candidates"][0]["parameters"]["speaker_gains"].__setitem__(
                0, 0.9
            ),
            lambda d: d["candidates"][1]["parameters"]["speaker_gains"].__setitem__(
                3, 0.9
            ),
            lambda d: d["programmes"][1].update(
                audio_sha256=d["programmes"][0]["audio_sha256"]
            ),
            lambda d: d["context"].update(renderer_sha256="0" * 64),
            lambda d: d["programmes"][0].update(audio_sha256="0" * 64),
            lambda d: d["programmes"][0].update(frames=2**63),
            lambda d: d["context"].update(
                decoder_parameters=dict(d["context"]["decoder_parameters"], typo=True)
            ),
        ]
        for change in changes:
            data = copy.deepcopy(self.data)
            change(data)
            with self.assertRaises((ValueError, TypeError)):
                fitter.prepare(data, RENDERER)

    def test_unexcited_native_auditions_and_untrimmed_headroom_refuse(self):
        for kind in ["silent-layout", "raw-headroom"]:
            data = copy.deepcopy(self.data)
            for i, p in enumerate(data["programmes"]):
                x = np.fromfile(p["audio_file"], dtype="<f4").reshape(-1, 4)
                if kind == "silent-layout":
                    x[:, [0, 1, 3]] = 0  # Native planar decoder has no Z sensitivity.
                else:
                    x *= 100
                    p["post_gains_db"] = {
                        k: v - 20 for k, v in p["post_gains_db"].items()
                    }
                path = self.root / f"{kind}-{i}.f32le"
                path.write_bytes(x.tobytes())
                p.update(
                    audio_file=str(path), audio_sha256=fitter.digest(path.read_bytes())
                )
            result = fitter.run(data, RENDERER, self.root / kind)
            self.assertFalse(result["accepted"])
            self.assertIn(
                "effectively_duplicate_native_auditions"
                if kind == "silent-layout"
                else "candidate_or_audition_sample_headroom",
                result["reasons"],
            )
            self.assertFalse((self.root / kind / "preset.json").exists())

    def test_cli_emits_review_preset_and_returns_two_on_abstention(self):
        for kind in ["accept", "abstain"]:
            data = copy.deepcopy(self.data)
            if kind == "abstain":
                for t in data["trials"]:
                    t["choice"] = "tie"
            source = self.root / f"{kind}.json"
            source.write_text(json.dumps(data))
            output = self.root / f"cli-{kind}"
            p = subprocess.run(
                [
                    sys.executable,
                    str(Path(fitter.__file__)),
                    "--session",
                    str(source),
                    "--renderer",
                    RENDERER,
                    "--output",
                    str(output),
                ],
                check=False,
                capture_output=True,
                text=True,
                timeout=120,
            )
            self.assertEqual(p.returncode, 0 if kind == "accept" else 2, p.stderr)
            self.assertEqual((output / "preset.json").exists(), kind == "accept")


if __name__ == "__main__":
    unittest.main()
