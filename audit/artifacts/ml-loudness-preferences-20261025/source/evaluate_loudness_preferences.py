#!/usr/bin/env python3
"""Fresh synthetic mean-curve probes and explicit abstention controls."""

import argparse
import json
from pathlib import Path

import numpy as np
from loudness_preference_cases import session, target
from loudness_preferences import fit, validate


def linear_prediction(data, query):
    reference, cells = validate(data)
    _, inverse, counts = np.unique(
        [c["programme"] for c in cells], return_inverse=True, return_counts=True
    )
    weights = 1 / counts[inverse]
    x = (reference - np.asarray([c["level"] for c in cells])) / 20
    y = np.asarray([c["gains"] for c in cells])
    beta = np.sum(weights[:, None] * x[:, None] * y, axis=0) / (
        np.sum(weights * x * x) + 0.01
    )
    return beta * ((reference - query) / 20)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--artifacts", type=Path, required=True)
    args = parser.parse_args()
    protocol = json.loads((args.artifacts / "protocol.json").read_text())
    final = args.artifacts / "fresh-synthetic"
    final.mkdir(exist_ok=False)
    rows = []
    for index in range(protocol["final_sessions"]):
        data = session(protocol["final_seed"] + index)
        name = f"session-{index}"
        (final / (name + ".json")).write_text(json.dumps(data, indent=2) + "\n")
        for query in protocol["queries_db_spl"]:
            result = fit(data, query)
            expected = target(query)
            predicted = np.asarray(result["predicted_gains_db"])
            baseline = linear_prediction(data, query)
            inside = bool(
                np.all(expected >= result["interval_low_db"])
                and np.all(expected <= result["interval_high_db"])
            )
            rows.append(
                {
                    "session": name,
                    "query_db_spl": query,
                    "accepted": result["accepted"],
                    "gp_rmse_db": float(np.sqrt(np.mean((predicted - expected) ** 2))),
                    "linear_rmse_db": float(
                        np.sqrt(np.mean((baseline - expected) ** 2))
                    ),
                    "identity_rmse_db": float(np.sqrt(np.mean(expected**2))),
                    "known_synthetic_mean_inside_band": inside,
                    "reasons": result["reasons"],
                }
            )
            (final / f"{name}-{query}-report.json").write_text(
                json.dumps(result, indent=2) + "\n"
            )
    controls = []
    for kind in ["contradictory", "uncertain"]:
        for index in range(4):
            data = session(protocol["final_seed"] + 100 + index, kind)
            result = fit(data, 55)
            (final / f"{kind}-{index}-session.json").write_text(
                json.dumps(data, indent=2) + "\n"
            )
            (final / f"{kind}-{index}-report.json").write_text(
                json.dumps(result, indent=2) + "\n"
            )
            controls.append(
                {
                    "kind": kind,
                    "accepted": result["accepted"],
                    "reasons": result["reasons"],
                }
            )
    summary = {
        "queries": len(rows),
        "rows": rows,
        "controls": controls,
        "accepted_consistent_queries": sum(r["accepted"] for r in rows),
        "accepted_contradictory_or_uncertain": sum(r["accepted"] for r in controls),
        "median_gp_rmse_db": float(np.median([r["gp_rmse_db"] for r in rows])),
        "median_linear_rmse_db": float(np.median([r["linear_rmse_db"] for r in rows])),
        "qualification": "Synthetic known-mean checks only; correlated query pairs are not independent listener data or calibrated confidence coverage",
    }
    (final / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
    print(
        json.dumps({k: v for k, v in summary.items() if k not in ["rows", "controls"]})
    )


if __name__ == "__main__":
    main()
