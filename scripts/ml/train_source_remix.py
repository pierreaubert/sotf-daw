"""Freeze a centered-source mask before disjoint generated and ambiguity probes."""

import argparse
import hashlib
import json
from pathlib import Path

import numpy as np
from scipy.optimize import minimize
from source_remix_model import export, features, load, objective, predict


def scene(seed, kind="matched", rows=512):
    rng = np.random.default_rng(seed)
    foreground = 10 ** rng.uniform(-4, 1, rows)
    bed = foreground * 10 ** rng.uniform(-2, 2, rows)
    foreground[: rows // 10] = 0
    bed[rows // 10 : rows // 5] = 0
    f = np.sqrt(foreground) * np.exp(2j * np.pi * rng.random(rows))
    left = np.sqrt(bed / 2) * (rng.normal(size=rows) + 1j * rng.normal(size=rows))
    right = np.sqrt(bed / 2) * (rng.normal(size=rows) + 1j * rng.normal(size=rows))
    if kind == "dual_mono_bed":
        right = left.copy()
    if kind == "anti_phase_bed":
        right = -left
    fl, fr = f.copy(), f.copy()
    if kind == "panned_foreground":
        fr *= 0.2
    if kind == "phase_shifted_foreground":
        fr *= np.exp(0.7j)
    l, r = fl + left, fr + right
    if kind == "level_shift":
        l *= 4
        r *= 4
    mid_foreground = (fl + fr) / 2
    mid_bed = (left + right) / 2
    target = abs(mid_foreground) ** 2 / np.maximum(
        abs(mid_foreground) ** 2 + abs(mid_bed) ** 2, 1e-20
    )
    frequency = rng.uniform(1 / 256, 255 / 256, rows)
    x = features(l, r, frequency)
    return (
        x,
        target,
        {
            "seed": seed,
            "kind": kind,
            "rows": rows,
            "features_sha256": hashlib.sha256(x.astype("<f8").tobytes()).hexdigest(),
            "target_sha256": hashlib.sha256(target.astype("<f8").tobytes()).hexdigest(),
        },
    )


def corpus(first, groups, kind="matched"):
    data = [scene(first + i, kind) for i in range(groups)]
    return (
        np.vstack([r[0] for r in data]),
        np.concatenate([r[1] for r in data]),
        [r[2] for r in data],
    )


def metrics(theta, x, y):
    p = predict(theta, x)
    # M/S Wiener estimate for an uncorrelated symmetric bed, not semantic separation.
    fraction = x[:, 0]
    analytic = np.clip((2 * fraction - 1) / np.maximum(fraction, 1e-20), 0, 1)
    return {
        "mask_mse": float(np.mean((p - y) ** 2)),
        "analytic_ms_mask_mse": float(np.mean((analytic - y) ** 2)),
        "confidence_coverage": float(np.mean((p >= 0.85) | (p <= 0.15))),
        "confident_foreground_when_bed_dominant_fraction": float(
            np.mean((p >= 0.85) & (y < 0.5))
        ),
        "rows": len(y),
    }


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--output", type=Path, required=True)
    args = p.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    x, y, train = corpus(20271120, 64)
    vx, vy, validation = corpus(20272120, 16)
    initial = np.random.default_rng(20271119).normal(0, 0.2, 65)
    fit = minimize(
        objective,
        initial,
        args=(x, y),
        jac=True,
        method="L-BFGS-B",
        bounds=[(-16, 16)] * 65,
        options={"maxiter": 300, "ftol": 1e-10, "gtol": 1e-6},
    )
    model = args.output / "model.json"
    model.write_text(json.dumps(export(fit.x), indent=2, allow_nan=False) + "\n")
    identity = hashlib.sha256(model.read_bytes()).hexdigest()
    theta = load(model, identity)
    # No final scene is generated until model/policy bytes have been frozen.
    held = {}
    for kind in [
        "matched",
        "dual_mono_bed",
        "anti_phase_bed",
        "panned_foreground",
        "phase_shifted_foreground",
        "level_shift",
    ]:
        hx, hy, rows = corpus(20273120, 32, kind)
        held[kind] = {"metrics": metrics(theta, hx, hy), "groups": rows}
    report = {
        "model_sha256": identity,
        "optimizer_success": bool(fit.success),
        "optimizer_message": str(fit.message),
        "iterations": int(fit.nit),
        "training_groups": train,
        "validation_groups": validation,
        "training": metrics(theta, x, y),
        "validation": metrics(theta, vx, vy),
        "held": held,
        "deployment_admitted": False,
        "scope": "Generated stereo centered-foreground/bed energy fractions, not dialogue or instrument identity.64/16/32 disjoint source seeds; final variants share seeds. Confidence is an uncalibrated decision heuristic; mask MSE is not remix waveform quality.",
    }
    (args.output / "training-report.json").write_text(
        json.dumps(report, indent=2, allow_nan=False) + "\n"
    )
    print(
        json.dumps(
            {
                "model_sha256": identity,
                "optimizer_success": report["optimizer_success"],
                "held": {k: v["metrics"] for k, v in held.items()},
            }
        )
    )


if __name__ == "__main__":
    main()
