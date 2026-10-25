#!/usr/bin/env python3
"""Independent finite-difference likelihood fits and export/abstention checks."""

import argparse
import json
import math
from pathlib import Path

import numpy as np
from scipy.optimize import minimize


def reference_fit(design, labels, weights, ridge):
    weights = weights / weights.sum()

    def loss(theta):
        logits = design @ theta
        return (
            np.dot(weights, np.logaddexp(0, logits) - labels * logits)
            + ridge * np.dot(theta, theta) / 2
        )

    solved = minimize(
        loss,
        np.zeros(design.shape[1]),
        method="BFGS",
        options={"gtol": 1e-8, "maxiter": 500},
    )
    return solved.x, loss(solved.x)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--artifacts", type=Path, required=True)
    args = parser.parse_args()
    root = args.artifacts
    rows = []
    max_utility_error, max_lopo_error, max_bootstrap_error = 0.0, 0.0, 0.0
    for path in sorted((root / "fresh-synthetic").glob("*-session.json")):
        data = json.loads(path.read_text())
        report = json.loads(
            path.with_name(path.name.replace("-session", "-report")).read_text()
        )
        ids = [candidate["id"] for candidate in data["candidates"]]
        groups = [trial["programme_id"] for trial in data["trials"]]
        unique, inverse, counts = np.unique(
            groups, return_inverse=True, return_counts=True
        )
        design = np.zeros((len(groups), len(ids) + 1))
        labels = []
        for index, trial in enumerate(data["trials"]):
            (
                design[index, ids.index(trial["a"])],
                design[index, ids.index(trial["b"])],
                design[index, -1],
            ) = 1, -1, 1
            labels.append({"a": 1.0, "b": 0.0, "tie": 0.5}[trial["choice"]])
        labels = np.asarray(labels)
        weights = 1 / counts[inverse]
        ridge = report["policy"]["ridge"]
        reference, _ = reference_fit(design, labels, weights, ridge)
        stored = np.r_[
            report["model"]["utilities"], report["model"]["presentation_side_bias"]
        ]
        max_utility_error = max(
            max_utility_error, float(np.max(np.abs(reference - stored)))
        )
        loss = []
        for group in range(len(unique)):
            held = inverse == group
            fitted, _ = reference_fit(design, labels, weights * ~held, ridge)
            logits = design[held] @ fitted
            loss.append(float(np.mean(np.logaddexp(0, logits) - labels[held] * logits)))
        max_lopo_error = max(
            max_lopo_error, abs(np.mean(loss) - report["lopo_log_loss"])
        )
        samples = np.asarray(report["bootstrap_utilities"])
        if (
            samples.shape != (report["policy"]["bootstrap_samples"], len(ids))
            or not np.isfinite(samples).all()
        ):
            raise ValueError("Invalid bootstrap samples")
        rng = np.random.default_rng(report["policy"]["bootstrap_seed"])
        for index in range(5):
            draws = np.bincount(
                rng.integers(len(unique), size=len(unique)), minlength=len(unique)
            )
            fitted, _ = reference_fit(design, labels, weights * draws[inverse], ridge)
            max_bootstrap_error = max(
                max_bootstrap_error, float(np.max(np.abs(fitted[:-1] - samples[index])))
            )
        winner = int(np.argmax(stored[:-1]))
        margins = samples[:, winner] - np.max(
            np.delete(samples, winner, axis=1), axis=1
        )
        if (
            abs(float(np.mean(margins > 0)) - report["winner_bootstrap_fraction"])
            > 1e-12
            or abs(float(np.quantile(margins, 0.05)) - report["winner_margin_p05"])
            > 1e-12
        ):
            raise ValueError("Reported bootstrap statistics differ")
        if report["accepted"]:
            if (
                report["reasons"]
                or report["selected_id"] != ids[winner]
                or report["parameters"] != data["candidates"][winner]["parameters"]
            ):
                raise ValueError("Accepted output is not an exact auditioned setting")
        elif report["parameters"] is not None or report["selected_id"] is not None:
            raise ValueError("Abstention exposed controls")
        rows.append({"case": path.stem, "accepted": report["accepted"]})
    functional = []
    for kind in ["consistent", "ties", "heterogeneous", "side-bias"]:
        folder = root / ("functional-final-" + kind)
        report = json.loads((folder / "report.json").read_text())
        if (folder / "preset.json").exists() != report["accepted"]:
            raise ValueError("Preset existence disagrees with acceptance")
        functional.append({"case": kind, "accepted": report["accepted"]})
    if max(max_utility_error, max_bootstrap_error) > 3e-5 or max_lopo_error > 1e-6:
        raise ValueError("Independent likelihood fitting differs")
    native_data = (
        Path(__file__).resolve().parents[2]
        / "crates/sotf-plugins/crates/sotf-plugin-crossfeed/tests/data"
    )
    for name, original in [
        ("ml-preferences-session.json", root / "consistent-session.json"),
        (
            "ml-preferences-preset.json",
            root / "functional-final-consistent/preset.json",
        ),
        (
            "ml-preferences-report.json",
            root / "functional-final-consistent/report.json",
        ),
    ]:
        if (native_data / name).read_bytes() != original.read_bytes():
            raise ValueError("Native test fixture differs from CLI output")
    result = {
        "sessions": len(rows),
        "independent_bootstrap_fits": len(rows) * 5,
        "independent_lopo_fits": len(rows) * 8,
        "max_utility_error": max_utility_error,
        "max_bootstrap_utility_error": max_bootstrap_error,
        "max_lopo_log_loss_error": float(max_lopo_error),
        "uniform_log_loss": math.log(2),
        "functional": functional,
        "qualification": "Numerical model/export correctness only; no listening evidence or confidence calibration",
    }
    (root / "independent-analysis.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result))


if __name__ == "__main__":
    main()
