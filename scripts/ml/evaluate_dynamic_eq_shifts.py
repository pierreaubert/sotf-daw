#!/usr/bin/env python3
"""Frozen fitter evaluation across physical rates, routings and channel controls."""

import argparse
import json
from pathlib import Path

from dynamic_eq_shift_cases import SPECS, case
from fit_dynamic_eq import run


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--renderer", type=Path, required=True)
    p.add_argument("--work", type=Path, required=True)
    p.add_argument("--output", type=Path, required=True)
    p.add_argument("--case")
    args = p.parse_args()
    args.work.mkdir(parents=True, exist_ok=False)
    rows = []
    for index, spec in enumerate(SPECS):
        if args.case and args.case != spec["id"]:
            continue
        data = case(
            args.work / (spec["id"] + "-case"),
            args.renderer,
            spec,
            20621026 + index * 1000,
        )
        for method in ["classical", "surrogate"]:
            folder = args.work / (spec["id"] + "-" + method)
            r = run(data, args.renderer, folder, method=method, budget=160)
            row = {
                "case": spec["id"],
                "method": method,
                "sample_rate": spec["rate"],
                "channels": spec["channels"],
                "bands": len(spec["bands"]),
                "target": str(args.work / (spec["id"] + "-case") / "target.json"),
                "report": str(folder / "report.json"),
                "accepted": r["accepted"],
                "reasons": r["reasons"],
                "search_evaluations": r["search"]["evaluations"],
                "native_render_calls": r["native_render_calls"],
                "worst_validation_relative_rms": max(
                    c["metrics"]["relative_rms"]
                    for c in r["qualification"]
                    if c["role"] == "validation"
                ),
            }
            rows.append(row)
            print(json.dumps(row), flush=True)
            # Preserve partial authoritative progress after each terminal run.
            args.output.write_text(
                json.dumps(
                    {
                        "cases": rows,
                        "scope": "Fixed synthetic shifts and negative controls; no fitter tuning, real-material or population acceptance.",
                    },
                    indent=2,
                )
                + "\n"
            )
    if not rows:
        raise ValueError("Unknown shift case")


if __name__ == "__main__":
    main()
