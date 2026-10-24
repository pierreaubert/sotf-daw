#!/usr/bin/env python3
"""Evaluate a frozen multi-band initializer and its bounded export pipeline.

Fresh holdout identities exclude every earlier full/smoke split. Comparisons are
observational solver timings/call counts, not controlled runtime speed claims.
No checkpoint selection or training occurs in this script.
"""

import argparse
import contextlib
import io
import json
import time
from pathlib import Path
from types import SimpleNamespace

import eq_multiband as eq
import numpy as np
import torch


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--model", type=Path, required=True)
    parser.add_argument("--artifacts", type=Path, required=True)
    parser.add_argument("--cases", type=int, default=6)
    parser.add_argument("--holdout", type=int, default=1024)
    args = parser.parse_args()
    torch.set_num_threads(2)
    args.artifacts.mkdir(parents=True, exist_ok=True)
    weights = args.model / "weights.json"
    frozen_sha = eq.sha(weights)
    report = json.loads((args.model / "training-report.json").read_text())
    if frozen_sha != report["weights_sha256"]:
        raise ValueError("Frozen model identity mismatch")
    manifests = [
        json.loads((args.model / "data-manifest.json").read_text()),
        json.loads((args.artifacts / "smoke-model/data-manifest.json").read_text()),
    ]
    excluded = set().union(
        *(set(ids) for manifest in manifests for ids in manifest["split_ids"].values())
    )
    prior_overlap = set(manifests[0]["split_ids"]["test"]) & set(
        manifests[1]["split_ids"]["test"]
    )
    holdout = eq.dataset(args.holdout, 20263024)
    if excluded & set(holdout[3]):
        raise ValueError("Fresh holdout overlaps an earlier split")
    model = eq.network()
    portable = json.loads(weights.read_text())
    model.load_state_dict(
        {name: torch.as_tensor(value) for name, value in portable["weights"].items()}
    )
    model.eval()
    mse, predictions = eq.evaluate(model, holdout)
    flat = float(np.mean(holdout[2] ** 2))
    summary = {
        "weights_sha256_before_evaluation": frozen_sha,
        "checkpoint_sha256": report["checkpoint_sha256"],
        "fresh_holdout_seed": 20263024,
        "fresh_holdout_count": args.holdout,
        "fresh_holdout_ids": holdout[3],
        "prior_smoke_initial_test_overlap": len(prior_overlap),
        "qualification": "Initial report is preliminary because its seed overlaps the smoke. This final holdout is disjoint from every earlier split and scored after checkpoint selection.",
        "unrefined_response_db_mse": mse,
        "flat_baseline_db_mse": flat,
        "explained_response_fraction_vs_flat": 1 - mse / flat,
        "strata": [],
    }
    for rate in eq.TRAIN_RATES:
        selected = holdout[1] == rate
        error = eq.response(predictions[selected], rate=rate) - holdout[2][selected]
        summary["strata"].append(
            {
                "sample_rate": rate,
                "cases": int(selected.sum()),
                "mse": float(np.mean(error**2)),
            }
        )
    (args.artifacts / "fresh-holdout.json").write_text(
        json.dumps(summary, indent=2) + "\n"
    )
    print(
        json.dumps(
            {key: value for key, value in summary.items() if key != "fresh_holdout_ids"}
        ),
        flush=True,
    )
    parameters, _, _, ids = eq.dataset(args.cases, 20264024)
    if set(ids) & (excluded | set(holdout[3])):
        raise ValueError("Refinement comparisons overlap prior data")
    rows = []

    def run_target(name, target, rate):
        target_path = args.artifacts / f"{name}-target.json"
        target_path.write_text(
            json.dumps(
                {
                    "frequencies_hz": eq.GRID.tolist(),
                    "target_db": target.tolist(),
                    "sample_rate": rate,
                }
            )
            + "\n"
        )
        output = args.artifacts / f"{name}-proposal.json"
        try:
            with contextlib.redirect_stdout(io.StringIO()):
                eq.fit(
                    SimpleNamespace(
                        target=target_path,
                        output=output,
                        weights=weights,
                        max_evaluations=200,
                    )
                )
        except SystemExit as error:
            if error.code != 2:
                raise
        result = json.loads(output.read_text())
        result["case"] = name
        rows.append(result)
        print(
            name, "accepted", result["accepted"], "method", result["method"], flush=True
        )
        return result

    start = time.monotonic()
    for index, p in enumerate(parameters):
        for rate in eq.TRAIN_RATES:
            run_target(f"fresh-{index}-{int(rate)}", eq.response(p, rate=rate), rate)
    for index, p in enumerate(parameters[:2]):
        for rate in [88200.0, 192000.0]:
            run_target(f"shift-{index}-{int(rate)}", eq.response(p, rate=rate), rate)
    native = json.loads((args.artifacts / "native-target.json").read_text())
    if native["parameter_identity"] in (excluded | set(holdout[3]) | set(ids)):
        raise ValueError("Native acceptance fixture leaked into training/evaluation")
    native_exports = []
    for rate in eq.TRAIN_RATES:
        result = run_target(
            f"native-{int(rate)}",
            eq.response(native["source_parameters"], rate=rate),
            rate,
        )
        if not result["accepted"]:
            raise ValueError("Declared native acceptance fixture did not qualify")
        native_exports.append(result)
    (args.artifacts / "native-proposals.json").write_text(
        json.dumps(native_exports, indent=2) + "\n"
    )
    rejected = run_target(
        "out-of-family-jagged",
        np.where(np.arange(len(eq.GRID)) % 2, 3.0, -3.0),
        48000.0,
    )
    if rejected["accepted"] or rejected["parameters"] is not None:
        raise ValueError("Unrepresentable target exported filter parameters")
    if eq.sha(weights) != frozen_sha:
        raise ValueError("Weights changed during evaluation")
    overview = {
        "weights_sha256": frozen_sha,
        "cases": len(rows),
        "unique_parameter_identities": ids,
        "accepted": sum(row["accepted"] for row in rows),
        "refinement_elapsed_seconds": time.monotonic() - start,
        "rows": rows,
        "qualification": "Supplied sample accuracy; fixed in-family/shift probes, no measured-room or perceptual quality claim. Candidate times exclude initializer/cold import costs; no speed advantage inferred.",
    }
    (args.artifacts / "refinement-comparison.json").write_text(
        json.dumps(overview, indent=2) + "\n"
    )
    print(
        "Completed",
        len(rows),
        "comparisons; accepted",
        overview["accepted"],
        flush=True,
    )


if __name__ == "__main__":
    main()
