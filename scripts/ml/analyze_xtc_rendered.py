"""Verify native render evidence and retain signed measured mismatch outcomes."""

import argparse
import hashlib
import json
from pathlib import Path

import numpy as np


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--report", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    report = json.loads(args.report.read_text())
    hashes = 0
    for row in report["waveforms"]:
        for info in row["files"].values():
            if (
                hashlib.sha256(Path(info["path"]).read_bytes()).hexdigest()
                != info["sha256"]
            ):
                raise ValueError("Native rendered evidence changed")
            hashes += 1
    groups = {}
    for method in ["learned", "native"]:
        for yaw in [0, 5, -5, 10, -10, 20, -20, 90]:
            rows = [
                r
                for r in report["measured_ear_projections"]
                if r["method"] == method and r["yaw_deg"] == yaw
            ]
            summary = {}
            for metric in [
                "normalized_separation_db",
                "intended_gain_db",
                "raw_crosstalk_attenuation_db",
            ]:
                finite = [
                    r[metric]["value"] for r in rows if r[metric]["kind"] == "finite"
                ]
                summary[metric] = {
                    "minimum": min(finite),
                    "median": float(np.median(finite)),
                    "maximum": max(finite),
                    "negative_rows": sum(v < 0 for v in finite),
                    "nonfinite_rows": len(rows) - len(finite),
                }
            groups[f"{method}:yaw={yaw}"] = summary
    result = {
        "model_sha256": report["model_sha256"],
        "proposal_sha256": report["proposal_sha256"],
        "renderer_sha256": report["renderer_sha256"],
        "dataset_sha256": report["dataset_sha256"],
        "native_waveforms": len(report["waveforms"]),
        "native_file_hashes_verified": hashes,
        "measured_ear_projections": len(report["measured_ear_projections"]),
        "maximum_wola_sample_error": max(
            r["maximum_reference_sample_error"] for r in report["waveforms"]
        ),
        "maximum_hann_transfer_error": max(
            r["hann_prediction_max_error"] for r in report["tones"]
        ),
        "maximum_raw_bin_transfer_error": max(
            r["raw_bin_prediction_max_error"] for r in report["tones"]
        ),
        "maximum_measured_matrix_response_error": max(
            r["measured_matrix_response_max_error"]
            for r in report["measured_ear_projections"]
        ),
        "groups": groups,
        "deployment_admitted": False,
    }
    args.output.write_text(json.dumps(result, indent=2, allow_nan=False) + "\n")
    print(
        json.dumps(
            {
                "native_hashes_verified": hashes,
                "maximum_wola_sample_error": result["maximum_wola_sample_error"],
                "deployment_admitted": False,
            }
        )
    )


if __name__ == "__main__":
    main()
