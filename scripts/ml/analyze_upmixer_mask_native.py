"""Independent native mask-export diagnosis; retain every fixed-gate failure."""

import argparse
import hashlib
import json
from collections import Counter
from pathlib import Path

import numpy as np
from upmixer_mask_cases import fixture
from upmixer_source_mask import analyze, evidence, load, predict


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--report", required=True, type=Path)
    parser.add_argument("--model", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    report = json.loads(args.report.read_text())
    theta = load(args.model, report["model_sha256"])
    diagnoses = []
    raw_hashes_verified = 0
    for row in report["cases"]:
        for files in [row["selected_files"], row["native_files"]]:
            for file in files.values():
                if (
                    hashlib.sha256(Path(file["path"]).read_bytes()).hexdigest()
                    != file["sha256"]
                ):
                    raise RuntimeError("Native evidence changed after export")
                raw_hashes_verified += 1
        identity = row["identity"]
        source, _, _, reproduced = fixture(identity["seed"], identity["kind"])
        if reproduced != identity:
            raise RuntimeError("Generated source identity changed")
        input_sha = hashlib.sha256(
            np.asarray(source, dtype="<f4").tobytes()
        ).hexdigest()
        if any(
            files["input"]["sha256"] != input_sha
            for files in [row["selected_files"], row["native_files"]]
        ):
            raise RuntimeError(
                "Quantized native input differs from the identified source"
            )
        if row["parity"]["feature_gate_pass"]:
            continue
        spectra = analyze(np.asarray(source, dtype="<f4"))
        reference, _, valid = evidence(spectra)
        exported = json.loads(
            Path(row["selected_files"]["features_output"]["path"]).read_text()
        )
        count = len(exported["rows"])
        reference, valid = reference[:count], valid[:count]
        native = np.asarray(
            [
                [f if f is not None else [0.0] * 6 for f in r["state"]["features"]]
                for r in exported["rows"]
            ]
        )
        native_valid = np.asarray(
            [[f is not None for f in r["state"]["features"]] for r in exported["rows"]]
        )
        shared = valid & native_valid
        error = np.max(abs(native - reference), axis=2)
        failed = shared & (error > report["feature_tolerance"])
        power = np.stack(
            [
                np.sum(
                    abs(spectra[:count, start : min(start + 8, 256)]) ** 2, axis=(1, 2)
                )
                for start in range(1, 256, 8)
            ],
            axis=1,
        )
        frame, band = np.unravel_index(
            np.argmax(np.where(shared, error, -1)), error.shape
        )
        native_prob, reference_prob = (
            predict(theta, native[shared]),
            predict(theta, reference[shared]),
        )
        diagnoses.append(
            {
                "identity": identity,
                "layout": row["layout"],
                "failed_band_frames": int(failed.sum()),
                "valid_band_frames": int(shared.sum()),
                "failed_reference_energy_fraction": float(
                    power[failed].sum() / max(power.sum(), 1e-30)
                ),
                "reference_power_weighted_feature_error": float(
                    np.sum(error[shared] * power[shared])
                    / max(power[shared].sum(), 1e-30)
                ),
                "max_probability_difference_from_fft_features": float(
                    np.max(abs(native_prob - reference_prob))
                ),
                "worst": {
                    "analysis_frame": int(frame),
                    "band": int(band),
                    "reference_band_power": float(power[frame, band]),
                    "reference_total_frame_power": float(power[frame].sum()),
                    "native_features": native[frame, band].tolist(),
                    "reference_features": reference[frame, band].tolist(),
                },
            }
        )
    groups = {}
    for layout in ["5.1", "7.1"]:
        for kind in sorted({r["identity"]["kind"] for r in report["cases"]}):
            rows = [
                r
                for r in report["cases"]
                if r["layout"] == layout
                and r["identity"]["kind"] == kind
                and r["split"] not in {"training", "validation"}
            ]
            if not rows:
                continue
            groups[layout + ":" + kind] = {
                "cases": len(rows),
                "feature_gate_failures": sum(
                    not r["parity"]["feature_gate_pass"] for r in rows
                ),
                "mean_relative_rms_output_change": float(
                    np.mean([r["relative_rms_output_change"] for r in rows])
                ),
                "mean_front_energy_change_db": float(
                    np.mean(
                        [
                            10
                            * np.log10(
                                max(r["selected"]["group_energy"]["front"], 1e-30)
                                / max(r["native"]["group_energy"]["front"], 1e-30)
                            )
                            for r in rows
                        ]
                    )
                ),
                "maximum_selected_peak": max(r["selected"]["peak"] for r in rows),
            }
    result = {
        "model_sha256": report["model_sha256"],
        "renderer_sha256": report["renderer_sha256"],
        "raw_hashes_verified": raw_hashes_verified,
        "cases": len(report["cases"]),
        "compared_band_frames": sum(
            r["parity"]["compared_band_frames"] for r in report["cases"]
        ),
        "validity_disagreements": sum(
            r["parity"]["validity_disagreements"] for r in report["cases"]
        ),
        "feature_gate_failures": dict(
            Counter(
                r["identity"]["kind"]
                for r in report["cases"]
                if not r["parity"]["feature_gate_pass"]
            )
        ),
        "maximum_network_error_on_native_features": max(
            r["parity"]["maximum_network_error_on_native_features"]
            for r in report["cases"]
        ),
        "groups": groups,
        "feature_failure_diagnoses": diagnoses,
        "deployment_admitted": False,
    }
    args.output.write_text(json.dumps(result, indent=2, allow_nan=False) + "\n")
    print(
        json.dumps(
            {
                "cases": result["cases"],
                "feature_gate_failures": result["feature_gate_failures"],
                "hashes_verified": raw_hashes_verified,
            }
        )
    )


if __name__ == "__main__":
    main()
