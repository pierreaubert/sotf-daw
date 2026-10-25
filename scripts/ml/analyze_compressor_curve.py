#!/usr/bin/env python3
"""Independent scalar reference, fitted controls and native preset reopen checks."""

import argparse
import json
import subprocess
from pathlib import Path

import numpy as np


def reference(p, x):
    threshold, ratio, knee, makeup = p[0], np.exp(p[1]), p[2], p[3]
    output = []
    for level in x:
        if knee < 0.1:
            value = min(level, threshold) + max(level - threshold, 0) / ratio
        elif level < threshold - knee / 2:
            value = level
        elif level > threshold + knee / 2:
            value = threshold + (level - threshold) / ratio
        else:
            value = level + (1 / ratio - 1) * (level - threshold + knee / 2) ** 2 / (
                2 * knee
            )
        output.append(value + makeup)
    return np.asarray(output)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--artifacts", type=Path, required=True)
    parser.add_argument("--renderer", type=Path, required=True)
    args = parser.parse_args()
    final = args.artifacts / "final"
    holdout = json.loads((final / "fresh-holdout.json").read_text())
    x = np.linspace(-84, 0, 128)
    maximum_source_error, maximum_fit_error = 0.0, 0.0
    for row in holdout["rows"]:
        target = np.asarray(row["target_dbfs"])
        maximum_source_error = max(
            maximum_source_error,
            float(np.max(np.abs(reference(row["source_parameters"], x) - target))),
        )
        for fit in row["fit"]["candidates"]:
            if fit["accepted"]:
                error = reference(fit["parameters"], x) - target
                if (
                    np.max(np.abs(error)) > 0.0300001
                    or np.sqrt(np.mean(error**2)) > 0.0100001
                ):
                    raise ValueError("Independent accepted curve failed")
                maximum_fit_error = max(
                    maximum_fit_error, abs(float(np.max(np.abs(error))) - fit["max_db"])
                )
    reopen = final / "reopen"
    reopen.mkdir(exist_ok=False)
    rows, maximum_native_discrepancy = [], 0.0
    for row in json.loads((final / "native-summary.json").read_text()):
        name = row["case"]
        report = json.loads((final / (name + "-export/report.json")).read_text())
        if not report["accepted"]:
            if (
                report["parameters"] is not None
                or (final / (name + "-export/preset.json")).exists()
            ):
                raise ValueError("Rejected case exported controls")
            continue
        preset = json.loads((final / (name + "-export/preset.json")).read_text())
        target = json.loads((final / (name + "-target.json")).read_text())
        if (
            preset != report["parameters"]
            or preset["crossover_frequencies"] != target["crossovers_hz"]
        ):
            raise ValueError("Exported preset differs from intended layout")
        for source, band in zip(target["bands"], preset["bands"]):
            if (
                band["attack_ms"] != source["attack_ms"]
                or band["release_ms"] != source["release_ms"]
                or band["auto_makeup"]
                or band["measured_auto_makeup"]
            ):
                raise ValueError("Explicit controls changed during fitting")
        request = {
            "sample_rate": target["sample_rate"],
            "callback_frames": 257,
            "settle_multiple": 64.0,
            "params": preset,
            "levels_dbfs": [band["input_dbfs"] for band in target["bands"]],
        }
        path = reopen / (name + ".json")
        path.write_text(json.dumps(request) + "\n")
        completed = subprocess.run(
            [str(args.renderer), str(path)],
            capture_output=True,
            text=True,
            check=True,
            timeout=600,
        )
        native = json.loads(completed.stdout)
        for band, curve, previous, desired in zip(
            preset["bands"],
            native["curves"],
            report["native"]["curves"],
            target["bands"],
        ):
            actual = np.asarray(curve["output_dbfs"])
            ideal = reference(
                [
                    band["threshold_db"],
                    np.log(band["ratio"]),
                    band["knee_db"],
                    band["makeup_gain_db"],
                ],
                desired["input_dbfs"],
            )
            if np.max(np.abs(actual - ideal)) > 0.050001:
                raise ValueError("Independent native/reference comparison failed")
            maximum_native_discrepancy = max(
                maximum_native_discrepancy,
                float(np.max(np.abs(actual - previous["output_dbfs"]))),
            )
        (reopen / (name + "-result.json")).write_text(
            json.dumps(native, indent=2) + "\n"
        )
        rows.append(name)
    if (
        maximum_source_error > 1e-12
        or maximum_fit_error > 1e-12
        or maximum_native_discrepancy > 1e-6
    ):
        raise ValueError("Independent source/fit/settling consistency failed")
    report = {
        "fresh_curves_checked": len(holdout["rows"]),
        "presets_reopened": rows,
        "max_source_curve_error_db": maximum_source_error,
        "max_fit_error_discrepancy_db": maximum_fit_error,
        "max_native_partition_settling_discrepancy_db": maximum_native_discrepancy,
        "qualification": "Static isolated-band validation; no dynamic, hardware or multiband programme equivalence claim",
    }
    (args.artifacts / "independent-analysis.json").write_text(
        json.dumps(report, indent=2) + "\n"
    )
    print(json.dumps(report))


if __name__ == "__main__":
    main()
