"""Native coloration baseline and explicitly calibrated paired-capture contracts."""

import copy
import hashlib
import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

import numpy as np
from analog_color_native import controls, render
from analog_hardware_capture import (
    FORMAT,
    load_manifest,
    read_corpus,
    rms_to_dbu,
    validate,
)

RENDERER = os.environ.get("SOTF_ANALOG_COLOR_RENDERER")


def corpus(work):
    work = Path(work)
    records = []
    for i, split in enumerate(["train", "validation", "test"]):
        rng = np.random.default_rng(20261031 + i)
        x = rng.normal(0, 0.07, 12001 + i).astype("<f4")
        y = (0.8 * np.tanh(2 * x)).astype("<f4")
        row = {
            "id": f"capture-{i}",
            "source_group": f"source-{i}",
            "split": split,
            "frames": len(x),
        }
        for kind, signal in [("dry", x), ("wet", y)]:
            path = work / f"{kind}-{i}.f32le"
            path.write_bytes(signal.tobytes())
            row[kind + "_file"] = str(path)
            row[kind + "_sha256"] = hashlib.sha256(path.read_bytes()).hexdigest()
        records.append(row)
    return {
        "format": FORMAT,
        "context": {
            "data_origin": "synthetic_validation",
            "sample_rate": 48000,
            "channels": 1,
            "hardware": {
                "manufacturer": "synthetic",
                "model": "tanh-test-only",
                "unit_id": "not-real-hardware",
                "configuration_id": "synthetic-fixed-point",
                "controls": {"gain": 2, "path": "synthetic"},
            },
            "calibration": {
                "calibration_id": "synthetic-level-reference",
                "measurement_reference": "synthetic-only",
                "reference": "full_scale_sine_rms_dbu",
                "shared_clock": True,
                "input_full_scale_dbu": 18.0,
                "output_full_scale_dbu": 24.0,
                "source_impedance_ohms": 100.0,
                "load_impedance_ohms": 10000.0,
            },
            "alignment": {
                "paired_sample_aligned": True,
                "maximum_residual_delay_samples": 0.0,
                "maximum_residual_drift_ppm": 0.0,
                "measurement_reference": "same-generated-sample-clock",
            },
            "rights": {
                "license": "GPL-3.0-or-later",
                "attribution": "project-generated synthetic fixture",
                "reference": "local-generated-test",
                "training_authorized": True,
            },
        },
        "captures": records,
    }


class CaptureTests(unittest.TestCase):
    def test_capture_cli_reports_data_without_admitting_a_hardware_model(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            data = corpus(root)
            manifest = root / "manifest.json"
            manifest.write_text(json.dumps(data))
            output = root / "summary.json"
            command = [
                sys.executable,
                str(Path(__file__).with_name("analog_hardware_capture.py")),
                "--manifest",
                str(manifest),
                "--output",
                str(output),
            ]
            p = subprocess.run(
                command, check=False, capture_output=True, text=True, timeout=30
            )
            self.assertEqual(p.returncode, 0, p.stderr)
            self.assertFalse(json.loads(output.read_text())["hardware_model_admitted"])
            original = output.read_bytes()
            p = subprocess.run(
                command, check=False, capture_output=True, text=True, timeout=30
            )
            self.assertNotEqual(p.returncode, 0)
            self.assertEqual(output.read_bytes(), original)
            data["context"]["calibration"].pop("input_full_scale_dbu")
            manifest.write_text(json.dumps(data))
            command[-1] = str(root / "invalid-summary.json")
            p = subprocess.run(
                command, check=False, capture_output=True, text=True, timeout=30
            )
            self.assertNotEqual(p.returncode, 0)
            self.assertFalse(Path(command[-1]).exists())

    def test_sine_reference_dbu_conversion_and_silence(self):
        self.assertAlmostEqual(rms_to_dbu(1 / np.sqrt(2), 18), 18)
        self.assertAlmostEqual(rms_to_dbu(0.1 / np.sqrt(2), 18), -2)
        self.assertIsNone(rms_to_dbu(0, 18))

    def test_paired_bytes_and_calibration_levels_are_preserved(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            data = corpus(root)
            source = root / "manifest.json"
            source.write_text(json.dumps(data))
            reopened, h = load_manifest(source)
            self.assertEqual(h, hashlib.sha256(source.read_bytes()).hexdigest())
            loaded, summary = read_corpus(reopened)
            self.assertFalse(summary["hardware_origin_declared"])
            self.assertEqual(len(loaded), 3)
            for r, s in zip(data["captures"], summary["captures"], strict=True):
                x, y = loaded[r["id"]]
                self.assertEqual(x.tobytes(), Path(r["dry_file"]).read_bytes())
                self.assertEqual(y.tobytes(), Path(r["wet_file"]).read_bytes())
                self.assertAlmostEqual(
                    s["levels"]["dry"]["rms_dbu"],
                    s["levels"]["dry"]["rms_dbfs"] + 18 + 10 * np.log10(2),
                    places=12,
                )

    def test_calibration_origin_clock_alignment_rights_and_split_refusals(self):
        with tempfile.TemporaryDirectory() as folder:
            data = corpus(folder)
            changes = [
                lambda d: d["context"].update(data_origin="plugin_simulation"),
                lambda d: d["context"].update(channels=True),
                lambda d: d["context"]["calibration"].pop("input_full_scale_dbu"),
                lambda d: d["context"]["calibration"].update(shared_clock=False),
                lambda d: d["context"]["alignment"].update(paired_sample_aligned=False),
                lambda d: d["context"]["alignment"].update(
                    maximum_residual_drift_ppm=10
                ),
                lambda d: d["context"]["rights"].update(training_authorized=False),
                lambda d: d["captures"][1].update(
                    source_group=d["captures"][0]["source_group"]
                ),
                lambda d: d["captures"][0].update(frames=2**63),
            ]
            for change in changes:
                invalid = copy.deepcopy(data)
                change(invalid)
                with self.assertRaises((ValueError, KeyError, TypeError)):
                    validate(invalid)

    def test_hash_duplicate_source_adc_headroom_and_signal_refusals(self):
        with tempfile.TemporaryDirectory() as folder:
            data = corpus(folder)
            for kind in ["hash", "duplicate", "nan", "clip", "silence", "short"]:
                invalid = copy.deepcopy(data)
                r = invalid["captures"][0]
                if kind == "hash":
                    r["dry_sha256"] = "0" * 64
                elif kind == "duplicate":
                    invalid["captures"][1].update(
                        dry_file=r["dry_file"],
                        dry_sha256=r["dry_sha256"],
                        frames=r["frames"],
                    )
                    invalid["captures"][1].update(
                        wet_file=r["wet_file"], wet_sha256=r["wet_sha256"]
                    )
                else:
                    x = np.fromfile(r["dry_file"], dtype="<f4")
                    if kind == "nan":
                        x[0] = np.nan
                    if kind == "clip":
                        x[0] = 1
                    if kind == "silence":
                        x.fill(0)
                    if kind == "short":
                        x = x[:-1]
                    path = Path(folder) / (kind + ".f32le")
                    path.write_bytes(x.tobytes())
                    r.update(
                        dry_file=str(path),
                        dry_sha256=hashlib.sha256(path.read_bytes()).hexdigest(),
                    )
                with self.assertRaises(ValueError):
                    read_corpus(invalid)


@unittest.skipUnless(RENDERER, "Set SOTF_ANALOG_COLOR_RENDERER")
class NativeTests(unittest.TestCase):
    def test_zero_color_identity_all_models_and_rates(self):
        rng = np.random.default_rng(20261032)
        x = rng.normal(0, 0.08, (2003, 2)).astype(np.float32)
        with tempfile.TemporaryDirectory() as folder:
            for rate in [44100, 48000, 96000, 192000]:
                for model in range(6):
                    y, m = render(
                        x,
                        RENDERER,
                        Path(folder) / f"{rate}-{model}",
                        controls(
                            model_id=model,
                            color=0,
                            drive_db=36,
                            character=1,
                            output_trim_db=24,
                        ),
                        rate=rate,
                    )
                    np.testing.assert_array_equal(y, x)
                    self.assertEqual(m["input_sha256"], m["output_sha256"])

    def test_native_json_callbacks_prepared_chunk_and_explicit_zeros(self):
        rng = np.random.default_rng(20261033)
        x = rng.normal(0, 0.025, (5101, 8)).astype(np.float32)
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            for rate in [44100, 192000]:
                for model in range(6):
                    p = controls(
                        model_id=model,
                        drive_db=12,
                        color=0.73,
                        character=0.31,
                        output_trim_db=-3,
                    )
                    y, m = render(
                        x,
                        RENDERER,
                        root / f"base-{rate}-{model}",
                        p,
                        rate=rate,
                        zeros=4096,
                    )
                    reloaded = json.loads(json.dumps(m["effective_controls"]))
                    for callback, maximum in [(17, 1), (4096, 4096)]:
                        b, _ = render(
                            x,
                            RENDERER,
                            root / f"reopen-{rate}-{model}-{callback}",
                            reloaded,
                            rate=rate,
                            callback=callback,
                            maximum=maximum,
                            zeros=4096,
                        )
                        np.testing.assert_array_equal(y, b)

    def test_independent_wet_trim_and_color_blend_laws(self):
        rng = np.random.default_rng(20261034)
        x = rng.normal(0, 0.01, (3101, 1)).astype(np.float32)
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            for model in range(6):
                wet, _ = render(
                    x,
                    RENDERER,
                    root / f"wet-{model}",
                    controls(model_id=model, drive_db=6, character=0.31),
                )
                trimmed, _ = render(
                    x,
                    RENDERER,
                    root / f"trim-{model}",
                    controls(
                        model_id=model, drive_db=6, character=0.31, output_trim_db=-6
                    ),
                )
                np.testing.assert_allclose(
                    trimmed, wet * 10 ** (-6 / 20), atol=2e-7, rtol=0
                )
                blended, _ = render(
                    x,
                    RENDERER,
                    root / f"blend-{model}",
                    controls(
                        model_id=model,
                        drive_db=6,
                        character=0.31,
                        output_trim_db=-6,
                        color=0.37,
                    ),
                )
                np.testing.assert_allclose(
                    blended, x + 0.37 * (trimmed - x), atol=2e-7, rtol=0
                )

    def test_invalid_controls_and_geometry_refuse(self):
        with tempfile.TemporaryDirectory() as folder:
            x = np.zeros((17, 1))
            for i, p in enumerate(
                [
                    controls(model_id=6),
                    controls(output_trim_db=-60),
                    dict(controls(), typo=True),
                ]
            ):
                with self.assertRaises(ValueError):
                    render(x, RENDERER, Path(folder) / str(i), p)


if __name__ == "__main__":
    unittest.main()
