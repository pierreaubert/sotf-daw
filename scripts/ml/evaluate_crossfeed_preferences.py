#!/usr/bin/env python3
"""Frozen synthetic preference probes; no claim of measured listener benefit."""

import argparse
import copy
import json
from pathlib import Path

import fit_crossfeed_preferences as fitter
import numpy as np
from scipy.special import expit


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--template", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    template = json.loads(args.template.read_text())
    args.output.mkdir(parents=True, exist_ok=False)
    protocol = {
        "seed": 20271026,
        "sessions_per_family": 8,
        "families": {"strong": [0, 2.5, 0.5], "weak": [0, 0, 0]},
        "heldout_programmes": 8,
        "policy": fitter.POLICY,
        "qualification": "Synthetic logistic choices from the assumed model; not independent physical listening evidence",
    }
    (args.output / "protocol.json").write_text(json.dumps(protocol, indent=2) + "\n")
    rows = []
    for family, utilities in protocol["families"].items():
        for index in range(protocol["sessions_per_family"]):
            seed = protocol["seed"] + index + (100 if family == "weak" else 0)
            rng = np.random.default_rng(seed)
            data = copy.deepcopy(template)
            data["data_origin"] = "synthetic_validation"
            data["context"]["session_id"] = f"synthetic-{family}-{index}"
            ids = [c["id"] for c in data["candidates"]]
            test = []
            for split in [data["trials"], copy.deepcopy(data["trials"])]:
                for trial in split:
                    probability = expit(
                        utilities[ids.index(trial["a"])]
                        - utilities[ids.index(trial["b"])]
                    )
                    trial["choice"] = "a" if rng.random() < probability else "b"
                if split is not data["trials"]:
                    for trial in split:
                        trial["id"] = "heldout-" + trial["id"]
                        trial["programme_id"] = "heldout-" + trial["programme_id"]
                    test = split
            result = fitter.fit(data)
            theta = np.r_[
                result["model"]["utilities"], result["model"]["presentation_side_bias"]
            ]
            test_data = dict(data, trials=test)
            _, design, labels, _, _ = fitter.validate(test_data)
            logits = design @ theta
            log_loss = float(np.mean(np.logaddexp(0, logits) - labels * logits))
            # A simple vote baseline has no learned pairwise model or abstention.
            wins = np.zeros(len(ids))
            for trial in data["trials"]:
                wins[ids.index(trial[trial["choice"]])] += 1
            learned_id = ids[int(np.argmax(theta[:-1]))]
            vote_id = ids[int(np.argmax(wins))]
            expected = ids[int(np.argmax(utilities))] if family == "strong" else None
            name = f"{family}-{index}"
            (args.output / (name + "-session.json")).write_text(
                json.dumps(data, indent=2) + "\n"
            )
            (args.output / (name + "-heldout.json")).write_text(
                json.dumps(test_data, indent=2) + "\n"
            )
            (args.output / (name + "-report.json")).write_text(
                json.dumps(result, indent=2) + "\n"
            )
            rows.append(
                {
                    "case": name,
                    "family": family,
                    "expected_best": expected,
                    "learned_best": learned_id,
                    "vote_best": vote_id,
                    "accepted": result["accepted"],
                    "selected_id": result["selected_id"],
                    "heldout_log_loss": log_loss,
                    "uniform_log_loss": float(np.log(2)),
                    "reasons": result["reasons"],
                }
            )
    summary = {
        "sessions": len(rows),
        "rows": rows,
        "strong_accepted_correct": sum(
            r["accepted"] and r["selected_id"] == r["expected_best"]
            for r in rows
            if r["family"] == "strong"
        ),
        "strong_accepted_wrong": sum(
            r["accepted"] and r["selected_id"] != r["expected_best"]
            for r in rows
            if r["family"] == "strong"
        ),
        "weak_accepted": sum(r["accepted"] for r in rows if r["family"] == "weak"),
        "qualification": protocol["qualification"],
    }
    (args.output / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
    print(json.dumps({k: v for k, v in summary.items() if k != "rows"}))


if __name__ == "__main__":
    main()
