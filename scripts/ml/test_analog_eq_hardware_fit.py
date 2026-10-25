"""Verify training isolation, immutable candidate scoring and native export gates."""

import copy
import hashlib
import json
import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import fit_analog_eq_hardware as fit
import numpy as np
from analog_eq_cases import session
from analog_eq_fit import decode, encode

RENDERER = os.environ.get("SOTF_ANALOG_EQ_RENDERER")


@unittest.skipUnless(RENDERER, "Set SOTF_ANALOG_EQ_RENDERER")
class FitTests(unittest.TestCase):
    def test_preparation_rejects_identity_bounds_context_and_group_failures(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            spec, data, _ = session(root / "captures", RENDERER)
            for label in ["renderer", "manifest", "bounds", "context", "groups"]:
                bad = copy.deepcopy(spec)
                if label in {"renderer", "manifest"}:
                    bad[
                        "renderer_sha256"
                        if label == "renderer"
                        else "capture_manifest_sha256"
                    ] = "0" * 64
                elif label == "bounds":
                    bad["free"]["low_gain"]["maximum"] = 25
                else:
                    altered = copy.deepcopy(data)
                    if label == "context":
                        altered["context"]["calibration"]["shared_clock"] = False
                    else:
                        altered["captures"] = altered["captures"][1:]
                    manifest = root / (label + ".json")
                    manifest.write_text(json.dumps(altered))
                    bad["capture_manifest"] = str(manifest)
                    bad["capture_manifest_sha256"] = hashlib.sha256(
                        manifest.read_bytes()
                    ).hexdigest()
                output = root / label
                with self.assertRaises(ValueError):
                    fit.run(bad, RENDERER, output, budget=40)
                self.assertFalse(output.exists())

    def exercise(self, root, kind, mutate=False):
        spec, data, target = session(root / "captures", RENDERER, kind)
        spec["template"] = target
        output = root / "fit"
        events = []
        original_read = fit.read_corpus

        def read(subset):
            split = {r["split"] for r in subset["captures"]}
            events.append(split)
            if split != {"train"}:
                self.assertTrue((output / "candidate.json").is_file())
                self.assertFalse((output / "preset.json").exists())
                frozen = json.loads((output / "candidate.json").read_text())
                self.assertEqual(frozen["parameters"], selected)
                if mutate and split == {"test"}:
                    (output / "candidate.json").write_text("{}\n")
            return original_read(subset)

        selected = decode(spec, encode(spec), target["analog_model"])

        def optimizer(objective, actual, budget, method):
            self.assertEqual(events, [{"train"}])
            self.assertFalse((output / "candidate.json").exists())
            loss = objective(selected)
            # Recompute every source's native residual, independently of metrics().
            losses = []
            for i, row in enumerate(data["captures"][:8]):
                y = np.fromfile(
                    output / "search" / f"0-{i}" / "output.f32le", dtype="<f4"
                ).astype(float)
                wet = np.fromfile(row["wet_file"], dtype="<f4").astype(float)
                losses.append(float(np.dot(y - wet, y - wet) / np.dot(wet, wet)))
            self.assertAlmostEqual(loss, float(np.mean(losses)), places=14)
            row = {
                "parameters": selected,
                "model": selected["analog_model"],
                "x": encode(spec).tolist(),
                "loss": loss,
            }
            return {"best": row, "trace": [row], "policy": {}, "evaluations": 1}

        with (
            patch.object(fit, "read_corpus", side_effect=read),
            patch.object(fit, "optimize", side_effect=optimizer),
        ):
            report = fit.run(spec, RENDERER, output, budget=40)
        self.assertEqual(events, [{"train"}, {"validation"}, {"test"}])
        self.assertFalse(report["automatic_apply"])
        self.assertFalse(report["hardware_model_admitted"])
        self.assertEqual(report["native_objective_calls"], 1)
        return output, report

    def test_candidate_is_frozen_before_held_reads_and_native_preset_reopens(self):
        with tempfile.TemporaryDirectory() as folder:
            output, report = self.exercise(Path(folder), "matched")
            self.assertTrue(report["accepted"])
            self.assertEqual(
                (output / "candidate.json").read_bytes(),
                (output / "preset.json").read_bytes(),
            )
            self.assertEqual(len(report["qualification"]), 4)
            self.assertTrue(
                all(row["reopen_error"] == 0 for row in report["qualification"])
            )

    def test_out_of_family_waveform_retains_candidate_but_exports_no_preset(self):
        with tempfile.TemporaryDirectory() as folder:
            output, report = self.exercise(Path(folder), "delay")
            self.assertFalse(report["accepted"])
            self.assertIsNone(report["parameters"])
            self.assertIn("test_waveform_mismatch", report["reasons"])
            self.assertFalse((output / "preset.json").exists())
            self.assertTrue((output / "candidate.json").exists())

    def test_candidate_tampering_aborts_before_preset_export(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            with self.assertRaisesRegex(ValueError, "Frozen candidate changed"):
                self.exercise(root, "matched", mutate=True)
            self.assertFalse((root / "fit" / "preset.json").exists())


if __name__ == "__main__":
    unittest.main()
