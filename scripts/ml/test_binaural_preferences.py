"""Synthetic preferences and real native audition/refusal integration."""

import copy
import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

import fit_binaural_preferences as fitter
import h5py
from binaural_preference_cases import session

RENDERER = os.environ.get("SOTF_BINAURAL_RENDERER")


@unittest.skipUnless(RENDERER, "Set SOTF_BINAURAL_RENDERER")
class BinauralPreferenceTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.temporary = tempfile.TemporaryDirectory()
        cls.root = Path(cls.temporary.name)
        cls.data = session(cls.root / "fixtures", RENDERER)

    @classmethod
    def tearDownClass(cls):
        cls.temporary.cleanup()

    def test_consistent_choices_native_export_and_reopening(self):
        result = fitter.run(self.data, RENDERER, self.root / "success")
        self.assertTrue(result["accepted"])
        self.assertEqual(result["selected_id"], "candidate-1")
        self.assertEqual(len(result["reopen_max_errors"]), 8)
        self.assertLess(max(result["reopen_max_errors"]), fitter.REOPEN_MAX_ABS_ERROR)
        self.assertEqual(len(result["native_validation"]["auditions"]), 24)
        chosen = self.data["candidates"][1]["parameters"]
        self.assertEqual(
            fitter.digest((self.root / "success/selected.sofa").read_bytes()),
            chosen["sofa_sha256"],
        )
        preset = json.loads((self.root / "success/preset.json").read_text())
        self.assertEqual(preset["room_model"]["max_order"], 0)
        self.assertFalse(preset["late_reverb_enabled"])
        self.assertTrue((self.root / "success/audition_setup.json").exists())

    def test_ties_source_heterogeneity_and_side_bias_abstain(self):
        for kind in ["ties", "heterogeneous", "side-bias"]:
            data = copy.deepcopy(self.data)
            for t in data["trials"]:
                a = int(t["a"][-1])
                b = int(t["b"][-1])
                p = int(t["programme_id"].split("-")[-1])
                rank = [0, 2, 1] if p < 4 else [2, 0, 1]
                t["choice"] = {
                    "ties": "tie",
                    "side-bias": "a",
                    "heterogeneous": "a" if rank[a] > rank[b] else "b",
                }[kind]
            result = fitter.fit(data)
            self.assertFalse(result["accepted"], kind)
            self.assertIsNone(result["selected_id"])
            self.assertEqual(result["fallback_id"], "candidate-0")

    def test_native_level_mismatch_refuses_without_export(self):
        data = copy.deepcopy(self.data)
        for p in data["programmes"]:
            p["post_gains_db"]["candidate-1"] += 3
        result = fitter.run(data, RENDERER, self.root / "mismatch")
        self.assertFalse(result["accepted"])
        self.assertIn("native_auditions_not_rms_level_matched", result["reasons"])
        self.assertFalse((self.root / "mismatch/preset.json").exists())
        self.assertFalse((self.root / "mismatch/selected.sofa").exists())

    def test_duplicate_waveforms_with_different_file_metadata_are_refused(self):
        data = copy.deepcopy(self.data)
        original = data["candidates"][0]["parameters"]
        path = self.root / "same-transfer.sofa"
        path.write_bytes(Path(original["sofa_file"]).read_bytes())
        with h5py.File(path, "r+") as f:
            f.attrs["Title"] = "Different metadata only"
        data["candidates"][1]["parameters"] = {
            "sofa_file": str(path),
            "sofa_sha256": fitter.digest(path.read_bytes()),
        }
        for p in data["programmes"]:
            p["post_gains_db"]["candidate-1"] = p["post_gains_db"]["candidate-0"]
        result = fitter.run(data, RENDERER, self.root / "duplicate-transfer")
        self.assertFalse(result["accepted"])
        self.assertIn("effectively_duplicate_native_auditions", result["reasons"])

    def test_hashes_groups_context_and_protocol_reject_before_native_work(self):
        data = copy.deepcopy(self.data)
        data["context"]["renderer_sha256"] = "0" * 64
        with self.assertRaises(ValueError):
            fitter.prepare(data, RENDERER)
        data = copy.deepcopy(self.data)
        data["programmes"][0]["audio_sha256"] = "0" * 64
        with self.assertRaises(ValueError):
            fitter.prepare(data, RENDERER)
        data = copy.deepcopy(self.data)
        data["programmes"][1]["audio_sha256"] = data["programmes"][0]["audio_sha256"]
        with self.assertRaises(ValueError):
            fitter.validate(data)
        data = copy.deepcopy(self.data)
        data["candidates"][1]["parameters"]["sofa_sha256"] = data["candidates"][0][
            "parameters"
        ]["sofa_sha256"]
        with self.assertRaises(ValueError):
            fitter.validate(data)
        for key, value in [
            ("head_pose_deg", [1, 0, 0]),
            ("sample_rate", True),
            ("diffuse_field_eq", 0),
        ]:
            data = copy.deepcopy(self.data)
            data["context"][key] = value
            with self.assertRaises(ValueError):
                fitter.validate(data)
        data = copy.deepcopy(self.data)
        data["protocol"]["whole_source_grouping"] = False
        with self.assertRaises(ValueError):
            fitter.validate(data)

    def test_cli_abstention_and_existing_output_preservation(self):
        data = copy.deepcopy(self.data)
        for t in data["trials"]:
            t["choice"] = "tie"
        source = self.root / "tie-session.json"
        source.write_text(json.dumps(data))
        output = self.root / "cli-tie"
        command = [
            sys.executable,
            str(Path(fitter.__file__)),
            "--session",
            str(source),
            "--renderer",
            RENDERER,
            "--output",
            str(output),
        ]
        result = subprocess.run(command, capture_output=True, text=True, check=False)
        self.assertEqual(result.returncode, 2, result.stderr)
        self.assertFalse((output / "preset.json").exists())
        before = (output / "report.json").read_bytes()
        result = subprocess.run(command, capture_output=True, text=True, check=False)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual((output / "report.json").read_bytes(), before)


if __name__ == "__main__":
    unittest.main()
