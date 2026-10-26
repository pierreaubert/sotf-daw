"""Independent measured-plant checks and signed XTC proposal summaries."""

import argparse
import hashlib
import json
from pathlib import Path

import h5py
import numpy as np
from xtc_transfer_model import load, predict


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--report", required=True, type=Path)
    parser.add_argument("--proposal", required=True, type=Path)
    parser.add_argument("--model", required=True, type=Path)
    parser.add_argument("--sofa", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    r = json.loads(args.report.read_text())
    proposal = json.loads(args.proposal.read_text())
    if hashlib.sha256(args.sofa.read_bytes()).hexdigest() != r["dataset_sha256"]:
        raise ValueError("Measured source identity changed")
    model, provenance = load(args.model, r["model_sha256"])
    if (
        provenance["dataset_sha256"] != r["dataset_sha256"]
        or proposal["model_sha256"] != r["model_sha256"]
    ):
        raise ValueError("Model/proposal measured-source binding mismatch")
    sets = [
        set(r[k])
        for k in [
            "training_source_indices",
            "validation_source_indices",
            "final_source_indices",
        ]
    ]
    if any(a & b for i, a in enumerate(sets) for b in sets[i + 1 :]):
        raise ValueError("Measured direction partitions overlap")
    with h5py.File(args.sofa) as f:
        ir = f["Data.IR"][()]
        position = f["SourcePosition"][()]
    for case in r["final_pose_cases"]:
        if not set(case["measured_source_indices"]).issubset(sets[2]):
            raise ValueError("Final XTC plant reuses fitting/validation directions")
        for index, angle in zip(
            case["measured_source_indices"], case["speaker_azimuths_deg"], strict=True
        ):
            if abs(position[index, 0] - angle) > 1e-8:
                raise ValueError("Measured speaker position misidentified")
    # Verify the centered proposal with direct complex normal-equation arithmetic,
    # independent of the fitter's SVD inverse construction.
    predicted = predict(model, np.array([30.0, 330.0]))
    h = np.stack(
        [np.fft.rfft(predicted[s], n=2048, axis=1).T for s in range(2)], axis=-1
    )
    wdata = np.asarray(proposal["filters_real_imag"], float)
    w = wdata[..., 0] + 1j * wdata[..., 1]
    reference = np.empty_like(w)
    for b, matrix in enumerate(h):
        beta = 0.001 * np.sum(abs(matrix) ** 2) / 2
        raw = np.linalg.solve(
            matrix.conj().T @ matrix + beta * np.eye(2), matrix.conj().T
        )
        raw *= min(
            1.0, 4 / max(np.sqrt(np.max(np.linalg.eigvalsh(raw.conj().T @ raw))), 1e-30)
        )
        reference[b] = raw
    if np.max(abs(w - reference)) > 1e-10:
        raise ValueError(
            "Proposal disagrees with independent bounded normal-equation solve"
        )
    maximum_gain = float(
        np.sqrt(np.max(np.linalg.eigvalsh(w.conj().transpose(0, 2, 1) @ w)))
    )
    if maximum_gain > 4 + 1e-12:
        raise ValueError("Proposal exceeds inverse gain budget")
    centered = next(c for c in r["final_pose_cases"] if c["yaw_deg"] == 0)
    metric_error = 0.0
    for metric in centered["methods"]["learned"]["metrics"]:
        b, cue = metric["bin"], metric["cue"]
        wave = np.exp(-2j * np.pi * b * np.arange(512) / 2048)
        actual = np.array(
            [
                [
                    np.dot(ir[index, ear], wave)
                    for index in centered["measured_source_indices"]
                ]
                for ear in range(2)
            ]
        )
        response = np.array(
            [
                [
                    sum(
                        actual[ear, speaker] * w[b, speaker, column]
                        for speaker in range(2)
                    )
                    for column in range(2)
                ]
                for ear in range(2)
            ]
        )
        desired, unwanted = abs(response[cue, cue]), abs(response[1 - cue, cue])
        independently_measured = {
            "intended_gain_db": 20 * np.log10(desired),
            "normalized_separation_db": 20 * (np.log10(desired) - np.log10(unwanted)),
            "raw_crosstalk_attenuation_db": 20
            * (np.log10(abs(actual[1 - cue, cue])) - np.log10(unwanted)),
        }
        for name, value in independently_measured.items():
            if metric[name]["kind"] != "finite":
                raise ValueError("Unexpected nonfinite measured centered metric")
            metric_error = max(metric_error, abs(value - metric[name]["value"]))
    if metric_error > 1e-9:
        raise ValueError(
            "Reported centered cancellation differs from direct measured-IR DFT"
        )
    summaries = {}
    for method in ["learned", "nearest_training", "measured_oracle"]:
        rows = [
            m for c in r["final_pose_cases"] for m in c["methods"][method]["metrics"]
        ]
        summaries[method] = {}
        for name in [
            "normalized_separation_db",
            "intended_gain_db",
            "raw_crosstalk_attenuation_db",
        ]:
            values = [m[name]["value"] for m in rows if m[name]["kind"] == "finite"]
            summaries[method][name] = {
                "finite_rows": len(values),
                "minimum": min(values),
                "median": float(np.median(values)),
                "maximum": max(values),
                "negative_rows": sum(v < 0 for v in values),
                "nonfinite_rows": len(rows) - len(values),
            }
    result = {
        "model_sha256": r["model_sha256"],
        "dataset_sha256": r["dataset_sha256"],
        "direction_partition_sizes": [len(s) for s in sets],
        "final_pairs_verified_against_measured_positions": len(r["final_pose_cases"]),
        "proposal_maximum_operator_gain": maximum_gain,
        "proposal_independent_inverse_max_error": float(np.max(abs(w - reference))),
        "centered_measured_dft_metric_max_error_db": float(metric_error),
        "final_mean_relative_squared_hrir_error": float(
            np.mean(
                [x["relative_squared_hrir_error"] for x in r["final_direction_errors"]]
            )
        ),
        "methods": summaries,
        "deployment_admitted": False,
    }
    args.output.write_text(json.dumps(result, indent=2, allow_nan=False) + "\n")
    print(
        json.dumps(
            {
                "partitions": result["direction_partition_sizes"],
                "verified_measured_pairs": len(r["final_pose_cases"]),
                "proposal_maximum_gain": maximum_gain,
            }
        )
    )


if __name__ == "__main__":
    main()
