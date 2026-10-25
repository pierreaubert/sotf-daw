#!/usr/bin/env python3
"""Freeze generated routing sessions and evaluate native-qualified suggestions."""

import argparse
import hashlib
import json
import time
from pathlib import Path

import numpy as np
from matrix_routing import fit
from matrix_routing_cases import cases
from matrix_routing_native import qualify, render


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--renderer", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--work", type=Path, required=True)
    parser.add_argument("--seed", type=int, default=20511026)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    if args.work.exists():
        raise ValueError("Use fresh native work")
    rows = []
    for index, (name, data, truth, expected) in enumerate(cases(args.seed)):
        path = args.output / (name + "-examples.json")
        path.write_text(json.dumps(data, allow_nan=False) + "\n")
        started = time.monotonic()
        result = fit(data)
        validation = []
        if result["accepted"]:
            result["native"] = qualify(data, result, args.renderer, args.work / name)
            rng = np.random.default_rng(np.random.SeedSequence([args.seed, index, 900]))
            matrix = np.array(result["matrix"])
            # New sources share only the session's known matrix, not input rows.
            for source in range(4):
                x = rng.uniform(-0.8, 0.8, (97, truth.shape[1]))
                y = x @ truth.T
                physical = np.zeros(
                    (len(x), max(data["input_channel_map"]) + 1), np.float32
                )
                physical[:, data["input_channel_map"]] = x
                actual, meta = render(
                    result["parameters"],
                    physical,
                    data["sample_rate"],
                    args.renderer,
                    args.work / name / f"fresh-{source}",
                    31,
                )
                delta = actual[:, data["output_channel_map"]] - y
                validation.append(
                    {
                        "input": x.tolist(),
                        "desired_output": y.tolist(),
                        "native": meta,
                        "native_rms": float(np.sqrt(np.mean(delta**2))),
                        "native_max_error": float(np.max(np.abs(delta))),
                        "statistical_rms": float(
                            np.sqrt(np.mean((x @ matrix.T - y) ** 2))
                        ),
                    }
                )
        row = {
            "name": name,
            "expected_admission": expected,
            "truth": truth.tolist(),
            "examples_sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
            "result": result,
            "fresh_validation": validation,
            "seconds": time.monotonic() - started,
            "work": str((args.work / name).resolve()),
        }
        (args.output / (name + "-result.json")).write_text(
            json.dumps(row, indent=2) + "\n"
        )
        rows.append(row)
        print(
            json.dumps(
                {
                    "name": name,
                    "accepted": result["accepted"],
                    "expected": expected,
                    "reasons": result["reasons"],
                }
            ),
            flush=True,
        )
    report = {
        "seed": args.seed,
        "renderer_sha256": hashlib.sha256(args.renderer.read_bytes()).hexdigest(),
        "rows": rows,
        "qualification": "Fresh generated explicit linear routing sessions with source-group validation and mismatch controls. No natural user examples, semantic channel inference or automatically applied routing.",
    }
    (args.output / "evaluation.json").write_text(json.dumps(report, indent=2) + "\n")


if __name__ == "__main__":
    main()
