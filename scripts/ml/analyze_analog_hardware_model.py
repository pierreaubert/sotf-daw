"""Independent scalar neural forward pass, classical fit and held-source metrics."""

import argparse
import hashlib
import json
import math
from pathlib import Path

import numpy as np


def reference(model, x):
    taps, hidden = model["architecture"]["taps"], model["architecture"]["hidden"]
    coefficients = model["hammerstein"]["weights"]
    neural, baseline = [], []
    for position in range(len(x)):
        u = [
            float(x[position - k]) / model["input_scale"] if position >= k else 0.0
            for k in range(taps)
        ]
        h = [
            math.tanh(
                math.fsum(u[k] * model["hidden_weights"][k][j] for k in range(taps))
                + model["hidden_bias"][j]
            )
            for j in range(hidden)
        ]
        neural.append(
            math.fsum(
                [
                    *(u[k] * model["linear"][k] for k in range(taps)),
                    *(h[j] * model["output_weights"][j] for j in range(hidden)),
                    model["output_bias"],
                ]
            )
        )
        features = [u[k] ** degree for degree in range(1, 6) for k in range(taps)] + [
            1.0
        ]
        baseline.append(
            math.fsum(a * b for a, b in zip(features, coefficients, strict=True))
        )
    return np.array(neural), np.array(baseline)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--evaluation", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.output.exists():
        raise ValueError("Refuse to overwrite analysis")
    cases = json.loads(args.evaluation.read_text())["cases"]
    maximum_metric = maximum_baseline = 0.0
    records_verified = 0
    for case in cases:
        raw = Path(case["candidate"]).read_bytes()
        model = json.loads(raw)
        data = json.loads(Path(case["manifest"]).read_text())
        report = json.loads(Path(case["report"]).read_text())
        assert hashlib.sha256(raw).hexdigest() == report["candidate_sha256"]
        assert (
            model["context"] == data["context"]
            and model["deployment_admitted"] is False
        )
        taps = model["architecture"]["taps"]
        train = [r for r in data["captures"] if r["split"] == "train"]
        quota = report["training"]["training_samples"] // len(train)
        features, targets, weights = [], [], []
        groups = {r["source_group"] for r in train}
        for r in train:
            x = np.fromfile(r["dry_file"], dtype="<f4").astype(float)
            y = np.fromfile(r["wet_file"], dtype="<f4").astype(float)
            indices = np.linspace(taps - 1, len(x) - 1, quota, dtype=int)
            u = np.array([x[indices - k] / model["input_scale"] for k in range(taps)]).T
            f = np.column_stack(
                [u[:, k] ** degree for degree in range(1, 6) for k in range(taps)]
                + [np.ones(len(indices))]
            )
            features.append(f)
            targets.append(y[indices])
            count = sum(a["source_group"] == r["source_group"] for a in train)
            weights.append(np.full(quota, 1 / (len(groups) * count * quota)))
        f, y, w = (
            np.concatenate(features),
            np.concatenate(targets),
            np.concatenate(weights),
        )
        # Independent regularized normal-equation solve versus training SVD.
        rhs = f.T @ (w * y)
        theta = np.linalg.solve(
            f.T @ (w[:, None] * f)
            + model["policy"]["hammerstein_ridge"] * np.eye(f.shape[1]),
            rhs,
        )
        maximum_baseline = max(
            maximum_baseline,
            float(np.max(abs(theta - model["hammerstein"]["weights"]))),
        )
        rows = {
            r["id"]: r for split in report["held_splits"] for r in split["captures"]
        }
        for r in data["captures"]:
            if r["split"] == "train":
                continue
            for name in ["dry", "wet"]:
                assert (
                    hashlib.sha256(Path(r[name + "_file"]).read_bytes()).hexdigest()
                    == r[name + "_sha256"]
                )
            x = np.fromfile(r["dry_file"], dtype="<f4")
            target = np.fromfile(r["wet_file"], dtype="<f4").astype(float)
            neural, baseline = reference(model, x)
            energy = math.fsum(float(v) * float(v) for v in target) / len(target)
            errors = [
                10
                * math.log10(
                    max(
                        math.fsum(float(v) * float(v) for v in p - target)
                        / len(target)
                        / energy,
                        1e-30,
                    )
                )
                for p in [neural, baseline]
            ]
            row = rows[r["id"]]
            maximum_metric = max(
                maximum_metric,
                abs(errors[0] - row["neural_nmse_db"]),
                abs(errors[1] - row["hammerstein_nmse_db"]),
                abs(float(np.max(abs(neural))) - row["sample_peak"]),
                abs(float(np.mean(neural - target)) - row["dc_error"]),
            )
            records_verified += 1
    assert maximum_metric < 1e-7 and maximum_baseline < 1e-8
    result = {
        "cases": len(cases),
        "held_records_verified": records_verified,
        "independent_hammerstein_fits": len(cases),
        "max_metric_error": maximum_metric,
        "max_baseline_coefficient_error": maximum_baseline,
        "scope": "Numerical reconstruction on generated targets only; no hardware fidelity or production admission.",
    }
    args.output.write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
