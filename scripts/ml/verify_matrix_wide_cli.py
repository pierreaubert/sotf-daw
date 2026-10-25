#!/usr/bin/env python3
"""Replay frozen small sessions and verify full-width routing CLI contracts."""

import argparse
import hashlib
import json
import subprocess
import sys
from pathlib import Path

import numpy as np
from matrix_routing import fit


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--prior-evaluation", type=Path, required=True)
    parser.add_argument("--sessions", type=Path, required=True)
    parser.add_argument("--renderer", type=Path, required=True)
    parser.add_argument("--work", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.work.exists() or args.output.exists():
        raise ValueError("Use new verification paths")
    prior = json.loads(args.prior_evaluation.read_text())
    replay = []
    max_metric = 0.0
    for row in prior["rows"]:
        path = args.prior_evaluation.parent / (row["name"] + "-examples.json")
        if hashlib.sha256(path.read_bytes()).hexdigest() != row["examples_sha256"]:
            raise ValueError("Prior example changed")
        current = fit(json.loads(path.read_text()))
        original = row["result"]
        if (
            current["accepted"] != original["accepted"]
            or current["parameters"] != original["parameters"]
        ):
            raise ValueError("Prior admission/parameters changed: " + row["name"])
        if "matrix" in original:
            if current["selected_lambda"] != original["selected_lambda"]:
                raise ValueError("Prior selected penalty changed")
            max_metric = max(
                max_metric,
                abs(current["nested_rms"] - original["nested_rms"]),
                float(
                    np.max(
                        np.abs(
                            np.array(current["regularization_cv_mse"])
                            - original["regularization_cv_mse"]
                        )
                    )
                ),
            )
        replay.append(
            {
                "name": row["name"],
                "admission_unchanged": True,
                "parameters_unchanged": True,
            }
        )
    args.work.mkdir(parents=True)
    invalid = json.loads((args.sessions / "wide-128x128-examples.json").read_text())
    invalid["input_labels"].append("unsupported-129")
    invalid_path = args.work / "invalid-129.json"
    invalid_path.write_text(json.dumps(invalid) + "\n")
    checks = []
    for name, examples, expected, exported in [
        ("wide", args.sessions / "wide-128x128-examples.json", 0, True),
        ("bad-output", args.sessions / "one-bad-output-examples.json", 2, False),
        ("invalid-129", invalid_path, 1, False),
    ]:
        output = args.work / (name + "-output")
        command = [
            sys.executable,
            str(Path(__file__).with_name("fit_matrix_routing.py")),
            "--examples",
            str(examples),
            "--renderer",
            str(args.renderer),
            "--work",
            str(args.work / (name + "-native")),
            "--output",
            str(output),
        ]
        result = subprocess.run(
            command, check=False, capture_output=True, text=True, timeout=300
        )
        if (
            result.returncode != expected
            or (output / "preset.json").exists() != exported
        ):
            raise ValueError("Unexpected wide CLI result: " + name + result.stderr)
        report = (
            json.loads((output / "report.json").read_text())
            if output.exists()
            else None
        )
        if exported:
            preset = json.loads((output / "preset.json").read_text())
            if (
                len(preset["matrix"]) != 128 * 128
                or preset != report["parameters"]
                or len(report["native"]["nested_output_rms"]) != 128
            ):
                raise ValueError("Full-width export lost routing/output validation")
        elif report is not None and (
            report["accepted"] or report["parameters"] is not None
        ):
            raise ValueError("Unqualified wide export")
        checks.append(
            {
                "name": name,
                "exit_code": result.returncode,
                "exported": exported,
                "stdout_sha256": hashlib.sha256(result.stdout.encode()).hexdigest(),
                "stderr": result.stderr,
            }
        )
        if exported:
            before = {
                p.name: hashlib.sha256(p.read_bytes()).hexdigest()
                for p in output.iterdir()
            }
            repeated = subprocess.run(
                command, check=False, capture_output=True, text=True, timeout=30
            )
            after = {
                p.name: hashlib.sha256(p.read_bytes()).hexdigest()
                for p in output.iterdir()
            }
            if repeated.returncode == 0 or before != after:
                raise ValueError("Full-width outputs overwritten")
            checks.append(
                {
                    "name": "overwrite-rejected",
                    "exit_code": repeated.returncode,
                    "hashes_unchanged": True,
                }
            )
    if max_metric > 1e-14:
        raise ValueError("Prior numeric metrics changed materially")
    args.output.write_text(
        json.dumps(
            {
                "prior_replay": replay,
                "max_prior_numeric_metric_difference": max_metric,
                "cli_checks": checks,
            },
            indent=2,
        )
        + "\n"
    )
    print(
        json.dumps(
            {
                "prior_admissions_and_parameters_unchanged": len(replay),
                "max_metric_difference": max_metric,
                "cli_checks": len(checks),
            }
        )
    )


if __name__ == "__main__":
    main()
