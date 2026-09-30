#!/usr/bin/env python3
"""Verify and summarize the retained AUD142 Criterion sample sets."""

from __future__ import annotations

import csv
import json
import math
import statistics
from pathlib import Path


ARTIFACTS = Path(__file__).resolve().parent.parent
RUN = Path(__file__).resolve().parent
PREEDIT = ARTIFACTS / "aud142-preedit" / "criterion-r3"
ARCHIVED = RUN / "criterion-r1" / "archived"
CURRENT = RUN / "criterion-r1" / "current"


def case_key(sample_path: Path, root: Path) -> str:
    return "/".join(sample_path.relative_to(root).parts[:-2])


def read_cases(root: Path) -> dict[str, dict[str, object]]:
    cases: dict[str, dict[str, object]] = {}
    for sample_path in sorted(root.rglob("sample.json")):
        key = case_key(sample_path, root)
        sample = json.loads(sample_path.read_text())
        estimate_path = sample_path.parent / "estimates.json"
        estimates = json.loads(estimate_path.read_text())
        iterations = sample["iters"]
        times = sample["times"]
        if len(iterations) != 30 or len(times) != 30:
            raise ValueError(f"{key}: expected 30 samples, got {len(times)}")
        per_iteration_ns = [time / count for time, count in zip(times, iterations)]
        mean = statistics.mean(per_iteration_ns)
        cv_pct = statistics.stdev(per_iteration_ns) / mean * 100 if mean else math.nan
        cases[key] = {
            "sample_path": sample_path,
            "estimate_path": estimate_path,
            "median_ns": estimates["median"]["point_estimate"],
            "median_ci_low_ns": estimates["median"]["confidence_interval"]["lower_bound"],
            "median_ci_high_ns": estimates["median"]["confidence_interval"]["upper_bound"],
            "sample_min_ns": min(per_iteration_ns),
            "sample_max_ns": max(per_iteration_ns),
            "sample_cv_pct": cv_pct,
            "sample_count": len(per_iteration_ns),
        }
    return cases


def write_csv(path: Path, rows: list[dict[str, object]], fields: list[str]) -> None:
    with path.open("w", newline="") as output:
        writer = csv.DictWriter(output, fieldnames=fields)
        writer.writeheader()
        writer.writerows(rows)


old = read_cases(ARCHIVED)
current = read_cases(CURRENT)
preedit = read_cases(PREEDIT)
legacy_keys = sorted(key for key in old if not key.startswith("crossover_new_iir_"))
current_legacy_keys = sorted(key for key in current if not key.startswith("crossover_new_iir_"))
preedit_keys = sorted(preedit)
if legacy_keys != current_legacy_keys or legacy_keys != preedit_keys:
    raise ValueError("archived/current/pre-edit legacy case IDs differ")
if len(legacy_keys) != 31:
    raise ValueError(f"expected 31 legacy cases, got {len(legacy_keys)}")

legacy_rows = []
for key in legacy_keys:
    old_case = old[key]
    current_case = current[key]
    preedit_case = preedit[key]
    legacy_rows.append(
        {
            "case_id": key,
            "sample_count": current_case["sample_count"],
            "archived_median_ns": old_case["median_ns"],
            "archived_median_ci_low_ns": old_case["median_ci_low_ns"],
            "archived_median_ci_high_ns": old_case["median_ci_high_ns"],
            "archived_sample_cv_pct": old_case["sample_cv_pct"],
            "current_median_ns": current_case["median_ns"],
            "current_median_ci_low_ns": current_case["median_ci_low_ns"],
            "current_median_ci_high_ns": current_case["median_ci_high_ns"],
            "current_sample_cv_pct": current_case["sample_cv_pct"],
            "current_over_archived": current_case["median_ns"] / old_case["median_ns"],
            "preedit_r3_median_ns": preedit_case["median_ns"],
            "current_over_preedit_r3": current_case["median_ns"] / preedit_case["median_ns"],
        }
    )
write_csv(
    RUN / "legacy-paired-summary-r1.csv",
    legacy_rows,
    list(legacy_rows[0]),
)

new_keys = sorted(key for key in current if key.startswith("crossover_new_iir_"))
if len(new_keys) != 64:
    raise ValueError(f"expected 64 new-family cases, got {len(new_keys)}")
new_rows = []
for key in new_keys:
    pieces = key.split("/")
    group = pieces[0]
    label = pieces[1]
    tail = label.split("_2ch_", 1)
    family = tail[0]
    topology = tail[1].removesuffix("_both").removesuffix("_mixed")
    case = current[key]
    new_rows.append(
        {
            "case_id": key,
            "measurement": "setup_construct_initialize_drop" if group == "crossover_new_iir_setup" else "process_call",
            "family": family,
            "topology": topology,
            "frames": pieces[2] if len(pieces) > 2 else "",
            "sample_count": case["sample_count"],
            "median_ns": case["median_ns"],
            "median_ci_low_ns": case["median_ci_low_ns"],
            "median_ci_high_ns": case["median_ci_high_ns"],
            "sample_min_ns": case["sample_min_ns"],
            "sample_max_ns": case["sample_max_ns"],
            "sample_cv_pct": case["sample_cv_pct"],
        }
    )
write_csv(RUN / "new-family-summary-r1.csv", new_rows, list(new_rows[0]))

legacy_ratios = [row["current_over_archived"] for row in legacy_rows]
preedit_ratios = [row["current_over_preedit_r3"] for row in legacy_rows]
group_summary: dict[str, dict[str, float]] = {}
for group in ("crossover_setup_and_initialize", "crossover_lr_interleaved_blocks", "crossover_fir_interleaved_blocks"):
    values = [row["current_over_archived"] for row in legacy_rows if row["case_id"].startswith(group + "/")]
    group_summary[group] = {
        "case_count": len(values),
        "ratio_min": min(values),
        "ratio_median": statistics.median(values),
        "ratio_max": max(values),
    }

new_summary: dict[str, dict[str, object]] = {}
for measurement in ("setup_construct_initialize_drop", "process_call"):
    selected = [row for row in new_rows if row["measurement"] == measurement]
    by_key: dict[str, list[float]] = {}
    for row in selected:
        suffix = f"{row['family']}/{row['topology']}"
        if row["frames"]:
            suffix += f"/{row['frames']}"
        by_key.setdefault(suffix, []).append(row["median_ns"])
    new_summary[measurement] = {
        "case_count": len(selected),
        "median_of_case_medians_ns": statistics.median(row["median_ns"] for row in selected),
        "min_case_median_ns": min(row["median_ns"] for row in selected),
        "max_case_median_ns": max(row["median_ns"] for row in selected),
        "max_sample_cv_pct": max(row["sample_cv_pct"] for row in selected),
        "case_medians_ns": {key: values[0] for key, values in sorted(by_key.items())},
    }

summary = {
    "legacy_case_count": len(legacy_rows),
    "new_family_case_count": len(new_rows),
    "sample_count_per_case": 30,
    "matched_current_over_archived_r1_ratio_range": [min(legacy_ratios), max(legacy_ratios)],
        "matched_current_over_archived_r1_ratio_median": statistics.median(legacy_ratios),
        "current_over_preserved_preedit_r3_ratio_range": [min(preedit_ratios), max(preedit_ratios)],
        "current_over_preserved_preedit_r3_ratio_median": statistics.median(preedit_ratios),
        "legacy_archived_r1_max_sample_cv_pct": max(row["archived_sample_cv_pct"] for row in legacy_rows),
        "legacy_current_r1_max_sample_cv_pct": max(row["current_sample_cv_pct"] for row in legacy_rows),
    "legacy_group_ratios": group_summary,
    "new_family_absolute": new_summary,
    "limits": [
        "Throughput samples only; no worst-case execution-time claim.",
        "Setup times include construct, initialize, and drop; process times cover one call on a reused instance.",
        "New-family absolute measurements are not compared to the old binary because it has no new-family cases.",
        "The current run had a changing shared-host load; inspect retained raw samples and CV before interpreting small differences.",
    ],
}
(RUN / "timing-summary-r1.json").write_text(json.dumps(summary, indent=2) + "\n")
print(json.dumps(summary, indent=2))
