#!/usr/bin/env python3
"""Fresh curve holdout and fixed native import/export probes; no training."""

import argparse
import hashlib
import json
import subprocess
import sys
import time
from pathlib import Path

import numpy as np
from compressor_curve import GRID, Model, dataset, fit_curve, response


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--model", type=Path, required=True)
    parser.add_argument("--renderer", type=Path, required=True)
    parser.add_argument("--artifacts", type=Path, required=True)
    args = parser.parse_args()
    protocol = json.loads((args.artifacts / "protocol.json").read_text())
    training = json.loads(args.model.with_name("training-report.json").read_text())
    model = Model(args.model, training["weights_sha256"])
    manifest = json.loads(args.model.with_name("data-manifest.json").read_text())
    excluded = set().union(*(set(ids) for ids in manifest["split_ids"].values()))
    parameters, curves, ids = dataset(
        protocol["final_examples"], protocol["final_seed"]
    )
    if excluded & set(ids):
        raise ValueError("Fresh holdout identity overlap")
    final = args.artifacts / "final"
    final.mkdir(exist_ok=False)
    fits = []
    raw_errors = []
    for index, (p, curve, identity) in enumerate(zip(parameters, curves, ids)):
        prediction = model.predict(curve)
        raw_errors.append(float(np.mean((response(prediction) - curve) ** 2)))
        start = time.monotonic()
        result = fit_curve(GRID, curve, model, 100)
        fits.append(
            {
                "case": index,
                "parameter_identity": identity,
                "source_parameters": p.tolist(),
                "target_dbfs": curve.tolist(),
                "raw_prediction": prediction.tolist(),
                "fit": result,
                "warm_fit_seconds": time.monotonic() - start,
            }
        )
    holdout = {
        "model_sha256": model.digest,
        "cases": len(fits),
        "rows": fits,
        "raw_mse_db2": float(np.mean(raw_errors)),
        "identity_baseline_mse_db2": float(np.mean((curves - GRID) ** 2)),
        "combined_accepted": sum(r["fit"]["accepted"] for r in fits),
        "neural_accepted": sum(r["fit"]["candidates"][0]["accepted"] for r in fits),
        "qualification": "Synthetic static curves; warm fitting times exclude import/load/native realization and are not controlled speed evidence",
    }
    (final / "fresh-holdout.json").write_text(json.dumps(holdout, indent=2) + "\n")
    x = GRID.copy()
    one = [-18, np.log(4), 6, 2]
    cases = [
        ("single-soft", 48000, [one], [], 5, False),
        (
            "three-band",
            44100,
            [[-35, np.log(3), 8, 1], [-22, np.log(5), 4, -1], [-12, np.log(2), 10, 0]],
            [200, 2000],
            5,
            False,
        ),
        ("five-band", 96000, parameters[:5], [200, 2000, 8000, 12000], 5, False),
        ("constant-gain", 48000, [[-30, 0, 6, 3]], [], 5, False),
        ("hard-knee", 48000, [[-24, np.log(6), 0, 0]], [], 5, False),
        ("near-unity", 48000, [[-20, np.log(1.0001), 20, 1]], [], 5, False),
        ("shift-slow-attack", 192000, [[-54, np.log(16), 20, -2]], [], 100, False),
        ("noise", 48000, [one], [], 5, False),
        ("irregular", 48000, [one], [], 5, False),
        ("jagged", 48000, [one], [], 5, False),
        ("classical-only", 48000, [one], [], 5, True),
    ]
    rows = []
    rng = np.random.default_rng(protocol["final_seed"] + 1)
    for name, rate, params, crossovers, attack, no_model in cases:
        levels = (
            np.sort(np.r_[-84, rng.uniform(-84, 0, 126), 0])
            if name == "irregular"
            else x
        )
        bands = []
        for p in params:
            output = response(p, levels)
            if name == "noise":
                output = output + rng.normal(0, 0.003, len(output))
            if name == "jagged":
                output = levels + np.where(np.arange(len(levels)) % 2, 3.0, -3.0)
            bands.append(
                {
                    "input_dbfs": levels.tolist(),
                    "output_dbfs": output.tolist(),
                    "attack_ms": attack,
                    "release_ms": 50,
                }
            )
        target = {
            "format": "sotf-compressor-static-target-v1",
            "level_convention": "constant_amplitude_dbfs",
            "sample_rate": rate,
            "crossovers_hz": crossovers,
            "bands": bands,
        }
        path = final / (name + "-target.json")
        path.write_text(json.dumps(target, indent=2) + "\n")
        cmd = [
            sys.executable,
            str(Path(__file__).with_name("fit_compressor_curve.py")),
            "--target",
            str(path),
            "--renderer",
            str(args.renderer),
            "--output",
            str(final / (name + "-export")),
            "--work",
            str(final / (name + "-work")),
        ]
        if not no_model:
            cmd.extend(["--model", str(args.model), "--model-sha256", model.digest])
        start = time.monotonic()
        completed = subprocess.run(
            cmd, capture_output=True, text=True, check=False, timeout=600
        )
        if completed.returncode not in (0, 2):
            raise ValueError(completed.stderr)
        report = json.loads((final / (name + "-export/report.json")).read_text())
        if (
            report["accepted"] != (completed.returncode == 0)
            or (final / (name + "-export/preset.json")).exists() != report["accepted"]
        ):
            raise ValueError("Export decision/status mismatch")
        if not report["accepted"] and report["parameters"] is not None:
            raise ValueError("Rejected preset exposed controls")
        rows.append(
            {
                "case": name,
                "command": cmd,
                "exit_code": completed.returncode,
                "seconds": time.monotonic() - start,
                "accepted": report["accepted"],
                "reasons": report["reasons"],
            }
        )
        print(json.dumps(rows[-1]), flush=True)
    if rows[-2]["case"] != "jagged" or rows[-2]["accepted"]:
        raise ValueError("Incompatible curve was exported")
    if hashlib.sha256(args.model.read_bytes()).hexdigest() != model.digest:
        raise ValueError("Frozen model changed")
    (final / "native-summary.json").write_text(json.dumps(rows, indent=2) + "\n")
    print(
        json.dumps(
            {
                "holdout_cases": len(fits),
                "combined_accepted": holdout["combined_accepted"],
                "neural_accepted": holdout["neural_accepted"],
            }
        ),
        flush=True,
    )


if __name__ == "__main__":
    main()
