"""Frozen generated positive/negative controls for target-specific hardware learning."""

import argparse
import json
from pathlib import Path

from analog_hardware_cases import session
from train_analog_hardware import run


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--work", type=Path, required=True)
    parser.add_argument("--report", type=Path, required=True)
    args = parser.parse_args()
    if args.report.exists():
        raise ValueError("Refuse to overwrite retained comparison")
    args.work.mkdir(parents=True, exist_ok=False)
    rows = []
    for kind in ["cross-memory", "hammerstein"]:
        data = session(args.work / (kind + "-captures"), kind)
        model, report = run(data, args.work / kind, taps=3, hidden=6, iterations=300)
        rows.append(
            {
                "case": kind,
                "manifest": str(
                    (args.work / (kind + "-captures") / "manifest.json").resolve()
                ),
                "candidate": str((args.work / kind / "candidate.json").resolve()),
                "report": str((args.work / kind / "report.json").resolve()),
                "offline_quality_passed": report["offline_quality_passed"],
                "hardware_model_admitted": False,
                "training": report["training"],
                "held_splits": report["held_splits"],
                "global_output_bound": model["global_output_bound"],
            }
        )
    result = {
        "cases": rows,
        "scope": "Two generated functional targets; not physical hardware, real recordings, a neural quality guarantee or production admission.",
    }
    args.report.write_text(json.dumps(result, indent=2, allow_nan=False) + "\n")
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
