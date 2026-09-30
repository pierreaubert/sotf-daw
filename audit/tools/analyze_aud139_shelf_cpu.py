#!/usr/bin/env python3
"""Independently check matched Peak/LowShelf/HighShelf CPU capture logs."""

import argparse
import itertools
import json
import re
import statistics
from pathlib import Path


def analyze(logs: list[Path]) -> dict:
    shapes = ("peak", "low_shelf", "high_shelf")
    geometry = set(itertools.product((2, 8), (4, 8), (64, 256, 1024)))
    expected = {(*case, shape) for case in geometry for shape in shapes}
    captures = []
    for log in logs:
        content = log.read_text()
        assert "test result: ok. 1 passed; 0 failed; 0 ignored;" in content, log
        rows = {}
        fields_by_key = {}
        for line in content.splitlines():
            if "AUD139_CPU_COMPARE " not in line:
                continue
            fields = dict(re.findall(
                r"(\w+)=(\[[^\]]*\]|[^ ]+)",
                line.split("AUD139_CPU_COMPARE ", 1)[1],
            ))
            key = tuple(int(fields[k]) for k in ("channels", "bands", "callback_frames")) + (fields["shape"],)
            assert key not in rows, (log, key)
            trials = json.loads(fields["trial_ns"])
            assert len(trials) == 7 and all(isinstance(x, int) and x > 0 for x in trials)
            assert statistics.median(trials) == int(fields["median_ns"])
            assert fields["profile"] == "release"
            assert int(fields["sample_rate"]) == 48000
            assert int(fields["frames_per_trial"]) == 65536
            assert int(fields["warmups"]) == 2
            assert int(fields["measured_trials"]) == 7
            assert float(fields["shelf_slope"]) == 0.75
            assert float(fields["accepted_shelf_ratio_max"]) == 1.25
            rows[key] = trials
            fields_by_key[key] = fields
        assert set(rows) == expected, (log, set(rows))
        for key, trials in rows.items():
            ratio = statistics.median(trials) / statistics.median(rows[(*key[:3], "peak")])
            assert abs(ratio - float(fields_by_key[key]["ratio_to_peak"])) <= 5.1e-7
        captures.append(rows)
    cases = []
    for case in sorted(geometry):
        samples = {shape: sum((capture[(*case, shape)] for capture in captures), []) for shape in shapes}
        medians = {shape: statistics.median(values) for shape, values in samples.items()}
        for shape in shapes[1:]:
            ratio = medians[shape] / medians["peak"]
            cases.append({
                "channels": case[0], "bands": case[1], "callback_frames": case[2],
                "shape": shape, "shelf_to_peak_ratio": ratio, "budget_ratio": 1.25,
                "within_budget": ratio <= 1.25,
                "peak_trial_ns": samples["peak"], "shelf_trial_ns": samples[shape],
                "peak_median_ns": medians["peak"], "shelf_median_ns": medians[shape],
                "per_run_ratios": [
                    statistics.median(capture[(*case, shape)]) / statistics.median(capture[(*case, "peak")])
                    for capture in captures
                ],
            })
    return {
        "scope": "Same-executable, same-current-source matched shelf-versus-Peak processing timing; excludes setup/reset, native synchronization, accuracy, and WCET.",
        "logs": [str(log) for log in logs], "capture_count": len(captures),
        "geometry_case_count": len(geometry), "shelf_comparison_count": len(cases),
        "measured_trials_per_shape_per_case": 7 * len(captures),
        "shelf_slope": 0.75, "budget_ratio": 1.25,
        "min_ratio": min(case["shelf_to_peak_ratio"] for case in cases),
        "max_ratio": max(case["shelf_to_peak_ratio"] for case in cases),
        "over_budget_cases": [
            {key: case[key] for key in ("channels", "bands", "callback_frames", "shape", "shelf_to_peak_ratio")}
            for case in cases if not case["within_budget"]
        ],
        "cases": cases,
    }


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("logs", nargs="+", type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    result = analyze(args.logs)
    args.output.write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps({key: result[key] for key in ("capture_count", "shelf_comparison_count", "min_ratio", "max_ratio", "over_budget_cases")}, indent=2))
