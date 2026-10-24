#!/usr/bin/env python3
"""Compare a frozen EQ initializer with a classical seed on independent curves.

This records sampled/dense accuracy and actual residual calls. Timings are
observational; they do not establish a DAW deadline or a controlled speed win.
No weights or training hyperparameters are updated by this evaluator.
"""

import argparse
import json
import time
from pathlib import Path

import eq_assistant as eq
import numpy as np
import torch
from scipy.optimize import least_squares


def classical_seed(frequencies, target):
    peak = int(np.argmax(np.abs(target)))
    gain = target[peak]
    centered = np.sign(gain) * target
    left = np.flatnonzero(centered[:peak] <= abs(gain) / 2)
    right = np.flatnonzero(centered[peak + 1 :] <= abs(gain) / 2)
    q = 1.0
    if len(left) and len(right):
        bandwidth = frequencies[peak + 1 + right[0]] - frequencies[left[-1]]
        q = float(np.clip(frequencies[peak] / bandwidth, 0.35, 8.0))
    return np.array([np.log(np.clip(frequencies[peak], 50, 10000)), np.log(q), gain])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("model", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--cases", type=int, default=32)
    args = parser.parse_args()
    torch.set_num_threads(2)
    report = json.loads((args.model / "training-report.json").read_text())
    weights = args.model / "weights.json"
    if eq.sha(weights) != report["weights_sha256"]:
        raise ValueError("Frozen weights hash mismatch")
    load_start = time.monotonic()
    model = eq.network()
    raw = json.loads(weights.read_text())
    model.load_state_dict({n: torch.tensor(v) for n, v in raw["weights"].items()})
    model.eval()
    load_ms = (time.monotonic() - load_start) * 1000
    parameters, _, ids = eq.dataset(args.cases, 20264271)
    training = json.loads((args.model / "data-manifest.json").read_text())
    for split in training["split_ids"].values():
        assert not set(ids) & set(split), "evaluation parameter identity leaked"
    rows = []
    for rate in [44100.0, 48000.0, 96000.0]:
        for index, p in enumerate(parameters):
            target = eq.response(p, rate=rate)[0]
            start = time.monotonic()
            with torch.no_grad():
                neural = eq.physical(
                    model(torch.tensor(target[None] / 12, dtype=torch.float32))
                ).numpy()[0]
            infer_ms = (time.monotonic() - start) * 1000
            dense = np.unique(
                np.concatenate(
                    [
                        np.geomspace(20, 20000, 8193),
                        [0, rate / 2],
                        np.exp(p[0]) * np.array([0.99, 1.0, 1.01]),
                    ]
                )
            )
            for method, seed in [
                ("neural", neural),
                ("classical-half-bandwidth", classical_seed(eq.GRID, target)),
            ]:
                calls = 0

                def residual(candidate, source_rate=rate, observed=target):
                    nonlocal calls
                    calls += 1
                    return eq.response(candidate, rate=source_rate)[0] - observed

                start = time.monotonic()
                solved = least_squares(
                    residual,
                    np.clip(seed, eq.LOWER + 1e-8, eq.UPPER - 1e-8),
                    bounds=(eq.LOWER, eq.UPPER),
                    max_nfev=100,
                    ftol=1e-10,
                    xtol=1e-10,
                    gtol=1e-10,
                )
                elapsed_ms = (time.monotonic() - start) * 1000
                error = (
                    eq.response(solved.x, dense, rate)[0]
                    - eq.response(p, dense, rate)[0]
                )
                rows.append(
                    {
                        "case": index,
                        "parameter_sha256": ids[index],
                        "rate": rate,
                        "method": method,
                        "nfev": solved.nfev,
                        "residual_calls": calls,
                        "refinement_ms": elapsed_ms,
                        "neural_inference_ms": infer_ms,
                        "dense_rms_db": float(np.sqrt(np.mean(error**2))),
                        "dense_max_db": float(np.max(np.abs(error))),
                        "converged": bool(solved.success),
                    }
                )
    output = {
        "weights_sha256": eq.sha(weights),
        "kind": "frozen synthetic parameter and sample-rate comparison",
        "model_load_ms_excluding_python_imports": load_ms,
        "rows": rows,
        "limitations": [
            "finite dense samples, not a continuous guarantee",
            "timing not exclusive or DAW capacity acceptance",
            "not arbitrary multiband or measured-response validation",
            "out-of-family/noise cases remain separate gates",
        ],
    }
    args.output.write_text(json.dumps(output, indent=2) + "\n")
    for method in ["neural", "classical-half-bandwidth"]:
        subset = [r for r in rows if r["method"] == method]
        print(
            method,
            "cases",
            len(subset),
            "maximum_dense_error_db",
            max(r["dense_max_db"] for r in subset),
            "median_residual_calls",
            float(np.median([r["residual_calls"] for r in subset])),
        )


if __name__ == "__main__":
    main()
