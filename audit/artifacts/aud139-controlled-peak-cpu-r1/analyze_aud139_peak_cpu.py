#!/usr/bin/env python3
"""Validate and summarize the four matched AUD139 Peak timing logs."""

import argparse
import itertools
import json
import re
import statistics
from pathlib import Path


def analyze(directory: Path) -> dict:
    expected = set(itertools.product((2, 8), (4, 8), (64, 256, 1024)))
    names = ("run-1-old.log", "run-2-new.log", "run-3-new.log", "run-4-old.log")
    runs = []
    receipt = json.loads((directory / "timing-execution-receipt.json").read_text())
    assert receipt["order"] == ["old", "new", "new", "old"]
    assert len(receipt["runs"]) == 4
    assert all(run["exit_code"] == 0 for run in receipt["runs"])
    for name in names:
        data = {}
        for line in (directory / name).read_text().splitlines():
            if "AUD139_CPU " not in line:
                continue
            # libtest prefixes the first printed row with its test name.
            line = line.split("AUD139_CPU ", 1)[1]
            fields = dict(re.findall(r"(\w+)=(\[[^\]]*\]|[^ ]+)", line))
            key = tuple(int(fields[k]) for k in ("channels", "bands", "callback_frames"))
            trials = json.loads(fields["trial_ns"])
            assert key not in data and len(trials) == 7
            assert all(isinstance(value, int) and value > 0 for value in trials)
            assert statistics.median(trials) == int(fields["median_ns"])
            assert int(fields["sample_rate"]) == 48000
            assert int(fields["frames_per_trial"]) == 65536
            assert int(fields["warmups"]) == 2 and fields["profile"] == "release"
            data[key] = trials
        assert set(data) == expected, (name, set(data))
        runs.append(data)
    cases = []
    for channels, bands, callback_frames in sorted(expected):
        key = (channels, bands, callback_frames)
        old = runs[0][key] + runs[3][key]
        new = runs[1][key] + runs[2][key]
        old_median, new_median = statistics.median(old), statistics.median(new)
        ratio = new_median / old_median
        cases.append({
            "channels": channels, "bands": bands, "callback_frames": callback_frames,
            "old_trial_ns": old, "new_trial_ns": new,
            "old_median_ns": old_median, "new_median_ns": new_median,
            "old_ns_per_frame": old_median / 65536,
            "new_ns_per_frame": new_median / 65536,
            "new_to_old_ratio": ratio, "budget_ratio": 1.10,
            "within_budget": ratio <= 1.10,
            "paired_run_ratios": [
                statistics.median(runs[1][key]) / statistics.median(runs[0][key]),
                statistics.median(runs[2][key]) / statistics.median(runs[3][key]),
            ],
            "old_range_ns": [min(old), max(old)],
            "new_range_ns": [min(new), max(new)],
        })
    return {
        "scope": "Peak default processing only; old/new DynamicEQ source with byte-identical recovered harness and matched current dependencies/compiler/profile. Excludes shelves, native parameter synchronization, setup/reset, and WCET.",
        "sample_rate": 48000, "frames_per_trial": 65536,
        "measured_trials_per_variant_per_case": 14,
        "aggregation": "Median of concatenated seven-trial runs for each variant (14 measurements); paired run medians also retained.",
        "case_count": len(cases),
        "min_ratio": min(case["new_to_old_ratio"] for case in cases),
        "max_ratio": max(case["new_to_old_ratio"] for case in cases),
        "over_budget_cases": [
            {key: case[key] for key in ("channels", "bands", "callback_frames", "new_to_old_ratio")}
            for case in cases if not case["within_budget"]
        ],
        "cases": cases,
    }


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path)
    args = parser.parse_args()
    result = analyze(args.directory)
    (args.directory / "peak-comparison.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps({key: result[key] for key in ("case_count", "min_ratio", "max_ratio", "over_budget_cases")}, indent=2))
