#!/usr/bin/env python3
"""Independent dense posterior solves, confidence statistics and native reopens."""

import argparse
import json
from pathlib import Path

import numpy as np
from loudness_preference_native import qualify


def posterior(cells, policy, query, reference, multiplicities=None):
    x = np.asarray([c["level"] for c in cells])
    y = np.asarray([c["gains"] for c in cells])
    groups, index, counts = np.unique(
        [c["programme"] for c in cells], return_inverse=True, return_counts=True
    )
    weights = 1 / counts[index]
    if multiplicities is not None:
        weights *= multiplicities[index]
    active = weights > 0
    x, y, weights = x[active], y[active], weights[active]
    noise = np.asarray([c["noise"] for c in cells])[active]
    all_x = np.r_[x, query, reference]
    base = policy["prior_std_db"] ** 2 * np.exp(
        -((all_x[:, None] - all_x[None, :]) ** 2) / (2 * policy["lengthscale_db"] ** 2)
    )
    covariance = base[:-1, :-1] - np.outer(base[:-1, -1], base[-1, :-1]) / base[-1, -1]
    matrix = covariance[:-1, :-1] + np.diag(noise**2 / weights)
    cross = covariance[-1, :-1]
    mean = cross @ np.linalg.solve(matrix, y)
    variance = max(
        0.0, float(covariance[-1, -1] - cross @ np.linalg.solve(matrix, cross))
    )
    return mean, variance, len(groups)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--artifacts", type=Path, required=True)
    parser.add_argument("--renderer", type=Path, required=True)
    parser.add_argument("--work", type=Path, required=True)
    args = parser.parse_args()
    args.work.mkdir(parents=True, exist_ok=False)
    root = args.artifacts / "fresh-synthetic"
    max_mean, max_variance, max_bootstrap = 0.0, 0.0, 0.0
    checked, bootstrap_count = 0, 0
    for path in sorted(root.glob("*-report.json")):
        result = json.loads(path.read_text())
        if "model" not in result:
            continue
        policy, cells = result["policy"], result["cells"]
        query, reference = (
            result["requested_level_db_spl"],
            result["reference_level_db_spl"],
        )
        mean, variance, groups = posterior(cells, policy, query, reference)
        max_mean = max(
            max_mean, float(np.max(np.abs(mean - result["predicted_gains_db"])))
        )
        max_variance = max(max_variance, abs(variance - result["latent_variance_db2"]))
        rng = np.random.default_rng(policy["bootstrap_seed"])
        for index in range(5):
            draws = np.bincount(rng.integers(groups, size=groups), minlength=groups)
            predicted, _, _ = posterior(cells, policy, query, reference, draws)
            max_bootstrap = max(
                max_bootstrap,
                float(np.max(np.abs(predicted - result["bootstrap_gains_db"][index]))),
            )
            bootstrap_count += 1
        if not result["accepted"] and result["parameters"] is not None:
            raise ValueError("Abstention exposed controls")
        checked += 1
    native = []
    for name in ["consistent", "reference"]:
        folder = args.artifacts / (name + "-export")
        result = json.loads((folder / "report.json").read_text())
        params = json.loads((folder / "preset.json").read_text())
        if (
            params != result["parameters"]
            or params["mode"] != 0
            or params["auto_calibrated"]
            or params["auto_gain_enabled"]
        ):
            raise ValueError("Export does not preserve the explicit manual profile")
        report = qualify(
            params,
            result["context"]["sample_rate"],
            args.renderer,
            args.work / name,
            257,
        )
        if (
            abs(
                report["native_sampled_peak_db"]
                - result["native"]["native_sampled_peak_db"]
            )
            > 1e-9
        ):
            raise ValueError("Native preset reopen changed response")
        native.append(report)
    if max(max_mean, max_variance, max_bootstrap) > 1e-9:
        raise ValueError("Independent dense GP solve differs from production Cholesky")
    output = {
        "posterior_reports_checked": checked,
        "independent_bootstrap_fits": bootstrap_count,
        "max_mean_error_db": max_mean,
        "max_variance_error_db2": max_variance,
        "max_bootstrap_error_db": max_bootstrap,
        "native_reopens": native,
        "qualification": "Numerical posterior/export checks only; calibration metadata and human preference validity remain unverified",
    }
    (args.artifacts / "independent-analysis.json").write_text(
        json.dumps(output, indent=2) + "\n"
    )
    print(json.dumps({k: v for k, v in output.items() if k != "native_reopens"}))


if __name__ == "__main__":
    main()
