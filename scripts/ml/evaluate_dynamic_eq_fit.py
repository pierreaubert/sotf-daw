#!/usr/bin/env python3
"""Native GP-versus-classical comparison; preserve failures and all budgets."""

import argparse
import copy
import json
from pathlib import Path

from dynamic_eq_fit_cases import case
from fit_dynamic_eq import run


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--renderer", type=Path, required=True)
    parser.add_argument("--work", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    args.work.mkdir(parents=True, exist_ok=False)
    rows = []
    for kind, seed in [
        ("near", 20611026),
        ("far", 20612026),
        ("two-band", 20613026),
        ("shifted", 20614026),
        ("dry", 20615026),
        ("known-control", 20616026),
    ]:
        data = case(args.work / (kind + "-case"), args.renderer, kind, seed)
        if kind == "known-control":
            data = copy.deepcopy(data)
            data["template"] = json.loads(
                (args.work / (kind + "-case") / "truth.json").read_text()
            )["hidden_parameters"]
            (args.work / (kind + "-case") / "target.json").write_text(
                json.dumps(data, indent=2) + "\n"
            )
        for method in ["classical", "surrogate"]:
            folder = args.work / (kind + "-" + method)
            report = run(data, args.renderer, folder, method=method, budget=160)
            if kind in {"dry", "shifted"} and report["accepted"]:
                raise AssertionError("Negative control exported: " + kind)
            if kind == "known-control" and not report["accepted"]:
                raise AssertionError("Known-control wiring failed")
            rows.append(
                {
                    "case": kind,
                    "method": method,
                    "target": str(args.work / (kind + "-case") / "target.json"),
                    "report": str(folder / "report.json"),
                    "accepted": report["accepted"],
                    "reasons": report["reasons"],
                    "training_objective": report["search"]["best"]["loss"],
                    "worst_validation_relative_rms": max(
                        r["metrics"]["relative_rms"]
                        for r in report["qualification"]
                        if r["role"] == "validation"
                    ),
                    "search_evaluations": report["search"]["evaluations"],
                    "native_render_calls": report["native_render_calls"],
                }
            )
            print(json.dumps(rows[-1]), flush=True)
    result = {
        "cases": rows,
        "qualification": "Fresh fixed synthetic development signals, not listener/hardware data or a final generalization set. Known-control starts at truth and is only an export/admission control. No method-superiority assumption.",
    }
    args.output.write_text(json.dumps(result, indent=2) + "\n")


if __name__ == "__main__":
    main()
