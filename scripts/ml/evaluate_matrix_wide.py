#!/usr/bin/env python3
"""Evaluate full-width routing and independent per-output admission gates."""

import argparse
import copy
import hashlib
import json
import time
from pathlib import Path

import numpy as np
from matrix_routing import fit
from matrix_routing_cases import example
from matrix_routing_native import qualify, render


def cases(seed):
    for index, (inputs, outputs, rate) in enumerate(
        [
            (32, 32, 44100),
            (64, 17, 48000),
            (17, 64, 96000),
            (128, 128, 192000),
            (128, 1, 48000),
            (1, 128, 96000),
        ]
    ):
        data, truth = example(
            seed + index, inputs, outputs, sources=10, noise=0.0001, maps=True
        )
        data["sample_rate"] = rate
        yield f"wide-{inputs}x{outputs}", data, truth, True
    data, truth = example(seed + 100, inputs=2, outputs=128, sources=8, noise=0)
    for group in data["groups"]:
        y = np.array(group["desired_output"])
        y[:, -1] = np.where(np.arange(len(y)) % 2, 0.02, -0.02)
        group["desired_output"] = y.tolist()
    yield "one-bad-output", data, truth, False


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--renderer", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--work", type=Path, required=True)
    parser.add_argument("--seed", type=int, default=20541026)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    if args.work.exists():
        raise ValueError("Use new native work")
    rows = []
    for index, (name, data, truth, expected) in enumerate(cases(args.seed)):
        path = args.output / (name + "-examples.json")
        path.write_text(json.dumps(data, allow_nan=False) + "\n")
        started = time.monotonic()
        result = fit(data)
        validation = []
        if result["accepted"]:
            result["native"] = qualify(data, result, args.renderer, args.work / name)
            rng = np.random.default_rng(np.random.SeedSequence([args.seed, index, 999]))
            x = rng.uniform(-0.8, 0.8, (127, truth.shape[1]))
            y = x @ truth.T
            physical = np.zeros(
                (len(x), max(data["input_channel_map"]) + 1), np.float32
            )
            physical[:, data["input_channel_map"]] = x
            actual, metadata = render(
                result["parameters"],
                physical,
                data["sample_rate"],
                args.renderer,
                args.work / name / "fresh",
                1,
            )
            delta = actual[:, data["output_channel_map"]] - y
            validation.append(
                {
                    "input": x.tolist(),
                    "desired_output": y.tolist(),
                    "native": metadata,
                    "rms": float(np.sqrt(np.mean(delta**2))),
                    "worst_output_rms": float(np.sqrt(np.mean(delta**2, axis=0)).max()),
                }
            )
        if name == "one-bad-output":
            forced = copy.deepcopy(result)
            forced.update(
                accepted=True,
                parameters={
                    "input_channel_map": data["input_channel_map"],
                    "output_channel_map": data["output_channel_map"],
                    "matrix": np.array(result["matrix"]).ravel().tolist(),
                },
            )
            try:
                qualify(data, forced, args.renderer, args.work / name)
            except ValueError as error:
                if str(error) != "Native nested-heldout reference mismatch":
                    raise
                result[
                    "independent_native_gate_refused_after_statistical_gate_bypassed_for_test"
                ] = True
            else:
                raise ValueError("Native gate missed the bad output")
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
        rows.append(row)
        (args.output / "evaluation-in-progress.json").write_text(
            json.dumps({"rows": rows}) + "\n"
        )
        print(
            json.dumps(
                {
                    "name": name,
                    "accepted": result["accepted"],
                    "expected": expected,
                    "seconds": row["seconds"],
                    "reasons": result["reasons"],
                }
            ),
            flush=True,
        )
    report = {
        "seed": args.seed,
        "renderer_sha256": hashlib.sha256(args.renderer.read_bytes()).hexdigest(),
        "rows": rows,
        "qualification": "Fresh synthetic full-width linear routing and per-output control; no semantic/user/population or host performance claim. Bad-output native guard is deliberately tested after bypassing statistical admission, with no export.",
    }
    (args.output / "evaluation.json").write_text(json.dumps(report, indent=2) + "\n")
    (args.output / "evaluation-in-progress.json").unlink()


if __name__ == "__main__":
    main()
