"""Test proposal identity, FIR acceptance and no-export failure contracts."""

import hashlib
import json
import math
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

import numpy as np
from fit_linear_phase_eq import (
    convert,
    ideal_response,
    realize_candidates,
    validate_target,
)
from scipy.io import wavfile


class ProposalTests(unittest.TestCase):
    def test_identity_and_level_contract_are_enforced(self):
        proposal = {
            "accepted": True,
            "target_sha256": "target",
            "parameters": {"filters": [], "auto_gain": {"enabled": False}},
        }
        converted = convert(proposal, "target", 0)
        self.assertTrue(converted["accuracy_mode"])
        self.assertFalse(converted["auto_gain"])
        with self.assertRaises(ValueError):
            realize_candidates(
                {**proposal, "qualified_proposals": [proposal] * 4},
                "target",
                0,
                48000,
                None,
                None,
                Path("unused"),
                Path("unused"),
                193,
            )
        with self.assertRaises(ValueError):
            convert(proposal, "different-target", 0)
        with self.assertRaises(ValueError):
            convert([], "target", 0)
        with self.assertRaises(ValueError):
            convert({**proposal, "accepted": False}, "target", 0)
        with self.assertRaises(ValueError):
            convert(
                {
                    **proposal,
                    "parameters": {"filters": [], "auto_gain": {"enabled": True}},
                },
                "target",
                0,
            )

    def test_invalid_target_is_rejected(self):
        data = {
            "sample_rate": 48000,
            "frequencies_hz": np.geomspace(20, 20000, 128).tolist(),
            "target_db": [0] * 128,
        }
        validate_target(data)
        for invalid in [
            {**data, "sample_rate": 16000},
            {**data, "target_db": [float("nan")] * 128},
            {**data, "frequencies_hz": [20] * 128},
        ]:
            with self.assertRaises(ValueError):
                validate_target(invalid)

    def test_analog_reference_has_known_peak_and_shelf_endpoints(self):
        params = {
            "filters": [
                {"filter_type": "Peak", "frequency": 1000, "q": 1, "gain_db": 6}
            ]
        }
        magnitude = abs(ideal_response(params, np.array([1000]), 48000)[0])
        self.assertAlmostEqual(20 * math.log10(magnitude), 6.0, places=10)
        for kind, at_dc, near_nyquist in [("Lowshelf", 6, 0), ("Highshelf", 0, 6)]:
            params["filters"][0]["filter_type"] = kind
            response = ideal_response(params, np.array([0, 23999.99]), 48000)
            np.testing.assert_allclose(
                20 * np.log10(np.abs(response)), [at_dc, near_nyquist], atol=1e-8
            )


@unittest.skipUnless(
    os.environ.get("SOTF_LPEQ_EXPORTER"),
    "set native QA exporter path for actual FIR integration",
)
class NativeExportTests(unittest.TestCase):
    def run_export(
        self,
        root,
        rate=48000,
        peak=None,
        forged_flat=False,
        block=193,
        alternatives=None,
    ):
        frequencies = np.unique(
            np.r_[np.geomspace(20, 20000, 512), peak[0] if peak else []]
        )
        params = {
            "filters": []
            if peak is None
            else [
                {
                    "filter_type": "Peak",
                    "frequency": peak[0],
                    "q": peak[1],
                    "gain_db": peak[2],
                }
            ]
        }
        target = (
            np.zeros(len(frequencies))
            if forged_flat
            else 20 * np.log10(np.abs(ideal_response(params, frequencies, rate)))
        )
        target_path = root / "target.json"
        raw = json.dumps(
            {
                "sample_rate": rate,
                "frequencies_hz": frequencies.tolist(),
                "target_db": target.tolist(),
            }
        ).encode()
        target_path.write_bytes(raw)
        command = [
            sys.executable,
            str(Path(__file__).with_name("fit_linear_phase_eq.py")),
            "--target",
            str(target_path),
            "--renderer",
            os.environ["SOTF_LPEQ_EXPORTER"],
            "--output",
            str(root / "out"),
            "--work-dir",
            str(root / "work"),
            "--callback-frames",
            str(block),
        ]
        if peak:
            proposal = {
                "accepted": True,
                "target_sha256": hashlib.sha256(raw).hexdigest(),
                "method": "explicit-proposal-fixture",
                "parameters": {
                    "filters": [
                        {
                            "filter_type": "peak",
                            "freq": peak[0],
                            "q": peak[1],
                            "db_gain": peak[2],
                            "order": 2,
                        }
                    ],
                    "auto_gain": {"enabled": False},
                },
            }
            if alternatives is not None:
                proposal["qualified_proposals"] = [
                    {**proposal, "method": label, "parameters": controls}
                    for label, controls in alternatives
                ]
            path = root / "proposal.json"
            path.write_text(json.dumps(proposal))
            command += ["--proposal", str(path)]
        result = subprocess.run(command, check=False, capture_output=True, text=True)
        report = json.loads((root / "out/report.json").read_text())
        return result, report

    def test_flat_needs_no_model_and_exports_exact_native_latency(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            result, report = self.run_export(root)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertTrue(report["accepted"])
            self.assertEqual(report["latency_samples"], 544)
            rate, fir = wavfile.read(root / "out/fir.wav")
            self.assertEqual(rate, 48000)
            self.assertEqual(fir.dtype, np.dtype("float32"))
            self.assertEqual(int(np.argmax(np.abs(fir))), 512)
            self.assertFalse(report["automatic_apply"])

    def test_proposal_reopens_identically_across_callback_partitions(self):
        with tempfile.TemporaryDirectory() as directory:
            roots = [Path(directory) / "one", Path(directory) / "other"]
            reports = []
            for root, block in zip(roots, [1, 193], strict=True):
                root.mkdir()
                result, report = self.run_export(root, peak=[1000, 1, 6], block=block)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertTrue(report["accepted"])
                reports.append(report)
            self.assertEqual(
                reports[0]["checks"]["fir_sha256"], reports[1]["checks"]["fir_sha256"]
            )
            np.testing.assert_array_equal(
                wavfile.read(roots[0] / "out/fir.wav")[1],
                wavfile.read(roots[1] / "out/fir.wav")[1],
            )

    def test_forged_acceptance_cannot_bypass_target_checks(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            result, report = self.run_export(root, peak=[1000, 1, 6], forged_flat=True)
            self.assertEqual(result.returncode, 2)
            self.assertFalse(report["accepted"])
            self.assertIsNone(report["parameters"])
            self.assertFalse((root / "out/preset.json").exists())
            self.assertFalse((root / "out/fir.wav").exists())

    def test_later_candidate_survives_earlier_native_tap_failure(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            narrow = {
                "filters": [
                    {
                        "filter_type": "peak",
                        "freq": 30,
                        "q": 8,
                        "db_gain": 0.45,
                        "order": 2,
                    }
                ],
                "auto_gain": {"enabled": False},
            }
            identity = {"filters": [], "auto_gain": {"enabled": False}}
            result, report = self.run_export(
                root,
                rate=96000,
                peak=[1000, 1, 0],
                alternatives=[("narrow", narrow), ("identity", identity)],
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(report["fit_method"], "identity")
            self.assertFalse(report["native_candidates"][0]["accepted"])
            self.assertEqual(
                report["native_candidates"][0]["native"]["status"], "rejected"
            )
            self.assertTrue(report["native_candidates"][1]["accepted"])
            self.assertEqual(report["selected_candidate_index"], 1)

    def test_invalid_target_candidate_cannot_win_on_kernel_length(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            wrong = {"filters": [], "auto_gain": {"enabled": False}}
            correct = {
                "filters": [
                    {
                        "filter_type": "peak",
                        "freq": 1000,
                        "q": 1,
                        "db_gain": 6,
                        "order": 2,
                    }
                ],
                "auto_gain": {"enabled": False},
            }
            result, report = self.run_export(
                root,
                peak=[1000, 1, 6],
                alternatives=[("wrong-flat", wrong), ("correct", correct)],
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertFalse(report["native_candidates"][0]["checks"]["accepted"])
            self.assertEqual(report["fit_method"], "correct")

    def test_unrealizable_sharp_bass_is_rejected_without_exports(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            result, report = self.run_export(root, rate=96000, peak=[30, 8, 12])
            self.assertEqual(result.returncode, 2)
            self.assertFalse(report["accepted"])
            self.assertEqual(report["native"]["status"], "rejected")
            self.assertFalse((root / "out/preset.json").exists())
            self.assertFalse((root / "out/fir.wav").exists())


if __name__ == "__main__":
    unittest.main()
