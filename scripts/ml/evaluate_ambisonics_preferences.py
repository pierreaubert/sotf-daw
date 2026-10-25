"""Fixed synthetic listener conditions, native qualification and review exports."""

import argparse
import copy
import json
from pathlib import Path

from ambisonics_preference_cases import session
from fit_ambisonics_preferences import run


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--renderer", type=Path, required=True)
    parser.add_argument("--work", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.output.exists():
        raise ValueError("Refuse to overwrite evaluation")
    args.work.mkdir(parents=True, exist_ok=False)
    base = session(args.work / "fixtures", args.renderer)
    cases = []
    for kind in [
        "consistent",
        "baseline-preferred",
        "ties",
        "heterogeneous",
        "side-bias",
        "level-mismatch",
        "energy-mismatch",
    ]:
        data = copy.deepcopy(base)
        data["context"]["session_id"] = "functional-" + kind
        for t in data["trials"]:
            a, b = int(t["a"][-1]), int(t["b"][-1])
            programme = int(t["programme_id"][-1])
            rank = (
                [2, 0, 1]
                if kind == "baseline-preferred"
                or (kind == "heterogeneous" and programme >= 4)
                else [0, 2, 1]
            )
            t["choice"] = "a" if rank[a] > rank[b] else "b"
            if kind == "ties":
                t["choice"] = "tie"
            if kind == "side-bias":
                t["choice"] = "a"
        if kind == "level-mismatch":
            for p in data["programmes"]:
                p["post_gains_db"]["candidate-1"] += 3
        if kind == "energy-mismatch":
            data["candidates"][1]["parameters"]["speaker_gains"] = [
                1.05,
                1.05,
                1.05,
                1,
                1.05,
                1.05,
            ]
        source = args.work / f"{kind}-session.json"
        source.write_text(json.dumps(data, indent=2) + "\n")
        report = run(data, args.renderer, args.work / kind)
        expected = kind in {"consistent", "baseline-preferred"}
        if report["accepted"] != expected or (
            expected
            and report["selected_id"]
            != ("candidate-0" if kind == "baseline-preferred" else "candidate-1")
        ):
            raise AssertionError(kind)
        cases.append(
            {
                "case": kind,
                "session": str(source.resolve()),
                "report": str((args.work / kind / "report.json").resolve()),
                "accepted": report["accepted"],
                "selected_id": report["selected_id"],
                "reasons": report["reasons"],
                "native_auditions": len(report["native_validation"]["auditions"]),
                "reopen_max_error": max(report.get("reopen_max_errors", [0])),
            }
        )
    result = {
        "scope": "Seven simulated preference conditions over eight generated source groups and three corrections; not human listener observations, learned spatial quality, calibrated confidence or population acceptance.",
        "cases": cases,
    }
    args.output.write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
