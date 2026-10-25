#!/usr/bin/env python3
"""Equal native-budget neural versus analytic-only initialization on fresh pairs."""

import argparse
import json
import time
from pathlib import Path

import numpy as np
from evaluate_limiter_release import cases
from limiter_native import render
from limiter_release import MAX_LIMIT, RMS_LIMIT, Model, descriptors
from scipy.optimize import minimize_scalar


def run_method(dry, target, rate, fixed, renderer, root, initial, scale):
    cache, attempts = {}, []
    started = time.monotonic()

    def objective(value):
        value = float(np.clip(value, 1, 3))
        key = round(value, 10)
        if key not in cache:
            output, meta = render(
                dry,
                rate,
                renderer,
                root / f"render-{len(cache):03d}",
                193,
                fixed | {"release_ms": 10**value},
            )
            delta = output.astype(np.float64) - target
            cache[key] = {
                "release_ms": 10**value,
                "relative_rms": float(np.sqrt(np.mean(delta**2)) / scale),
                "max_error": float(np.max(np.abs(delta))),
                "native": meta,
            }
        attempts.append(key)
        return cache[key]["relative_rms"] ** 2

    for value in [1, 3, np.log10(initial)]:
        objective(value)
    center = min(cache, key=lambda key: cache[key]["relative_rms"])
    minimize_scalar(
        objective,
        bounds=(max(1, center - 0.3), min(3, center + 0.3)),
        method="bounded",
        options={"maxiter": 21, "xatol": 1e-5},
    )
    best = min(cache.values(), key=lambda item: item["relative_rms"])
    result = {
        "attempted_evaluations": len(attempts),
        "unique_native_renders": len(cache),
        "seconds": time.monotonic() - started,
        "waveform_qualified": best["relative_rms"] <= RMS_LIMIT
        and best["max_error"] <= MAX_LIMIT,
        "best": best,
    }
    if len(attempts) > 24:
        raise ValueError("Exceeded equal method budget")
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--renderer", type=Path, required=True)
    parser.add_argument("--model", type=Path, required=True)
    parser.add_argument("--model-sha256", required=True)
    parser.add_argument("--work", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--seed", type=int, default=20481026)
    args = parser.parse_args()
    if args.work.exists() or args.output.exists():
        raise ValueError("Use fresh benchmark paths")
    load_start = time.monotonic()
    model = Model(args.model, args.model_sha256)
    load_seconds = time.monotonic() - load_start
    rows = []
    for name, dry, rate, fixed, release, identity, kind, noise in cases(args.seed):
        if kind == "negative-control":
            continue
        root = args.work / name
        target, _ = render(
            dry,
            rate,
            args.renderer,
            root / "target",
            127,
            fixed | {"release_ms": release},
        )
        if noise:
            target = target + np.random.default_rng(args.seed + 900).normal(
                0, noise, target.shape
            ).astype(np.float32)
        start = time.monotonic()
        features, analytic = descriptors(dry, target, rate, fixed)
        feature_seconds = time.monotonic() - start
        start = time.monotonic()
        proposed = model.predict_features(features)
        inference_seconds = time.monotonic() - start
        scale = max(float(np.sqrt(np.mean(target.astype(np.float64) ** 2))), 1e-6)
        methods = {}
        for method, initial in [("neural", proposed), ("analytic-only", analytic)]:
            methods[method] = run_method(
                dry, target, rate, fixed, args.renderer, root / method, initial, scale
            )
        rows.append(
            {
                "name": name,
                "identity": identity,
                "true_release_ms": release,
                "methods": methods,
                "feature_seconds": feature_seconds,
                "inference_seconds": inference_seconds,
            }
        )
        print(
            json.dumps(
                {
                    "name": name,
                    "attempts": {
                        k: v["attempted_evaluations"] for k, v in methods.items()
                    },
                }
            ),
            flush=True,
        )
    report = {
        "seed": args.seed,
        "model_load_seconds": load_seconds,
        "budget_per_method": 24,
        "rows": rows,
        "methods": {
            name: {
                "waveform_qualified": sum(
                    r["methods"][name]["waveform_qualified"] for r in rows
                ),
                "total": len(rows),
                "median_attempts": float(
                    np.median(
                        [r["methods"][name]["attempted_evaluations"] for r in rows]
                    )
                ),
                "median_seconds": float(
                    np.median([r["methods"][name]["seconds"] for r in rows])
                ),
            }
            for name in ["neural", "analytic-only"]
        },
        "qualification": "Fresh independent signal seeds; no cross-method cache. Additional baseline-only comparison after initial evaluation, with model/feature/gate/optimizer unchanged. Waveform gates only here, no preset export or independent ambiguity acceptance. Timing includes subprocess/I/O and is not isolated inference or realtime performance.",
    }
    args.output.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report["methods"]), flush=True)


if __name__ == "__main__":
    main()
