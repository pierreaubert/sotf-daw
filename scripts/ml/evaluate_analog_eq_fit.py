"""Equal-budget GP/classical native search on generated native and delayed targets."""

import argparse
import json
from pathlib import Path

from analog_eq_cases import session
from fit_analog_eq_hardware import run


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--renderer", type=Path, required=True)
    parser.add_argument("--work", type=Path, required=True)
    parser.add_argument("--report", type=Path, required=True)
    parser.add_argument("--budget", type=int, default=128)
    args = parser.parse_args()
    if args.report.exists():
        raise ValueError("Refuse to overwrite evaluation")
    args.work.mkdir(parents=True, exist_ok=False)
    rows = []
    for kind in ["matched", "delay"]:
        spec, _, truth = session(args.work / (kind + "-captures"), args.renderer, kind)
        for method in ["classical", "surrogate"]:
            folder = args.work / f"{kind}-{method}"
            report = run(spec, args.renderer, folder, args.budget, method)
            rows.append(
                {
                    "case": kind,
                    "method": method,
                    "spec": str(
                        (args.work / (kind + "-captures") / "fit-spec.json").resolve()
                    ),
                    "report": str((folder / "report.json").resolve()),
                    "accepted": report["accepted"],
                    "reasons": report["reasons"],
                    "evaluations": report["search"]["evaluations"],
                    "training_loss": report["search"]["best"]["loss"],
                    "held_relative_rms": [
                        r["metrics"]["relative_rms"] for r in report["qualification"]
                    ],
                    "reopen_max": max(
                        r["reopen_error"] for r in report["qualification"]
                    ),
                    "selected_parameters": report["search"]["best"]["parameters"],
                    "refinement_result": report["search"]["refinement_result"],
                    "generated_target_controls": truth,
                }
            )
    result = {
        "cases": rows,
        "scope": "Two generated transfer systems over the same12 source groups; no real hardware, calibrated parameter recovery, perceptual quality or general surrogate advantage claim.",
    }
    args.report.write_text(json.dumps(result, indent=2, allow_nan=False) + "\n")
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
