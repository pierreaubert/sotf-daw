#!/usr/bin/env python3
"""Fixed synthetic preference probes with actual native candidate qualification."""

import argparse
import copy
import json
from pathlib import Path

from binaural_preference_cases import session
from fit_binaural_preferences import run


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--renderer", type=Path, required=True)
    parser.add_argument("--work", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    args.work.mkdir(parents=True, exist_ok=False)
    base = session(args.work / "fixtures", args.renderer)
    rows = []
    for kind in [
        "consistent",
        "baseline-preferred",
        "ties",
        "heterogeneous",
        "side-bias",
        "level-mismatch",
    ]:
        data = copy.deepcopy(base)
        data["context"]["session_id"] = "functional-" + kind
        for t in data["trials"]:
            a = int(t["a"][-1])
            b = int(t["b"][-1])
            p = int(t["programme_id"].split("-")[-1])
            rank = (
                [2, 0, 1]
                if kind == "baseline-preferred" or (kind == "heterogeneous" and p >= 4)
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
        source = args.work / (kind + "-session.json")
        source.write_text(json.dumps(data, indent=2) + "\n")
        report = run(data, args.renderer, args.work / kind)
        expected = kind in {"consistent", "baseline-preferred"}
        if report["accepted"] != expected:
            raise AssertionError(kind)
        if expected and report["selected_id"] != (
            "candidate-0" if kind == "baseline-preferred" else "candidate-1"
        ):
            raise AssertionError(kind)
        rows.append(
            {
                "case": kind,
                "session": str(source),
                "report": str(args.work / kind / "report.json"),
                "accepted": report["accepted"],
                "selected_id": report["selected_id"],
                "reasons": report["reasons"],
                "native_auditions": len(report["native_validation"]["auditions"]),
                "reopen_max_error": max(report.get("reopen_max_errors", [0])),
            }
        )
    result = {
        "scope": "Six synthetic functional cases, three synthetic SOFAs and eight synthetic programmes; not listener data, pretrained model quality, localization or population acceptance",
        "cases": rows,
    }
    args.output.write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
