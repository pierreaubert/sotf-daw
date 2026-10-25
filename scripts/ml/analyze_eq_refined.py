#!/usr/bin/env python3
"""Independently check frozen supervision, checkpoint selection and exports."""

import argparse
import json
from pathlib import Path

import eq_multiband as eq
import numpy as np
import torch
from eq_model_loading import load_model
from evaluate_eq_refined import independent_response


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--experiment", type=Path, required=True)
    parser.add_argument("--artifacts", type=Path, required=True)
    parser.add_argument("--base", type=Path, required=True)
    args = parser.parse_args()
    torch.set_num_threads(2)
    manifest = json.loads((args.experiment / "data-manifest.json").read_text())
    train = eq.dataset(manifest["arguments"]["examples"], manifest["arguments"]["seed"])
    if train[3] != manifest["split_ids"]["train"]:
        raise ValueError("Training identities changed")
    base, digest = load_model(args.base, eq.CONTRACT, eq.network)
    if digest != manifest["base_weights_sha256"]:
        raise ValueError("Base changed")
    _, initial = eq.evaluate(base, train)
    teacher = json.loads((args.experiment / "teacher-report.json").read_text())
    labels = np.load(args.experiment / "teacher-labels.npz", allow_pickle=False)
    np.testing.assert_array_equal(labels["rates"], train[1])
    np.testing.assert_array_equal(labels["targets"], train[2])
    errors, stable = [], 0
    for index, (p, rate, target, row) in enumerate(
        zip(labels["parameters"], train[1], train[2], teacher)
    ):
        if not np.isfinite(p).all() or np.any(p < eq.LOWER) or np.any(p > eq.UPPER):
            raise ValueError("Invalid supervised parameters")
        reference = independent_response(
            eq.proposal_parameters(p)["filters"], eq.GRID, rate
        )
        mse = float(np.mean((reference - target) ** 2))
        before = float(np.mean((eq.response(initial[index], rate=rate) - target) ** 2))
        if row["improved_label"]:
            if not mse < before + 1e-10 or abs(mse - row["mse_db2"]) > 1e-7:
                raise ValueError(
                    "Teacher label response does not match improvement report"
                )
        else:
            np.testing.assert_array_equal(p, initial[index])
        errors.append(
            abs(mse - float(np.mean((eq.response(p, rate=rate) - target) ** 2)))
        )
        for section in eq.coefficients(p, np.asarray(rate)):
            if not np.all(np.abs(np.roots(section[3:])) < 1):
                raise ValueError("Unstable teacher section")
            stable += 1
    selected = {}
    for arm in ("original-labels", "refined-labels"):
        report = json.loads(
            (args.experiment / arm / "training-report.json").read_text()
        )

        def key(row):
            metrics = row["development"]
            return (
                -metrics["sampled_solver_qualified"],
                metrics["mean_refined_mse_db2"],
                metrics["median_residual_calls"],
            )

        expected = min(report["history"], key=key)
        if (
            expected["epoch"] != report["best_epoch"]
            or expected["development"] != report["best_development"]
        ):
            raise ValueError(
                "Checkpoint selection did not use declared development rule"
            )
        load_model(args.experiment / arm / "weights.json", eq.CONTRACT, eq.network)
        selected[arm] = report["best_epoch"]
    comparison = json.loads(
        (args.artifacts / "final-evaluation/comparison.json").read_text()
    )
    residual_errors, accepted, rejected = [], 0, 0
    for row in comparison["rows"]:
        result = row["result"]
        if not result["accepted"]:
            if result["parameters"] is not None:
                raise ValueError("Rejected fit has controls")
            rejected += 1
            continue
        target_path = (
            args.artifacts / "final-evaluation" / (row["case"] + "-target.json")
        )
        target = json.loads(target_path.read_text())
        if eq.sha(target_path) != result["target_sha256"]:
            raise ValueError("Export target digest mismatch")
        response = independent_response(
            result["parameters"]["filters"],
            np.asarray(target["frequencies_hz"]),
            target["sample_rate"],
        )
        error = response - np.asarray(target["target_db"])
        rms, maximum = float(np.sqrt(np.mean(error**2))), float(np.max(np.abs(error)))
        if rms > 0.100001 or maximum > 0.500001:
            raise ValueError("Independent exported accuracy failed")
        residual_errors.append(abs(rms - row["independent"]["rms_db"]))
        accepted += 1
    report = {
        "teacher_cases": len(teacher),
        "improved_teacher_labels": sum(r["improved_label"] for r in teacher),
        "stable_teacher_sections": stable,
        "max_teacher_mse_discrepancy": max(errors),
        "selected_epochs": selected,
        "accepted_exports_checked": accepted,
        "rejected_exports_checked": rejected,
        "max_export_rms_discrepancy": max(residual_errors, default=0),
        "qualification": "Independent analog/bilinear response check and recorded selection replay; no perceptual or measured-target evidence",
    }
    (args.artifacts / "independent-analysis.json").write_text(
        json.dumps(report, indent=2) + "\n"
    )
    print(json.dumps(report), flush=True)


if __name__ == "__main__":
    main()
