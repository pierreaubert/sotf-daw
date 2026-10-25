"""Independently reduce frozen AAE shift traces without inference or re-fitting."""

from __future__ import annotations

import argparse
import json
import math
import statistics
from collections import defaultdict
from pathlib import Path


def reduce(directory: Path) -> dict:
    protocol = json.loads((directory / "protocol.json").read_text())
    rows = [json.loads(p.read_text()) for p in sorted(directory.glob("case-*.json"))]
    expected = len(protocol["synthetic_conditions"]) + 3 * len(protocol["air_cases"])
    if len(rows) != expected or len({row["case"] for row in rows}) != expected:
        raise ValueError("incomplete or duplicate evaluation cases")
    groups = defaultdict(list)
    room_scores = defaultdict(list)
    search_renders = 0
    extra_validation_renders = 0
    for row in rows:
        group = row["case"].split(":")[0]
        if "admission_error" in row:
            if row["qualified_profile_proxy"] or any(
                name in row for name in ["classical", "neural"]
            ):
                raise ValueError("admission rejection contains an accepted method")
            groups[group].append(
                {
                    "case": row["case"],
                    "admitted": False,
                    "qualified": False,
                    "error": row["admission_error"],
                }
            )
            continue
        mask = row["target_mask"]
        curve_counts = [sum(mask[i : i + 32]) for i in range(0, 224, 32)]
        for method in ["classical", "neural"]:
            result = row[method]
            trace = result["trace"]
            if (
                not 0
                < len(trace)
                == result["native_evaluations"]
                <= protocol["native_budget_per_method"]
            ):
                raise ValueError("native budget mismatch")
            search_renders += len(trace)
            best = min(item["mse_db2"] for item in trace)
            if best != result["best"]["mse_db2"] or not math.isclose(
                math.sqrt(best), result["profile_rmse_db"], rel_tol=1e-12
            ):
                raise ValueError("reported best does not match trace")
            if result["qualified_profile_proxy"] != (
                best <= result["match_rmse_limit_db"] ** 2
            ):
                raise ValueError("qualification disagrees with exact native error")
            for item in trace:
                if (
                    item["native_tail"] != "Infinite"
                    or len(item["native_sha256"]) != 64
                ):
                    raise ValueError("missing conservative tail or native hash")
                if any(not 0 <= value <= 1 for value in item["unit"]):
                    raise ValueError("unbounded selected control")
                reconstructed = sum(
                    count * error**2
                    for count, error in zip(
                        curve_counts, item["per_curve_rmse_db"], strict=True
                    )
                ) / sum(curve_counts)
                if not math.isclose(
                    reconstructed, item["mse_db2"], rel_tol=1e-10, abs_tol=1e-12
                ):
                    raise ValueError("per-curve errors disagree with native objective")
            if group == "synthetic":
                if not math.isfinite(result["clean_full_window_rmse_db"]):
                    raise ValueError("missing clean-reference verification")
                extra_validation_renders += 1
        selected = min(
            ["classical", "neural"], key=lambda name: row[name]["profile_rmse_db"]
        )
        if (
            selected != row["selected"]
            or row["qualified_profile_proxy"]
            != row[selected]["qualified_profile_proxy"]
        ):
            raise ValueError("fallback selection disagrees with native objective")
        record = {
            "case": row["case"],
            "admitted": True,
            "selected": selected,
            "qualified": row["qualified_profile_proxy"],
            "classical_rmse_db": row["classical"]["profile_rmse_db"],
            "neural_rmse_db": row["neural"]["profile_rmse_db"],
            "selected_rmse_db": row[selected]["profile_rmse_db"],
        }
        if group == "synthetic":
            record["selected_clean_full_window_rmse_db"] = row[selected][
                "clean_full_window_rmse_db"
            ]
        else:
            room_scores[row["provenance"]["room"]].append(record["selected_rmse_db"])
        groups[group].append(record)
    summary = {
        "cases": expected,
        "native_search_renders": search_renders,
        "extra_clean_validation_renders": extra_validation_renders,
        "groups": {},
        "measured_arrival_sensitivity": {},
    }
    for name, entries in groups.items():
        valid = [row for row in entries if row["admitted"]]
        summary["groups"][name] = {
            "cases": len(entries),
            "admitted": len(valid),
            "qualified": sum(row["qualified"] for row in entries),
            "neural_lower_error": sum(
                row["neural_rmse_db"] < row["classical_rmse_db"] for row in valid
            ),
            "classical_median_rmse_db": statistics.median(
                row["classical_rmse_db"] for row in valid
            )
            if valid
            else None,
            "neural_median_rmse_db": statistics.median(
                row["neural_rmse_db"] for row in valid
            )
            if valid
            else None,
            "rows": entries,
        }
    for room, values in room_scores.items():
        summary["measured_arrival_sensitivity"][room] = {
            "annotations": len(values),
            "min_selected_rmse_db": min(values),
            "max_selected_rmse_db": max(values),
            "spread_db": max(values) - min(values),
        }
    summary["scope"] = (
        "Six measured rooms with correlated arrival variants, not 18 independent rooms. "
        "Approximate origins and legacy license-file reconciliation remain limitations. "
        "No spatial, perceptual, calibrated confidence or automatic adoption claim."
    )
    return summary


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--evaluation", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    summary = reduce(args.evaluation)
    args.output.write_text(json.dumps(summary, indent=2, allow_nan=False) + "\n")
    print(
        json.dumps(
            {
                name: {k: v for k, v in group.items() if k != "rows"}
                for name, group in summary["groups"].items()
            }
        )
    )


if __name__ == "__main__":
    main()
