"""Learn horizontal transfer functions from disjoint measured KEMAR directions."""

import argparse
import hashlib
import json
from pathlib import Path

import h5py
import numpy as np
from xtc_transfer_model import (
    bounded_inverse,
    cancellation_metrics,
    fit,
    load,
    plant,
    predict,
    save,
)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--sofa", required=True, type=Path)
    parser.add_argument("--manifest", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    provenance = json.loads(args.manifest.read_text())
    raw_sha = hashlib.sha256(args.sofa.read_bytes()).hexdigest()
    if raw_sha != provenance["sha256"]:
        raise ValueError("Measured SOFA differs from attributed manifest")
    with h5py.File(args.sofa) as f:
        if (
            f["SourcePosition"].attrs.get("Type") != b"spherical"
            or f["SourcePosition"].attrs.get("Units") != b"degree, degree, metre"
        ):
            raise ValueError("Expected SOFA spherical positions in degrees/metres")
        if (
            f["Data.IR"].shape != (710, 2, 512)
            or not np.array_equal(f["Data.SamplingRate"][()], [44100.0])
            or np.any(f["Data.Delay"][()] != 0)
        ):
            raise ValueError("Unsupported measured geometry, clock or separate delays")
        position, ir = f["SourcePosition"][()], f["Data.IR"][()]
    horizontal = np.flatnonzero(
        (abs(position[:, 1]) < 1e-8) & (abs(position[:, 2] - 1.4) < 1e-8)
    )
    order = np.argsort(position[horizontal, 0])
    indices = horizontal[order]
    angles, responses = position[indices, 0], ir[indices]
    if not np.allclose(angles, np.arange(0, 360, 5), atol=1e-8):
        raise ValueError("Expected72 unique measured horizontal directions")
    training = np.arange(0, 72, 4)
    validation = np.arange(1, 72, 4)
    final = np.sort(np.r_[np.arange(2, 72, 4), np.arange(3, 72, 4)])
    candidates = []
    best = None
    for length in [10.0, 20.0, 40.0]:
        for ridge in [1e-6, 0.001, 0.1]:
            model = fit(angles[training], responses[training], length, ridge)
            estimate = predict(model, angles[validation])
            mse = float(
                np.sum((estimate - responses[validation]) ** 2)
                / np.sum(responses[validation] ** 2)
            )
            candidates.append(
                {
                    "length_deg": length,
                    "ridge": ridge,
                    "validation_relative_squared_error": mse,
                }
            )
            if best is None or mse < best[0]:
                best = (mse, model)
    args.output.mkdir(parents=True, exist_ok=False)
    attribution = {
        "dataset_sha256": raw_sha,
        "source": provenance["source"],
        "authors": provenance["authors"],
        "copyright": provenance["copyright"],
        "terms": provenance["terms"],
        "terms_source": provenance["attribution_terms_source"],
        "scope": "single measured KEMAR mannequin; horizontal direction splits, not independent people",
        "training_direction_indices": indices[training].tolist(),
    }
    sha = save(best[1], args.output / "model.json", attribution)
    model, _ = load(args.output / "model.json", sha)
    # The chosen model is serialized and frozen before final measurements are evaluated.
    estimate = predict(model, angles[final])
    direction_errors = [
        {
            "source_index": int(indices[i]),
            "azimuth_deg": float(angles[i]),
            "relative_squared_hrir_error": float(
                np.sum((pred - responses[i]) ** 2) / np.sum(responses[i] ** 2)
            ),
        }
        for i, pred in zip(final, estimate, strict=True)
    ]
    nearest = lambda a: responses[
        training[np.argmin(abs(((angles[training] - a + 180) % 360) - 180))]
    ]
    bins = [5, 12, 23, 46, 93, 186, 372]
    cases = []
    final_set = set(final.tolist())
    for yaw in np.arange(-180, 180, 5):
        azimuths = np.array([(30 + yaw) % 360, (-30 + yaw) % 360], float)
        pair = (azimuths / 5).astype(int)
        if not all(i in final_set for i in pair):
            continue
        actual = plant(*responses[pair])
        learned = plant(*predict(model, azimuths))
        closest = plant(*(nearest(a) for a in azimuths))
        methods = {}
        for name, transfer in [
            ("learned", learned),
            ("nearest_training", closest),
            ("measured_oracle", actual),
        ]:
            inverse = bounded_inverse(transfer)
            maximum = float(np.max(np.linalg.svd(inverse, compute_uv=False)))
            if maximum > 4.0 + 1e-12:
                raise RuntimeError("Inverse operator gain bound exceeded")
            methods[name] = {
                "maximum_inverse_operator_gain": maximum,
                "relative_complex_transfer_error": float(
                    np.linalg.norm(transfer - actual) / np.linalg.norm(actual)
                ),
                "metrics": cancellation_metrics(actual, inverse, bins),
            }
        cases.append(
            {
                "yaw_deg": int(yaw),
                "speaker_azimuths_deg": azimuths.tolist(),
                "measured_source_indices": indices[pair].tolist(),
                "methods": methods,
            }
        )
    # One concrete off-thread filter proposal for a held-out centered pose.
    proposal_plant = plant(*predict(model, [30.0, 330.0]))
    proposal_inverse = bounded_inverse(proposal_plant)
    proposal = {
        "model_sha256": sha,
        "sample_rate": 44100,
        "fft_size": 2048,
        "speaker_azimuths_deg": [30.0, 330.0],
        "model_lookahead_samples": 0,
        "filters_real_imag": np.stack(
            [proposal_inverse.real, proposal_inverse.imag], axis=-1
        ).tolist(),
        "deployment_admitted": False,
        "native_imported": False,
        "scope": "frequency-domain off-thread proposal; no causal renderer or true-peak claim",
    }
    (args.output / "proposal.json").write_text(
        json.dumps(proposal, indent=2, allow_nan=False) + "\n"
    )
    report = {
        "dataset_sha256": raw_sha,
        "model_sha256": sha,
        "training_source_indices": indices[training].tolist(),
        "validation_source_indices": indices[validation].tolist(),
        "final_source_indices": indices[final].tolist(),
        "validation_candidates": candidates,
        "final_direction_errors": direction_errors,
        "final_pose_cases": cases,
        "deployment_admitted": False,
        "scope": "measured single-head horizontal interpolation; supplied-matrix cancellation, not rendered/native or human acceptance",
    }
    (args.output / "report.json").write_text(
        json.dumps(report, indent=2, allow_nan=False) + "\n"
    )
    print(
        json.dumps(
            {
                "model_sha256": sha,
                "held_directions": len(final),
                "held_pose_pairs": len(cases),
                "deployment_admitted": False,
            }
        )
    )


if __name__ == "__main__":
    main()
